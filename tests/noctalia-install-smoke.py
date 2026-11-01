#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise managed Noctalia install/upgrade/removal without a desktop session."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
from unittest.mock import patch

repo = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo / 'scripts'))
import installer
import install_transaction as tx


with tempfile.TemporaryDirectory(prefix='ferroseek-noctalia-install-') as temp:
    root = Path(temp)
    source = root / 'checkout'
    source.mkdir()
    for directory in ['scripts', 'data', 'integrations']:
        shutil.copytree(repo / directory, source / directory, ignore=shutil.ignore_patterns('__pycache__'))
    (source / 'docs').mkdir()
    for name in ['INSTALLATION.md', 'MCP.md', 'NOCTALIA.md']:
        shutil.copy2(repo / 'docs' / name, source / 'docs' / name)
    shutil.copy2(repo / 'Cargo.toml', source / 'Cargo.toml')
    release = source / 'target/release'
    release.mkdir(parents=True)
    for name in ['ferroseek', 'ferroseekd']:
        shutil.copy2(repo / 'target/release' / name, release / name)
    commands = root / 'commands'
    commands.mkdir()
    calls = root / 'calls.jsonl'
    stub = f'#!{sys.executable}\n' + '''import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['TEST_CALLS'], 'a') as f:
    f.write(json.dumps([name, *sys.argv[1:]]) + '\\n')
if name == 'noctalia':
    if sys.argv[1:] == ['--version']:
        print(os.environ.get('TEST_VERSION', 'noctalia v5.4.1'))
    elif os.environ.get('TEST_OFFLINE') == 'yes':
        sys.exit(1)
    elif sys.argv[1:4] == ['msg', 'plugins', 'enable'] and os.environ.get('TEST_ENABLE_FAIL') == 'yes':
        sys.exit(1)
    elif sys.argv[1:4] == ['msg', 'plugins', 'disable'] and os.environ.get('TEST_DISABLE_FAIL') == 'yes':
        sys.exit(1)
    elif sys.argv[1:3] == ['msg', 'config-get']:
        print('[]')
elif 'NameHasOwner' in sys.argv:
    print('b false')
elif 'is-active' in sys.argv or 'is-enabled' in sys.argv:
    sys.exit(1)
'''
    for name in ['systemctl', 'busctl', 'noctalia', 'gio', 'cargo']:
        (commands / name).write_text(stub)
        (commands / name).chmod(0o755)
    home = root / 'home with "quotes" é $'
    home.mkdir()
    runtime = root / 'runtime'
    runtime.mkdir(mode=0o700)
    env = {k: v for k, v in os.environ.items() if not k.startswith('FERROSEEK_')}
    env.update(HOME=str(home), XDG_DATA_HOME=str(root / 'custom-data'),
               XDG_CONFIG_HOME=str(root / 'custom-config'), XDG_STATE_HOME=str(root / 'custom-state'),
               XDG_RUNTIME_DIR=str(runtime), DBUS_SESSION_BUS_ADDRESS='unix:path=' + str(runtime / 'no-bus'),
               XDG_CURRENT_DESKTOP='niri', PATH=str(commands), TEST_CALLS=str(calls),
               CODEX_HOME=str(home / '.codex'), CLAUDE_CONFIG_DIR=str(home / '.claude'))
    plugin = Path(env['XDG_DATA_HOME']) / 'noctalia/plugins/ferroseek'
    manifest = Path(env['XDG_STATE_HOME']) / 'ferroseek/installer/manifest.json'
    settings = Path(env['XDG_STATE_HOME']) / 'noctalia/settings.toml'
    settings.parent.mkdir(parents=True)
    settings.write_text('# retain user intent\n[plugins]\nenabled = ["other/plugin"]\n')
    original_settings = settings.read_bytes()

    def run(*args, expected=0, extra=None):
        options = [] if '--uninstall' in args else ['--no-build']
        result = subprocess.run([sys.executable, '-B', str(source / 'scripts/installer.py'), *options, *args],
                                env={**env, **(extra or {})}, input='', text=True, capture_output=True)
        assert result.returncode == expected, (args, result.stdout, result.stderr)
        return result

    def history():
        return [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []

    # A dry-run neither installs files nor mutates Noctalia/session state.
    preview = run('--noctalia', '--dry-run')
    assert str(plugin / 'plugin.toml') in preview.stdout
    assert not plugin.exists() and not manifest.exists()
    assert all(call[1:] == ['--version'] for call in history() if call[0] == 'noctalia')
    for version in ['noctalia v4.9.0', 'noctalia v5.0.0-beta.8', 'unknown']:
        run('--noctalia', extra={'TEST_VERSION': version}, expected=1)
    noctalia_stub = (commands / 'noctalia').read_bytes()
    (commands / 'noctalia').unlink()
    run('--noctalia', expected=1)
    (commands / 'noctalia').write_bytes(noctalia_stub)
    (commands / 'noctalia').chmod(0o755)
    gio = (commands / 'gio').read_bytes()
    (commands / 'gio').unlink()
    run('--noctalia', expected=1)
    (commands / 'gio').write_bytes(gio)
    (commands / 'gio').chmod(0o755)
    assert not manifest.exists()
    run('--noctalia', '--enable', extra={'TEST_OFFLINE': 'yes'}, expected=1)
    assert not manifest.exists()

    run('--noctalia')
    info = json.loads(manifest.read_text())
    assert info['components'] == ['core', 'noctalia']
    assert set(installer.NOCTALIA_FILES) <= set(info['files'])
    copied = tomllib.loads((plugin / 'plugin.toml').read_text())
    binary = next(s for s in copied['setting'] if s['key'] == 'binary')['default']
    assert binary == str(home / '.local/bin/ferroseek')
    assert not any(p.is_symlink() for p in plugin.rglob('*'))
    assert (plugin / 'NOCTALIA.md').exists()
    assert '../../../docs/' not in (plugin / 'README.md').read_text()
    assert ['noctalia', 'msg', 'plugins', 'enable', installer.NOCTALIA_ID] not in history()
    assert settings.read_bytes() == original_settings
    # Removing the checkout does not affect installed plugin or executable.
    moved = root / 'checkout-moved'
    source.rename(moved)
    try:
        assert subprocess.run([binary, '--help'], env=env, capture_output=True).returncode == 0
        assert (plugin / 'launcher.luau').read_bytes() == (moved / 'integrations/noctalia/ferroseek/launcher.luau').read_bytes()
    finally:
        moved.rename(source)

    # Upgrades preserve user settings/data; identical repeats are harmless.
    config = Path(env['XDG_CONFIG_HOME']) / 'ferroseek/config.toml'
    config.parent.mkdir(parents=True)
    config.write_text('# keep roots\nroots = []\n')
    index = Path(env['XDG_DATA_HOME']) / 'ferroseek/sentinel'
    index.parent.mkdir(parents=True)
    index.write_text('keep index')
    run('--noctalia', '--enable')
    assert ['systemctl', '--user', 'enable', '--now', 'ferroseek.service'] in history()
    assert ['noctalia', 'msg', 'plugins', 'enable', installer.NOCTALIA_ID] in history()
    run('--noctalia', '--enable', extra={'TEST_ENABLE_FAIL': 'yes'}, expected=1)
    assert (plugin / 'plugin.toml').exists(), 'post-install IPC errors retain a complete install'

    # A mid-upgrade failure restores all old plugin bytes and the ownership manifest.
    before = {Path(item['path']): Path(item['path']).read_bytes() for item in json.loads(manifest.read_text())['files'].values()}
    before[manifest] = manifest.read_bytes()
    source_manifest = source / 'integrations/noctalia/ferroseek/plugin.toml'
    source_manifest.write_text(source_manifest.read_text().replace('version = "0.1.1"', 'version = "0.1.2"'))
    source_launcher = source / 'integrations/noctalia/ferroseek/launcher.luau'
    source_launcher.write_bytes(source_launcher.read_bytes() + b'\n-- upgrade fixture\n')
    original_write = tx.atomic_write
    def fail_upgrade(path, data, mode):
        if path == plugin / 'launcher.luau' and data.endswith(b'-- upgrade fixture\n'):
            raise OSError('injected plugin upgrade failure')
        return original_write(path, data, mode)
    with patch.dict(os.environ, env, clear=True), patch.object(installer, 'ROOT', source), \
            patch.object(sys, 'argv', ['installer.py', '--noctalia', '--no-build']), \
            patch.object(tx, 'atomic_write', fail_upgrade):
        assert installer.main() == 1
    assert all(path.read_bytes() == raw for path, raw in before.items())
    run('--noctalia')
    run('--noctalia')
    assert (plugin / 'launcher.luau').read_bytes().endswith(b'-- upgrade fixture\n')
    assert config.read_text().startswith('# keep') and index.read_text() == 'keep index'
    assert settings.read_bytes() == original_settings

    # Local edits and unrelated/symlinked destinations are not overwritten.
    launcher = plugin / 'launcher.luau'
    owned = launcher.read_bytes()
    launcher.write_bytes(b'local customization')
    run('--noctalia', expected=1)
    run('--uninstall', '--noctalia', expected=1)
    launcher.write_bytes(owned)
    run('--uninstall', '--core', expected=1)
    run('--uninstall', '--noctalia', '--dry-run')
    assert launcher.exists()
    run('--uninstall', '--noctalia', extra={'TEST_DISABLE_FAIL': 'yes'}, expected=1)
    assert launcher.exists()
    run('--uninstall', '--noctalia')
    assert not (plugin / 'plugin.toml').exists() and Path(binary).exists()
    assert ['noctalia', 'msg', 'plugins', 'disable', installer.NOCTALIA_ID] in history()
    assert settings.read_bytes() == original_settings
    run('--uninstall', '--noctalia')
    (plugin / 'plugin.toml').write_text('unrelated plugin')
    run('--noctalia', expected=1)
    (plugin / 'plugin.toml').unlink()
    (plugin / 'plugin.toml').symlink_to(settings)
    run('--noctalia', expected=1)
    assert settings.read_bytes() == original_settings
    (plugin / 'plugin.toml').unlink()
    run('--noctalia', extra={'TEST_OFFLINE': 'yes'})
    run('--uninstall', '--noctalia', extra={'TEST_OFFLINE': 'yes'})
    assert Path(binary).exists() and index.read_text() == 'keep index'
    # Missing files are harmless, but a recorded component still needs its
    # native enabled entry disabled (another source may supply the same ID).
    run('--noctalia')
    for p in plugin.rglob('*'):
        if p.is_file():
            p.unlink()
    calls.write_text('')
    run('--uninstall', '--noctalia')
    assert ['noctalia', 'msg', 'plugins', 'disable', installer.NOCTALIA_ID] in history()

print('PASS: Noctalia dependencies, dry-run, copied install, activation, upgrades, rollback and partial removal')
