#!/usr/bin/env python3
"""Independent oracle must detect missing, stale and metadata discrepancies."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

spec = importlib.util.spec_from_file_location('verify_index', 'scripts/verify-index.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
binary = Path('target/release/ferroseek').resolve()
with tempfile.TemporaryDirectory() as tmp:
    work = Path(tmp)
    root = work/'root'
    root.mkdir()
    (root/'one.txt').write_text('one')
    (root/'gone.txt').write_text('gone')
    (root/'link').symlink_to('one.txt')
    (root/'.hidden').write_text('hidden')
    (root/'skip.tmp').write_text('excluded')
    raw = os.fsencode(root) + b'/raw-\xff.txt'
    with open(raw, 'wb') as f: f.write(b'raw')
    config = work/'config.toml'
    config.write_text(f'roots = ["{root}"]\nexclude_names = ["*.tmp"]\n')
    env = dict(os.environ, FERROSEEK_CONFIG=str(config), FERROSEEK_DATA_DIR=str(work/'data'),
               DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-test-bus')
    effective = subprocess.check_output([binary, 'config'], env=env, text=True)
    cfg = tomllib.loads(effective)
    subprocess.run([binary, 'index'], env=env, check=True, capture_output=True)
    database = work/'data/index.sqlite'
    first = verify.compare(cfg, database)
    assert first['stable'] and first['missing'] == first['stale'] == first['metadata_mismatches'] == 0, first
    (root/'gone.txt').unlink()
    (root/'new.txt').write_text('new')
    (root/'one.txt').write_text('changed contents')
    second = verify.compare(cfg, database)
    assert second['stable'] and second['missing'] == second['stale'] == second['metadata_mismatches'] == 1, second
    subprocess.run([binary, 'index', '--full'], env=env, check=True, capture_output=True)
    final = verify.compare(cfg, database)
    assert final['stable'] and final['missing'] == final['stale'] == final['metadata_mismatches'] == 0, final
print('PASS: independent filesystem oracle detects drift and handles exclusions, symlinks and raw names')
