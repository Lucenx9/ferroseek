#!/usr/bin/env python3
"""Installer regression tests: temporary HOME/XDG, stubbed desktop commands."""
import os
from pathlib import Path
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='ferroseek-install-') as tmp:
    root = Path(tmp)
    home = root / 'home'
    home.mkdir()
    commands = root / 'commands'
    commands.mkdir()
    log = root / 'calls'
    for name in ['systemctl', 'busctl']:
        p = commands / name
        p.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_CALLS"\nexit 0\n')
        p.chmod(0o755)
    env = dict(os.environ, HOME=str(home), XDG_DATA_HOME='relative-data', XDG_CONFIG_HOME='relative-config',
               PATH=str(commands) + ':' + os.environ['PATH'], TEST_CALLS=str(log))
    cfg = root / 'config.toml'
    cfg.write_text('roots = []\n')
    def run(script, *args, expected=0, custom=None):
        r = subprocess.run([str(repo / 'scripts' / script), *args], env=custom or env,
                           cwd=repo, capture_output=True, text=True)
        assert r.returncode == expected, (r.returncode, r.stdout, r.stderr)
    for _ in range(2):
        run('install.sh', '--no-build', '--config', str(cfg))
    installed = home / '.local/bin/ferroseekd'
    assert installed.read_bytes() == (repo / 'target/release/ferroseekd').read_bytes()
    config = home / '.config/ferroseek/config.toml'
    config.write_text('# preserve custom config\nroots = []\n')
    run('install.sh', '--no-build', '--config', str(cfg))
    assert config.read_text().startswith('# preserve')
    data = home / '.local/share/ferroseek'
    data.mkdir()
    (data / 'sentinel').write_text('preserve')
    for _ in range(2):
        run('uninstall.sh')
    assert not installed.exists()
    assert config.exists() and (data / 'sentinel').exists()
    assert log.read_text().count('ReloadConfig') == 5
    run('uninstall.sh', '--unexpected', expected=2)
    run('install.sh', '--config', expected=2)
    for name in ['home with space', 'home&bad', 'home%bad']:
        bad = root / name
        bad.mkdir()
        run('install.sh', '--no-build', expected=2, custom=dict(env, HOME=str(bad)))
        assert list(bad.iterdir()) == []
print('PASS: repeat installation/removal, retained data, XDG fallback, invalid arguments and paths')
