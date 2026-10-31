#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""User-scoped component installation, independent of the running daemon."""
import argparse
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import tomllib

from install_transaction import InstallError, Transaction, digest, read, safe_path

ROOT = Path(__file__).resolve().parents[1]
COMPONENT_FILES = {'core': ['daemon', 'cli', 'unit', 'bus'], 'mcp': ['mcp'], 'krunner': ['runner']}
BINARIES = {'daemon': 'ferroseekd', 'cli': 'ferroseek', 'mcp': 'ferroseek-mcp'}
PACKAGES = {'daemon': 'ferroseek-daemon', 'cli': 'ferroseek-cli', 'mcp': 'ferroseek-mcp'}
ARCHIVE_FILES = ['LICENSE', 'README.md', 'docs/INSTALLATION.md', 'docs/MCP.md',
                 'scripts/install.sh', 'scripts/uninstall.sh', 'scripts/installer.py', 'scripts/install_transaction.py',
                 'data/ferroseek.service', 'data/io.github.ferroseek.FerroSeek.service', 'data/ferroseek-krunner.desktop']


def command(args):
    try:
        return subprocess.run(args, capture_output=True, text=True, timeout=8)
    except (OSError, subprocess.TimeoutExpired):
        return None


def succeeds(args):
    result = command(args)
    return result is not None and result.returncode == 0


def paths():
    home = Path(os.environ.get('HOME', ''))
    if not home.is_absolute():
        raise InstallError('HOME must be an absolute path')
    def xdg(key, fallback):
        value = Path(os.environ.get(key) or home / fallback)
        return value if value.is_absolute() else home / fallback
    data = xdg('XDG_DATA_HOME', '.local/share')
    config = xdg('XDG_CONFIG_HOME', '.config')
    state = xdg('XDG_STATE_HOME', '.local/state') / 'ferroseek/installer'
    codex = Path(os.environ.get('CODEX_HOME') or home / '.codex') / 'config.toml'
    claude = Path(os.environ['CLAUDE_CONFIG_DIR']) / '.claude.json' if os.environ.get('CLAUDE_CONFIG_DIR') else home / '.claude.json'
    targets = dict(daemon=home / '.local/bin/ferroseekd', cli=home / '.local/bin/ferroseek',
                   mcp=home / '.local/bin/ferroseek-mcp', unit=config / 'systemd/user/ferroseek.service',
                   bus=data / 'dbus-1/services/io.github.ferroseek.FerroSeek.service',
                   runner=data / 'krunner/dbusplugins/ferroseek.desktop', config=config / 'ferroseek/config.toml',
                   policy=config / 'ferroseek/mcp.toml', manifest=state / 'manifest.json',
                   agent_codex=codex, agent_claude=claude)
    return home, data, config, state, targets


def detect():
    try:
        distro = platform.freedesktop_os_release().get('PRETTY_NAME', 'Linux')
    except OSError:
        distro = 'Linux'
    desktop = os.environ.get('XDG_CURRENT_DESKTOP') or os.environ.get('XDG_SESSION_DESKTOP') or os.environ.get('DESKTOP_SESSION') or 'headless/unknown'
    kde = any(x in desktop.lower() for x in ['kde', 'plasma']) or (desktop == 'headless/unknown' and bool(shutil.which('plasmashell') and shutil.which('krunner')))
    has_bus_address = bool(os.environ.get('DBUS_SESSION_BUS_ADDRESS')) or (Path(os.environ.get('XDG_RUNTIME_DIR', '/nonexistent')) / 'bus').is_socket()
    bus = has_bus_address and (succeeds(['busctl', '--user', 'status']) if shutil.which('busctl') else succeeds(['gdbus', 'call', '--session', '--dest', 'org.freedesktop.DBus', '--object-path', '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.GetId']))
    manager = succeeds(['systemctl', '--user', 'show-environment'])
    return dict(distro=distro, arch=platform.machine(), desktop=desktop, kde=kde, bus=bus, manager=manager,
                claude=bool(shutil.which('claude')), codex=bool(shutil.which('codex')))


def systemd_quote(value):
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'


def render(targets, home, data, config, manager, version):
    # systemd restricts characters in the executable itself. env execs the
    # quoted path as an argument; ':' disables dollar expansion, not specifiers.
    executable = systemd_quote(str(targets['daemon']))
    unit = (ROOT / 'data/ferroseek.service').read_text().replace('ExecStart=@BINDIR@/ferroseekd', 'ExecStart=:/usr/bin/env -- ' + executable)
    environment = '\n'.join('Environment=' + systemd_quote(f'{k}={v}') for k, v in
                            [('HOME', home), ('XDG_DATA_HOME', data), ('XDG_CONFIG_HOME', config)])
    unit = unit.replace('[Service]', '[Service]\n' + environment)
    # D-Bus desktop files unescape backslashes, then parse a shell-style argv.
    argv = ['/usr/bin/env', f'HOME={home}', f'XDG_DATA_HOME={data}', f'XDG_CONFIG_HOME={config}', str(targets['daemon'])]
    bus_exec = shlex.join(argv).replace('\\', '\\\\')
    bus = '[D-BUS Service]\nName=io.github.ferroseek.FerroSeek\nExec=' + bus_exec + '\n'
    if manager:
        bus += 'SystemdService=ferroseek.service\n'
    runner = (ROOT / 'data/ferroseek-krunner.desktop').read_text().replace('@VERSION@', version)
    return dict(unit=unit.encode(), bus=bus.encode(), runner=runner.encode())


def load_manifest(targets):
    raw = read(targets['manifest'])
    if raw is None:
        return dict(format=1, components=[], files={})
    manifest = json.loads(raw)
    if manifest.get('format') != 1 or not isinstance(manifest.get('files'), dict):
        raise InstallError('unrecognized installer manifest')
    if not set(manifest['files']) <= set(sum(COMPONENT_FILES.values(), [])) or not set(manifest['components']) <= set(COMPONENT_FILES):
        raise InstallError('invalid installer manifest entries')
    # A changed XDG layout must not strand or overwrite an older installation.
    for key, entry in manifest['files'].items():
        if entry['path'] != str(targets[key]):
            raise InstallError('installed paths differ; use the original HOME/XDG environment to uninstall first')
    return manifest


def archive_info():
    path = ROOT / 'package.json'
    if not path.exists():
        return None
    package = json.loads(read(path))
    components = set(package['components'])
    if package.get('format') != 1 or 'core' not in components or not components <= set(COMPONENT_FILES):
        raise InstallError('unsupported archive manifest')
    expected = set(ARCHIVE_FILES) | {'bin/ferroseek', 'bin/ferroseekd'}
    if 'mcp' in components:
        expected.add('bin/ferroseek-mcp')
    if set(package['sha256']) != expected:
        raise InstallError('archive checksum manifest is incomplete or unexpected')
    if package['os'] != 'linux' or package['arch'] != platform.machine():
        raise InstallError('archive platform does not match this machine')
    # Integrity only: authenticate the downloaded archive checksum separately.
    for name, expected in package['sha256'].items():
        p = Path(name)
        if p.is_absolute() or '..' in p.parts or not p.parts:
            raise InstallError('unsafe archive manifest path')
        if digest(read(ROOT / p)) != expected:
            raise InstallError(f'archive integrity check failed: {name}')
    return package


def choose(args, env, package):
    selected = {name for name in COMPONENT_FILES if getattr(args, name)}
    if args.full:
        selected |= {'core', 'mcp'}
        if env['kde']:
            selected.add('krunner')
        else:
            print('No KDE detected: --full skips KRunner; no GNOME/Hyprland graphical integration exists.')
    if args.register_claude or args.register_codex:
        selected.add('mcp')
    if not selected:
        recommended = {'core'} | ({'krunner'} if env['kde'] else set()) | ({'mcp'} if env['claude'] or env['codex'] else set())
        if package:
            recommended &= set(package['components'])
        print('Recommended components: ' + ', '.join(sorted(recommended)))
        if args.dry_run:
            selected = recommended
        elif not sys.stdin.isatty():
            raise InstallError('non-interactive input: choose --core, --mcp, --krunner or --full (or --dry-run)')
        else:
            reply = input('Components [Enter = recommended; core mcp krunner; q = cancel]: ').strip()
            if reply == 'q':
                raise InstallError('installation cancelled')
            selected = set(reply.replace(',', ' ').split()) if reply else recommended
            if not selected <= set(COMPONENT_FILES):
                raise InstallError('unknown component selection')
            if 'mcp' in selected:
                for agent in ['claude', 'codex']:
                    if env[agent] and input(f'Offer registration for {agent}? [y/N] ').strip().lower() == 'y':
                        setattr(args, 'register_' + agent, True)
                if (args.register_claude or args.register_codex) and not args.allow:
                    directory = input('Directory to authorize for MCP (absolute path; empty = no registration): ').strip()
                    if directory:
                        args.allow = [directory]
                    else:
                        args.register_claude = args.register_codex = False
    selected.add('core')
    if 'krunner' in selected and not env['kde']:
        raise InstallError('KRunner requires KDE Plasma; use --core or --mcp on this desktop')
    if package and not selected <= set(package['components']):
        raise InstallError('this archive lacks the requested component; obtain the matching or full archive')
    return selected


def agent_changes(args, targets, data, env):
    changes = {}
    for agent in ['claude', 'codex']:
        if not getattr(args, 'register_' + agent):
            continue
        if not env[agent]:
            raise InstallError(f'{agent} CLI is not available; install it before registration')
        if not args.allow:
            raise InstallError('agent registration requires explicit --allow DIR; no home-directory default')
        roots = []
        for value in args.allow:
            p = Path(value)
            if not p.is_absolute() or not p.is_dir():
                raise InstallError('--allow requires an existing absolute directory')
            p = p.resolve()
            safe_path(p / '.ferroseek-path-validation')
            roots.append(str(p))
        if len(roots) > 32:
            raise InstallError('at most 32 MCP directories are allowed')
        argv = ['--index', str(data / 'ferroseek/index.sqlite')]
        if targets['policy'].exists():
            argv += ['--config', str(targets['policy'])]
        for root in roots:
            argv += ['--allow', root]
        executable = str(targets['mcp'])
        key = 'agent_' + agent
        old = read(targets[key])
        if agent == 'claude':
            obj = json.loads(old) if old else {}
            if not isinstance(obj, dict) or not isinstance(obj.get('mcpServers', {}), dict):
                raise InstallError('Claude configuration must contain a JSON object of MCP servers')
            servers = obj.setdefault('mcpServers', {})
            entry = dict(type='stdio', command=executable, args=argv)
            present = 'ferroseek' in servers
            existing = servers.get('ferroseek')
            servers['ferroseek'] = entry
            new = (json.dumps(obj, ensure_ascii=False, indent=2) + '\n').encode()
        else:
            obj = tomllib.loads(old.decode()) if old else {}
            entry = dict(command=executable, args=argv)
            present = 'ferroseek' in obj.get('mcp_servers', {})
            existing = obj.get('mcp_servers', {}).get('ferroseek')
            block = '\n[mcp_servers.ferroseek]\ncommand = ' + json.dumps(executable, ensure_ascii=False) + '\nargs = ' + json.dumps(argv, ensure_ascii=False) + '\n'
            new = (old or b'') + block.encode()
            # Refuse inline-table/dotted-key structures that cannot be extended safely.
            if existing is None:
                tomllib.loads(new.decode())
        if present:
            if existing == entry:
                print(f'{agent}: identical registration already present; unchanged.')
                continue
            raise InstallError(f'{agent}: existing ferroseek registration differs; refusing to overwrite it')
        print(f'Agent registration would edit {targets[key]}')
        print('  Command: ' + shlex.join([executable, *argv]))
        print('  Existing MCP policy remains in effect, including any previously authorized roots.')
        if args.dry_run:
            print('  Requires explicit confirmation during installation.')
            continue
        try:
            consent = input(f'Type yes to modify {agent} configuration: ').strip()
        except EOFError:
            consent = ''
        if consent != 'yes':
            print(f'{agent}: registration declined; configuration unchanged.')
            continue
        changes[key] = (old, new, 0o600)
    return changes


def refresh(env):
    commands = []
    if env['manager']:
        commands.append(['systemctl', '--user', 'daemon-reload'])
    if env['bus']:
        if shutil.which('busctl'):
            commands.append(['busctl', '--user', 'call', 'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'ReloadConfig'])
        else:
            commands.append(['gdbus', 'call', '--session', '--dest', 'org.freedesktop.DBus', '--object-path', '/org/freedesktop/DBus', '--method', 'org.freedesktop.DBus.ReloadConfig'])
    for cmd in commands:
        if not succeeds(cmd):
            print('Registration refresh failed; files are installed. Retry: ' + shlex.join(cmd), file=sys.stderr)


def install(args, env, layout):
    home, data, config, state, targets = layout
    package = archive_info()
    selected = choose(args, env, package)
    if args.enable and not (env['manager'] and env['bus']):
        raise InstallError('--enable requires an available systemd user manager and session D-Bus')
    version = package['version'] if package else tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    print('Install: ' + ', '.join(sorted(selected)))
    print('Existing configuration: ' + ('preserved' if targets['config'].exists() else 'none (engine defaults apply)'))
    print('Running daemons are never restarted automatically.')
    if not env['kde']:
        print('CLI and MCP work without graphical integration on this desktop.')
    if not env['bus']:
        print('No session D-Bus: foreground CLI indexing/search remains available.')
    keys = [k for component in sorted(selected) for k in COMPONENT_FILES[component]]
    for key in keys:
        safe_path(targets[key])
        print('  ' + str(targets[key]))
    initial_config = Path(args.config).read_bytes() if args.config else None
    if initial_config is not None:
        tomllib.loads(initial_config.decode())
    if args.dry_run:
        # Read-only checks, no build, lock, journal, directory creation or refresh.
        print('Would use archive binaries.' if package else 'Would use existing binaries.' if args.no_build else 'Would build selected Rust packages with --release --locked.')
        manifest = load_manifest(targets)
        for key in keys:
            old = read(targets[key])
            owned = manifest['files'].get(key)
            if old is not None and (not owned or digest(old) != owned['sha256']):
                print(f'  Existing unowned/modified file requires a byte-identical replacement or manual resolution: {targets[key]}')
        if (state / 'transaction.json').exists():
            print('Pending transaction would be recovered before installation.')
        agent_changes(args, targets, data, env)
        if args.enable:
            print('Would enable and start ferroseek.service (explicit --enable).')
        print('Dry run: nothing changed.')
        return
    source = ROOT / ('bin' if package else 'target/release')
    if not package and not args.no_build:
        cmd = ['cargo', 'build', '--release', '--locked']
        for key in keys:
            if key in PACKAGES:
                cmd += ['-p', PACKAGES[key]]
        subprocess.run(cmd, cwd=ROOT, check=True)
    content = render(targets, home, data, config, env['manager'], version)
    for key in keys:
        if key in BINARIES:
            p = source / BINARIES[key]
            if not p.is_file() or not os.access(p, os.X_OK):
                raise InstallError(f'missing executable: {p}; build first or use the matching archive')
            content[key] = p.read_bytes()
    transaction = Transaction(state, targets)
    with transaction.locked():
        manifest = load_manifest(targets)
        changes = {}
        for key in keys:
            old, new = read(targets[key]), content[key]
            owned = manifest['files'].get(key)
            if old is not None and old != new and (not owned or digest(old) != owned['sha256']):
                raise InstallError(f'refusing to overwrite unowned or locally modified file: {targets[key]}')
            mode = 0o755 if key in BINARIES else 0o644
            if old != new or (targets[key].stat().st_mode & 0o777) != mode:
                changes[key] = (old, new, mode)
            manifest['files'][key] = dict(path=str(targets[key]), sha256=digest(new))
        if initial_config is not None and read(targets['config']) is None:
            changes['config'] = (None, initial_config, 0o600)
        changes.update(agent_changes(args, targets, data, env))
        manifest.update(version=version, components=sorted(set(manifest['components']) | selected))
        old_manifest = read(targets['manifest'])
        new_manifest = (json.dumps(manifest, indent=2) + '\n').encode()
        if old_manifest != new_manifest:
            changes['manifest'] = (old_manifest, new_manifest, 0o600)
        transaction.apply(changes)
    refresh(env)
    if args.enable and not succeeds(['systemctl', '--user', 'enable', '--now', 'ferroseek.service']):
        raise InstallError('files installed, but enabling the service failed; retry systemctl --user enable --now ferroseek.service')
    print('Installed. Configuration and indexes preserved. No service restart performed.')
    print('Add ' + shlex.quote(str(home / '.local/bin')) + ' to PATH if necessary.')
    print('Choose indexing roots with ferroseek config before starting; engine defaults index your home.')
    if 'krunner' in selected:
        print('KRunner discovers new runners at its next restart/login. No KDE settings were changed.')
    if 'mcp' in selected:
        print('MCP stays off until a client starts it; access requires an explicit allowlist.')


def uninstall(args, env, layout):
    _, _, _, state, targets = layout
    selected = {name for name in COMPONENT_FILES if getattr(args, name)}
    if args.full or not selected:
        selected = set(COMPONENT_FILES)
    manifest = load_manifest(targets)
    if not manifest['files'] and not (state / 'transaction.json').exists():
        print('No managed installation; no files removed. Legacy/unrelated files and all data preserved.')
        return
    def plan():
        manifest = load_manifest(targets)
        remaining = set(manifest['components']) - selected
        if 'core' in selected and remaining:
            raise InstallError('core is required by installed integrations; remove them too or uninstall all')
        changes = {}
        for component in selected:
            for key in COMPONENT_FILES[component]:
                entry = manifest['files'].get(key)
                if entry is None:
                    continue
                old = read(targets[key])
                if old is not None and digest(old) != entry['sha256']:
                    raise InstallError(f'locally modified file retained; resolve before uninstalling: {targets[key]}')
                if old is not None:
                    changes[key] = (old, None, 0o600)
                manifest['files'].pop(key)
        manifest['components'] = sorted(remaining)
        changes['manifest'] = (read(targets['manifest']), (json.dumps(manifest, indent=2) + '\n').encode() if manifest['files'] else None, 0o600)
        return changes
    def check_service():
        if 'core' in selected and env['manager']:
            busy = succeeds(['systemctl', '--user', 'is-active', '--quiet', 'ferroseek.service']) or succeeds(['systemctl', '--user', 'is-enabled', '--quiet', 'ferroseek.service'])
            if busy and not args.stop:
                raise InstallError('service active or enabled; uninstall needs explicit --stop (never implicit)')
    if args.dry_run:
        if (state / 'transaction.json').exists():
            print('Pending transaction would be recovered before removal.')
        else:
            for key in plan():
                print('Remove/update: ' + str(targets[key]))
        check_service()
        print('Dry run: nothing changed.')
        return
    transaction = Transaction(state, targets)
    with transaction.locked():
        changes = plan()
        check_service()
        for key in changes:
            print('Remove/update: ' + str(targets[key]))
        if args.stop and 'core' in selected and env['manager']:
            if not succeeds(['systemctl', '--user', 'disable', '--now', 'ferroseek.service']):
                raise InstallError('could not stop/disable the service; no files removed')
        transaction.apply(changes)
    refresh(env)
    print('Uninstalled selected components. Indexes, configuration and agent registrations preserved.')
    if 'mcp' in selected:
        print('Remove an agent registration separately if desired: claude mcp remove --scope user ferroseek; codex mcp remove ferroseek')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ['full', 'core', 'mcp', 'krunner', 'dry-run', 'no-build', 'enable', 'register-claude', 'register-codex', 'stop']:
        parser.add_argument('--' + flag, action='store_true')
    parser.add_argument('--config', help='initial engine TOML; never replace an existing config')
    parser.add_argument('--allow', action='append', default=[], metavar='DIR', help='explicit MCP directory for agent registration (repeatable)')
    parser.add_argument('--uninstall', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if sys.platform != 'linux':
            raise InstallError('FerroSeek supports Linux only')
        if os.geteuid() == 0:
            raise InstallError('run as your normal user, without sudo')
        if args.uninstall and (args.enable or args.no_build or args.config or args.allow or args.register_claude or args.register_codex):
            raise InstallError('installation options cannot be used for uninstall')
        if not args.uninstall and args.stop:
            raise InstallError('--stop is only for uninstall')
        if args.allow and not (args.register_claude or args.register_codex):
            raise InstallError('--allow requires --register-claude or --register-codex')
        layout = paths()
        env = detect()
        print(f"Detected: {env['distro']}; {env['arch']}; desktop={env['desktop']}")
        print(f"Session D-Bus={env['bus']}; systemd user={env['manager']}; Claude={env['claude']}; Codex={env['codex']}")
        (uninstall if args.uninstall else install)(args, env, layout)
    except (InstallError, OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as e:
        print('ferroseek installer: ' + str(e), file=sys.stderr)
        return 1
    except (KeyboardInterrupt, EOFError):
        print('Installation cancelled.', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
