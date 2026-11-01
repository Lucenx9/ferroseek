#!/usr/bin/env python3
"""Independent oracle must detect missing, stale and metadata discrepancies."""
import argparse
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import sys
import tempfile
import tomllib

spec = importlib.util.spec_from_file_location('verify_index', 'scripts/verify-index.py')
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


def schema_compatibility():
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        (work/'home').mkdir()
        config = work/'effective.toml'
        config.write_text('roots = []\nexclude_paths = []\nexclude_names = []\n'
                          'stay_on_filesystem = false\nmax_depth = 1\n'
                          'respect_cachedir_tag = false\nindex_hidden = false\n')
        env = dict(os.environ, HOME=str(work/'home'),
                   DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-test-bus')
        for application_id, version in ((0x46534B31, 1), (0x46534B31, 2),
                                        (0x46534B31, 3), (0, 2)):
            database = work/f'index-{application_id}-{version}.sqlite'
            with closing(sqlite3.connect(database)) as db:
                db.executescript(f'PRAGMA application_id={application_id};\n'
                                 f'PRAGMA user_version={version};\n'
                                 'CREATE TABLE entries(id INTEGER PRIMARY KEY, parent INTEGER, '
                                 'name BLOB, key TEXT, kind INTEGER, size INTEGER, mtime INTEGER, ino INTEGER);')
                db.executemany('INSERT INTO entries VALUES (?,?,?,?,?,?,?,?)', (
                    (1, None, b'/synthetic', 'unused', 1, 0, 0, 1),
                    (2, 1, b'raw-\xff.txt', 'unused', 0, 3, 5, 2),
                ))
                db.commit()
            before = database.read_bytes()
            if application_id == 0x46534B31 and version in (1, 2):
                assert verify.indexed(database) == {b'/synthetic': (1,), b'/synthetic/raw-\xff.txt': (0, 3, 5)}
            else:
                try:
                    verify.indexed(database)
                except ValueError:
                    pass
                else:
                    raise AssertionError(f'accepted unsupported identity: {application_id}, {version}')
                result = subprocess.run([sys.executable, '-O', 'scripts/verify-index.py', '--config', str(config),
                                         '--database', str(database)], env=env, capture_output=True, text=True, timeout=5)
                assert result.returncode == 2, result
                assert json.loads(result.stdout)['error'] == 'comparison incomplete', result.stdout
            assert database.read_bytes() == before, 'verification must not modify the database'


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--schema-only', action='store_true', help='test schema compatibility without built binaries')
args = parser.parse_args()
schema_compatibility()
if args.schema_only:
    print('PASS: schemas 1 and 2 accepted; unknown versions and foreign application IDs rejected, including python -O')
    raise SystemExit(0)

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

failures = []
for case in ('depth', 'regular_tag', 'invalid_tag', 'symlink_tag', 'dangling_tag', 'fifo_tag', 'directory_tag'):
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        root = work/'root'
        root.mkdir()
        report = root/'nested/report.txt' if case == 'depth' else root/'report.txt'
        report.parent.mkdir(exist_ok=True)
        report.write_text('report')
        if case == 'depth':
            (root/'nested/beyond').mkdir()
            (root/'nested/beyond/too-deep.txt').write_text('excluded by depth')
        tag = root/'CACHEDIR.TAG'
        signature = b'Signature: 8a477f597d28d172789f06886806bc55'
        if case == 'regular_tag':
            tag.write_bytes(signature)
        elif case == 'invalid_tag':
            tag.write_bytes(b'not a cache directory tag')
        elif case == 'symlink_tag':
            (work/'tag').write_bytes(signature)
            tag.symlink_to(work/'tag')
        elif case == 'dangling_tag':
            tag.symlink_to(work/'missing')
        elif case == 'fifo_tag':
            os.mkfifo(tag)
        elif case == 'directory_tag':
            tag.mkdir()
            (tag/'inside.txt').write_text('still indexed')
        config = work/'config.toml'
        config.write_text(f'roots = ["{root}"]\nmax_depth = 1\n')
        env = dict(os.environ, FERROSEEK_CONFIG=str(config), FERROSEEK_DATA_DIR=str(work/'data'),
                   DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-test-bus')
        effective = work/'effective.toml'
        effective.write_bytes(subprocess.check_output([binary, 'config'], env=env))
        subprocess.run([binary, 'index', '--full'], env=env, check=True, capture_output=True)
        try:
            result = subprocess.run(['python3', 'scripts/verify-index.py', '--config', str(effective),
                                     '--database', str(work/'data/index.sqlite')],
                                    capture_output=True, text=True, timeout=5)
        except subprocess.TimeoutExpired:
            failures.append(f'{case}: verifier blocked for more than 5 seconds')
            continue
        comparison = json.loads(result.stdout)
        if (result.returncode != 0 or not comparison['stable']
                or any(comparison[k] for k in ('missing', 'stale', 'metadata_mismatches'))):
            failures.append(f'{case}: {comparison}')
assert not failures, '\n'.join(failures)
print('PASS: independent oracle detects drift and handles exclusions, raw names, depth limits and cache tag types')
