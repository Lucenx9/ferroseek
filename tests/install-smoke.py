#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Installer integration and transaction regressions; never use the real session."""
import json
import os
from pathlib import Path
import signal
import re
import subprocess
import sys
import tempfile
import tomllib
from types import SimpleNamespace
from unittest.mock import patch

repo = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo / 'scripts'))
import installer
import install_transaction as tx

with tempfile.TemporaryDirectory(prefix='ferroseek-install-') as tmp:
    root = Path(tmp)
    runtime = root / 'runtime'; runtime.mkdir(mode=0o700)
    commands = root / 'commands'
    commands.mkdir()
    log = root / 'calls'
    stub = '''#!/bin/sh
printf '%s %s\\n' "$(basename "$0")" "$*" >> "$TEST_CALLS"
case "$*" in
  *show-environment*) [ "$TEST_MANAGER" = yes ]; exit $?;;
  *status*) [ "${TEST_BUS:-$TEST_MANAGER}" = yes ]; exit $?;;
  *NameHasOwner*) if [ "$TEST_OWNER" = yes ]; then echo 'b true'; else echo 'b false'; fi; exit 0;;
  *GetConnectionUnixProcessID*) echo 'u 999'; exit 0;;
  *MainPID*) echo 999; exit 0;;
  *is-active*|*is-enabled*) [ "$TEST_ACTIVE" = yes ]; exit $?;;
esac
exit 0
'''
    for name in ['systemctl', 'busctl', 'gdbus', 'claude', 'codex', 'cargo', 'noctalia']:
        p = commands / name
        p.write_text('#!/bin/sh\nexit 1\n' if name == 'noctalia' else stub)
        p.chmod(0o755)
    def environment(name='home'):
        home = root / name
        home.mkdir(exist_ok=True)
        return dict(os.environ, HOME=str(home), XDG_DATA_HOME='relative-data', XDG_CONFIG_HOME='relative-config',
                    XDG_STATE_HOME=str(home / '.local/state'), XDG_RUNTIME_DIR=str(runtime), CODEX_HOME=str(home / '.codex'),
                    CLAUDE_CONFIG_DIR=str(home / '.claude-config'), XDG_CURRENT_DESKTOP='KDE',
                    FERROSEEK_CONFIG=str(home / '.config/ferroseek/config.toml'),
                    FERROSEEK_DATA_DIR=str(home / '.local/share/ferroseek'),
                    PATH=str(commands) + ':' + os.environ['PATH'], TEST_CALLS=str(log),
                    TEST_MANAGER='yes', TEST_ACTIVE='no', TEST_OWNER='no', TEST_BUS='yes', DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-install-test')
    # Interactive recommendations and customization, without relying on a CI TTY.
    def options():
        return SimpleNamespace(core=False, mcp=False, krunner=False, full=False, dry_run=False,
                               register_claude=False, register_codex=False, allow=[])
    detected = dict(kde=True, claude=True, codex=True, wayland=True)
    with patch.object(sys.stdin, 'isatty', return_value=True), patch('builtins.input', return_value=''):
        assert installer.choose(options(), detected, None) == {'core', 'launcher'}
        assert installer.choose(options(), dict(detected, wayland=False), None) == {'core'}
        assert installer.choose(options(), detected, {'components': ['core', 'mcp']}) == {'core'}
    with patch.object(sys.stdin, 'isatty', return_value=True), patch('builtins.input', return_value='core'):
        assert installer.choose(options(), detected, None) == {'core'}
    with patch.object(sys.stdin, 'isatty', return_value=True), patch('builtins.input', return_value='q'):
        try:
            installer.choose(options(), detected, None)
        except tx.InstallError:
            pass
        else:
            assert False, 'cancel must not install'
    env = environment()
    home = Path(env['HOME'])
    def run(*args, expected=0, custom=None, text=''):
        r = subprocess.run([str(repo / 'scripts/install.sh'), '--no-build', *args], env=custom or env,
                           cwd=repo, input=text, capture_output=True, text=True)
        assert r.returncode == expected, (args, r.returncode, r.stdout, r.stderr)
        return r
    def uninstall(*args, expected=0, custom=None):
        r = subprocess.run([str(repo / 'scripts/uninstall.sh'), *args], env=custom or env,
                           cwd=repo, capture_output=True, text=True)
        assert r.returncode == expected, (args, r.returncode, r.stdout, r.stderr)
        return r
    cfg = root / 'config.toml'
    cfg.write_text('roots = []\n')
    before = list(home.rglob('*'))
    run('--dry-run')
    assert list(home.rglob('*')) == before
    assert 'daemon-reload' not in log.read_text() and 'cargo' not in log.read_text()
    run(expected=1)  # No implicit non-interactive defaults.
    run('--config', expected=2)
    run('--unexpected', expected=2)
    preview = run('--core', '--config', str(cfg), '--dry-run')
    assert 'Create initial config:' in preview.stdout and str(home / '.config/ferroseek/config.toml') in preview.stdout
    run('--core', '--config', str(cfg))
    bindir = home / '.local/bin'
    assert (bindir / 'ferroseekd').read_bytes() == (repo / 'target/release/ferroseekd').read_bytes()
    assert not (bindir / 'ferroseek-mcp').exists()
    run('--launcher')
    launcher = bindir / 'ferroseek-launcher'
    assert launcher.read_bytes() == (repo / 'target/release/ferroseek-launcher').read_bytes()
    assert launcher.stat().st_mode & 0o777 == 0o755
    run('--core')
    assert launcher.exists()  # Core upgrades retain the installed UI.
    uninstall('--core', expected=1)
    launcher.write_bytes(b'local override')
    uninstall('--launcher', expected=1)
    run('--launcher', expected=1)
    assert launcher.read_bytes() == b'local override'
    launcher.write_bytes((repo / 'target/release/ferroseek-launcher').read_bytes())
    uninstall('--launcher')
    assert not launcher.exists() and (bindir / 'ferroseekd').exists()
    runner = home / '.local/share/krunner/dbusplugins/ferroseek.desktop'
    assert not runner.exists()
    config = home / '.config/ferroseek/config.toml'
    config.write_text('# preserve custom config\nroots = []\n')
    data = home / '.local/share/ferroseek'
    data.mkdir()
    (data / 'sentinel').write_text('preserve')
    run('--mcp', '--krunner', '--config', str(cfg))
    assert runner.exists() and (bindir / 'ferroseek-mcp').exists()
    run('--full')
    assert config.read_text().startswith('# preserve')
    assert (data / 'sentinel').read_text() == 'preserve'
    assert not any(word in log.read_text() for word in ['restart', 'enable --now', 'disable'])
    run('--core', custom=dict(env, TEST_ACTIVE='yes'))  # Never restart an active daemon.
    assert 'restart' not in log.read_text()
    run('--core', '--enable')
    assert 'enable --now ferroseek.service' in log.read_text()
    uninstall('--core', expected=1)
    uninstall('--mcp')
    assert runner.exists() and not (bindir / 'ferroseek-mcp').exists()
    uninstall(custom=dict(env, TEST_ACTIVE='yes'), expected=1)
    uninstall('--dry-run')
    assert (bindir / 'ferroseekd').exists()
    uninstall()
    uninstall()
    assert not (bindir / 'ferroseekd').exists() and config.exists() and (data / 'sentinel').exists()

    # Unknown files, symlinks and modified managed files cannot be overwritten.
    conflict = environment('conflict')
    p = Path(conflict['HOME']) / '.local/bin/ferroseek'
    p.parent.mkdir(parents=True)
    p.write_text('unrelated file')
    run('--core', custom=conflict, expected=1)
    assert p.read_text() == 'unrelated file' and not p.with_name('ferroseekd').exists()
    p.unlink()
    p.symlink_to(cfg)
    run('--core', custom=conflict, expected=1)
    assert cfg.read_text() == 'roots = []\n'
    p.unlink()
    run('--core', custom=conflict)
    p.write_text('local override')
    run('--core', custom=conflict, expected=1)
    uninstall(custom=conflict, expected=1)
    assert p.read_text() == 'local override'

    # All printable filename characters are handled, not interpolated into a shell.
    special = environment('home=with:hash# spazio é & % $ " \' \\ ` ;')
    # Service search paths use colon delimiters; keep those XDG roots separate
    # while still exercising a colon in the executable path itself.
    special.update(XDG_DATA_HOME=str(root / 'special-data'), XDG_CONFIG_HOME=str(root / 'special-config'),
                   FERROSEEK_CONFIG=str(root / 'special-config/ferroseek/config.toml'),
                   FERROSEEK_DATA_DIR=str(root / 'special-data/ferroseek'))
    run('--full', custom=special)
    unit = Path(special['XDG_CONFIG_HOME']) / 'systemd/user/ferroseek.service'
    # systemd-analyze treats ':' in its UNIT argument as an alias separator.
    # Verify identical unit bytes from an unambiguous temporary argument path.
    verify_dir = root / 'unit-verify'; verify_dir.mkdir()
    verify_unit = verify_dir / 'ferroseek.service'; verify_unit.write_bytes(unit.read_bytes())
    r = subprocess.run(['systemd-analyze', '--user', 'verify', str(verify_unit)], env=special, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    # Exercise the actual D-Bus argv parser with those same special characters.
    # No systemd manager is contacted: stub detection says unavailable.
    private = dict(special, TEST_MANAGER='no', TEST_BUS='no')
    run('--core', '--config', str(cfg), custom=private)
    bus = subprocess.Popen(['/usr/bin/dbus-daemon', '--session', '--nofork', '--print-address=1'],
                           env=private, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    daemon_pid = None
    try:
        address = bus.stdout.readline().strip()
        assert address.startswith('unix:'), address
        reply = subprocess.run(['/usr/bin/gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                '--object-path', '/io/github/ferroseek/FerroSeek', '--method',
                                'io.github.ferroseek.FerroSeek1.Status'],
                               env=dict(private, DBUS_SESSION_BUS_ADDRESS=address), capture_output=True, text=True, timeout=20)
        assert reply.returncode == 0, reply.stderr
        candidate_pid = int(re.search(r"'pid': '([0-9]+)'", reply.stdout)[1])
        assert Path(f'/proc/{candidate_pid}/exe').resolve() == Path(special['HOME']) / '.local/bin/ferroseekd'
        daemon_pid = candidate_pid
        manager_off = root / 'manager-off'; manager_off.mkdir()
        no_manager = manager_off / 'systemctl'
        no_manager.write_text('#!/bin/sh\nexit 1\n'); no_manager.chmod(0o755)
        actual_bus = dict(private, DBUS_SESSION_BUS_ADDRESS=address, PATH=str(manager_off) + ':/usr/bin:/bin')
        refusal = uninstall('--stop', custom=actual_bus, expected=1)
        assert 'unmanaged daemon' in refusal.stderr
        assert (Path(special['HOME']) / '.local/bin/ferroseekd').exists()
    finally:
        if daemon_pid is not None:
            os.kill(daemon_pid, signal.SIGTERM)
        bus.terminate()
        bus.communicate(timeout=10)
    uninstall(custom=special)
    bad = environment('control\npath')
    run('--core', custom=bad, expected=1)
    assert not list(Path(bad['HOME']).iterdir())
    colon = environment('colon:registration')
    refusal = run('--core', custom=colon, expected=1)
    assert 'registration paths cannot contain colons' in refusal.stderr
    assert not list(Path(colon['HOME']).iterdir())

    # Headless/no manager: core works, enabling fails, incompatible KDE is refused.
    headless = dict(environment('headless'), XDG_CURRENT_DESKTOP='GNOME', TEST_MANAGER='no', TEST_BUS='no')
    run('--core', custom=headless)
    run('--core', '--enable', custom=headless, expected=1)
    assert 'SystemdService=' not in (Path(headless['HOME']) / '.local/share/dbus-1/services/io.github.ferroseek.FerroSeek.service').read_text()
    # KDE binaries might also be installed on the test host; desktop selection wins.
    run('--krunner', custom=headless, expected=1)
    run('--full', custom=headless)
    assert not (Path(headless['HOME']) / '.local/share/krunner').exists()
    unmanaged = dict(headless, TEST_BUS='yes', TEST_OWNER='yes')
    refusal = uninstall('--stop', custom=unmanaged, expected=1)
    assert 'unmanaged daemon' in refusal.stderr
    assert (Path(headless['HOME']) / '.local/bin/ferroseekd').exists()
    managed = dict(headless, TEST_BUS='yes', TEST_OWNER='yes', TEST_MANAGER='yes', TEST_ACTIVE='yes')
    uninstall('--stop', custom=managed)
    assert 'disable --now ferroseek.service' in log.read_text()

    # Explicit, previewable agent registration preserves unrelated fields/comments.
    agents = environment('agents é')
    ahome = Path(agents['HOME'])
    allowed = ahome / 'source with spaces'; allowed.mkdir()
    cp = Path(agents['CLAUDE_CONFIG_DIR']) / '.claude.json'; cp.parent.mkdir()
    cp.write_text('{"otherSetting":true,"mcpServers":{"other":{"command":"other"}}}')
    xp = Path(agents['CODEX_HOME']) / 'config.toml'; xp.parent.mkdir()
    xp.write_text('# keep this comment\nmodel = "test"\n[mcp_servers.other]\ncommand = "other"\n')
    oldc, oldx = cp.read_bytes(), xp.read_bytes()
    flags = ['--mcp', '--register-claude', '--register-codex', '--allow', str(allowed)]
    run(*flags, '--dry-run', custom=agents)
    assert cp.read_bytes() == oldc and xp.read_bytes() == oldx
    run(*flags, custom=agents, text='no\nno\n')
    assert cp.read_bytes() == oldc and xp.read_bytes() == oldx
    run('--mcp', '--register-codex', custom=agents, expected=1)
    run(*flags, custom=agents, text='yes\nyes\n')
    c = json.loads(cp.read_text()); x = tomllib.loads(xp.read_text())
    assert c['otherSetting'] and c['mcpServers']['other']['command'] == 'other'
    assert xp.read_text().startswith('# keep this comment') and x['model'] == 'test'
    assert c['mcpServers']['ferroseek']['args'][-2:] == ['--allow', str(allowed)]
    assert x['mcp_servers']['ferroseek']['args'][-2:] == ['--allow', str(allowed)]
    if os.environ.get('FERROSEEK_TEST_AGENT_PARSERS') == '1':
        for exe, argv in [('/usr/bin/codex', ['mcp', 'get', 'ferroseek', '--json']),
                          ('/usr/bin/claude', ['mcp', 'get', 'ferroseek'])]:
            assert Path(exe).is_file()
            result = subprocess.run([exe, *argv], env=agents, cwd=ahome,
                                    capture_output=True, text=True, timeout=30)
            assert result.returncode == 0 and 'ferroseek' in result.stdout, result.stderr
    oldc, oldx = cp.read_bytes(), xp.read_bytes()
    run(*flags, custom=agents)  # Identical registration: no confirmation/write needed.
    assert cp.read_bytes() == oldc and xp.read_bytes() == oldx
    run('--mcp', '--register-codex', '--allow', str(ahome), custom=agents, text='yes\n', expected=1)
    assert xp.read_bytes() == oldx
    uninstall(custom=agents)
    assert cp.read_bytes() == oldc and xp.read_bytes() == oldx

    # Global setup needs explicit consent, including on redirected stdin.
    global_env = environment('global-agent')
    global_cfg = Path(global_env['CODEX_HOME']) / 'config.toml'
    global_flags = ['--mcp', '--register-codex', '--register-claude', '--mcp-access', 'all_indexed']
    run(*global_flags, '--dry-run', custom=global_env)
    assert not global_cfg.exists()
    run(*global_flags, custom=global_env, expected=1)
    assert not global_cfg.exists()
    run(*global_flags, custom=global_env, text='all_indexed\nyes\nyes\n')
    global_entry = tomllib.loads(global_cfg.read_text())['mcp_servers']['ferroseek']
    assert global_entry['args'][-2:] == ['--access-mode', 'all_indexed']
    assert '--allow' not in global_entry['args']
    claude_entry = json.loads((Path(global_env['CLAUDE_CONFIG_DIR']) / '.claude.json').read_text())['mcpServers']['ferroseek']
    assert claude_entry['args'] == global_entry['args']
    assert not (Path(global_env['HOME']) / '.config/ferroseek/config.toml').exists()
    uninstall(custom=global_env)

    # No --mcp-access is now a global recommendation, but never silently
    # activates: declined/EOF registration preserves the existing config.
    default_env = environment('new-global-default')
    default_cfg = Path(default_env['CODEX_HOME']) / 'config.toml'
    run('--mcp', '--register-codex', '--dry-run', custom=default_env)
    run('--mcp', '--register-codex', custom=default_env, expected=1)
    assert not default_cfg.exists()
    for consent in ['all_indexed\n', 'all_indexed\nno\n']:
        run('--mcp', '--register-codex', custom=default_env, text=consent)
        assert not default_cfg.exists()
    run('--mcp', '--register-codex', custom=default_env, text='all_indexed\nyes\n')
    default_entry = tomllib.loads(default_cfg.read_text())['mcp_servers']['ferroseek']
    assert default_entry['args'][-2:] == ['--access-mode', 'all_indexed']
    uninstall(custom=default_env)

    # Transaction failures restore original bytes/modes, including agent configs.
    state = root / 'transaction'; targets = {'one': root / 'one', 'two': root / 'two'}
    targets['one'].write_bytes(b'old'); targets['one'].chmod(0o640)
    transaction = tx.Transaction(state, targets)
    original = tx.atomic_write
    def fail_second(path, content, mode):
        if path == targets['two']:
            raise OSError('injected disk failure')
        return original(path, content, mode)
    with transaction.locked():
        with patch.object(tx, 'atomic_write', fail_second):
            try:
                transaction.apply({'one': (b'old', b'new', 0o755), 'two': (None, b'new', 0o600)})
            except OSError:
                pass
            else:
                assert False, 'fault was not injected'
    assert targets['one'].read_bytes() == b'old' and targets['one'].stat().st_mode & 0o777 == 0o640
    assert not targets['two'].exists() and not transaction.journal.exists()

    # Content-identical chmod updates must also roll back their original mode.
    with transaction.locked():
        with patch.object(tx, 'atomic_write', fail_second):
            try:
                transaction.apply({'one': (b'old', b'old', 0o755), 'two': (None, b'new', 0o600)})
            except OSError:
                pass
            else:
                assert False, 'fault was not injected'
    assert targets['one'].stat().st_mode & 0o777 == 0o640

    # Abrupt termination after the first replace: next lock recovers the journal.
    child = '''import os,sys
from pathlib import Path
sys.path.insert(0,sys.argv[1])
import install_transaction as t
state=Path(sys.argv[2]);one=Path(sys.argv[3]);two=Path(sys.argv[4])
orig=t.atomic_write
def die(path,data,mode):
    orig(path,data,mode)
    if path==one:os._exit(77)
t.atomic_write=die
x=t.Transaction(state,{'one':one,'two':two})
with x.locked():x.apply({'one':(b'old',b'new',0o755),'two':(None,b'new',0o600)})
'''
    r = subprocess.run([sys.executable, '-c', child, str(repo / 'scripts'), str(state), str(targets['one']), str(targets['two'])])
    assert r.returncode == 77 and transaction.journal.exists()
    with transaction.locked():
        pass
    assert targets['one'].read_bytes() == b'old' and not targets['two'].exists()
    # A concurrent writer wins: recovery refuses to clobber its new contents.
    r = subprocess.run([sys.executable, '-c', child, str(repo / 'scripts'), str(state), str(targets['one']), str(targets['two'])])
    targets['one'].write_bytes(b'external writer')
    try:
        with transaction.locked():
            pass
    except tx.InstallError:
        pass
    else:
        assert False, 'rollback overwrote an external writer'
    assert targets['one'].read_bytes() == b'external writer'

    # An external write during rollback must not be replaced by an old backup.
    race_targets = {name: root / ('race-' + name) for name in ['one', 'two', 'three']}
    race_targets['one'].write_bytes(b'old1'); race_targets['two'].write_bytes(b'old2')
    race = tx.Transaction(root / 'race-state', race_targets)
    def modify_during_rollback(path, content, mode):
        if path == race_targets['three']:
            raise OSError('injected failure')
        original(path, content, mode)
        if path == race_targets['two'] and content == b'old2':
            race_targets['one'].write_bytes(b'external during rollback')
    with race.locked():
        with patch.object(tx, 'atomic_write', modify_during_rollback):
            try:
                race.apply({'one': (b'old1', b'new1', 0o644), 'two': (b'old2', b'new2', 0o644), 'three': (None, b'new3', 0o644)})
            except tx.InstallError:
                pass
            else:
                assert False, 'rollback clobbered concurrent write'
    assert race.journal.exists() and race_targets['one'].read_bytes() == b'external during rollback'

print('PASS: component selection, upgrades, dry-run, paths, ownership, agent consent, rollback and crash recovery')
