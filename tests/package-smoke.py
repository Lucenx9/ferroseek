#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Local archive integrity, component contents and toolchain-free installation."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

repo = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='ferroseek-package-') as tmp:
    root = Path(tmp)
    runtime = root / 'runtime'; runtime.mkdir(mode=0o700)
    commands = root / 'commands'; commands.mkdir()
    for name in ['systemctl', 'busctl', 'gdbus', 'cargo', 'noctalia', 'gio']:
        p = commands / name
        p.write_text('#!/bin/sh\n[ "$1" = --version ] && echo "noctalia v5.4.1"\nexit 0\n'
                     if name == 'noctalia' else '#!/bin/sh\nexit 1\n')
        p.chmod(0o755)
    clean_env = {k: v for k, v in os.environ.items() if not k.startswith('FERROSEEK_')}
    env = dict(clean_env, XDG_RUNTIME_DIR=str(runtime), SOURCE_DATE_EPOCH='0', XDG_CURRENT_DESKTOP='KDE',
               XDG_SESSION_TYPE='wayland', WAYLAND_DISPLAY='wayland-test',
               PATH=str(commands) + ':' + os.environ['PATH'],
               DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-package-test')
    for component in ['launcher', 'core', 'mcp', 'krunner', 'noctalia', 'full']:
        output = root / component
        args = ['python3', str(repo / 'scripts/package.py'), '--no-build', '--components', component, '--output', str(output)]
        if component == 'launcher':
            args[3:5] = []  # The default archive must contain the main product.
        r = subprocess.run(args, env=env, capture_output=True, text=True)
        assert r.returncode == 0, r.stderr
        archive = next(output.glob('*.tar.gz'))
        checksum = archive.with_name(archive.name + '.sha256').read_text().split()[0]
        assert hashlib.sha256(archive.read_bytes()).hexdigest() == checksum
        original = archive.read_bytes()
        assert subprocess.run(args, env=env, capture_output=True).returncode != 0
        assert archive.read_bytes() == original
        extract = root / (component + '-extracted'); extract.mkdir()
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                assert member.isfile() and not Path(member.name).is_absolute() and '..' not in Path(member.name).parts
                assert member.uid == 0 and member.gid == 0 and member.mtime == 0
            tar.extractall(extract, filter='data')
        payload = next(extract.iterdir())
        metadata = json.loads((payload / 'package.json').read_text())
        assert (payload / 'bin/ferroseek-mcp').exists() == (component in ['mcp', 'full'])
        assert (payload / 'bin/ferroseek-launcher').exists() == (component in ['launcher', 'full'])
        assert (payload / 'integrations/noctalia/ferroseek/plugin.toml').exists() == (component in ['noctalia', 'full'])
        for name, expected in metadata['sha256'].items():
            assert hashlib.sha256((payload / name).read_bytes()).hexdigest() == expected
        home = root / ('home=' + component + ' é $%'); home.mkdir()
        isolated = dict(env, HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
                        XDG_DATA_HOME=str(home / '.local/share'), XDG_STATE_HOME=str(home / '.local/state'),
                        CODEX_HOME=str(home / '.codex'), CLAUDE_CONFIG_DIR=str(home / '.claude'))
        script = payload / 'scripts/install.sh'
        before_payload = sorted(str(p.relative_to(payload)) for p in payload.rglob('*'))
        preview = subprocess.run([str(script), '--' + component, '--dry-run'], env=isolated, capture_output=True, text=True)
        assert preview.returncode == 0, preview.stderr
        assert not list(home.iterdir())
        assert sorted(str(p.relative_to(payload)) for p in payload.rglob('*')) == before_payload
        r = subprocess.run([str(script), '--' + component], env=isolated, capture_output=True, text=True)
        assert r.returncode == 0, (r.stdout, r.stderr)
        assert (home / '.local/bin/ferroseek').exists()
        assert (home / '.local/bin/ferroseek-launcher').exists() == (component in ['launcher', 'full'])
        assert (home / '.local/share/krunner/dbusplugins/ferroseek.desktop').exists() == (component in ['krunner', 'full'])
        assert (home / '.local/bin/ferroseek-mcp').exists() == (component in ['mcp', 'full'])
        assert (home / '.local/share/noctalia/plugins/ferroseek/plugin.toml').exists() == (component in ['noctalia', 'full'])
        if component == 'core':
            assert subprocess.run([str(script), '--launcher'], env=isolated, capture_output=True).returncode != 0
            assert subprocess.run([str(script), '--mcp'], env=isolated, capture_output=True).returncode != 0
            assert subprocess.run([str(script), '--noctalia'], env=isolated, capture_output=True).returncode != 0
            duplicate = root / 'repeat'
            args[-1] = str(duplicate)
            assert subprocess.run(args, env=env, capture_output=True).returncode == 0
            assert next(duplicate.glob('*.tar.gz')).read_bytes() == original
        # Corruption is rejected before any installation file changes.
        binary = payload / 'bin/ferroseek'
        binary.write_bytes(binary.read_bytes() + b'corruption')
        installed = (home / '.local/bin/ferroseek').read_bytes()
        assert subprocess.run([str(script), '--core'], env=isolated, capture_output=True).returncode != 0
        assert (home / '.local/bin/ferroseek').read_bytes() == installed
        r = subprocess.run([str(payload / 'scripts/uninstall.sh')], env=isolated, capture_output=True, text=True)
        assert r.returncode == 0, r.stderr
        assert not (home / '.local/bin/ferroseek').exists()
        assert not (home / '.local/bin/ferroseek-launcher').exists()
print('PASS: six native component archives, checksums, reproducibility, corruption rejection and installation without Cargo')
