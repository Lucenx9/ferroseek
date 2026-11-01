#!/usr/bin/env python3
"""Read-only, independent filesystem/index comparison; output contains counts only.

Requires a complete effective TOML config (ferroseek config > effective.toml).
Exit 0: match; 1: mismatch on stable tree; 2: incomplete/active filesystem.
No repair, reconcile, daemon activation, or user filename logging.
"""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import time
import tomllib


def normalized(path):
    return os.fsencode(os.path.abspath(os.path.expanduser(path)))


def beneath(path, parent):
    return path == parent or path.startswith(parent.rstrip(b'/') + b'/')


def has_cachedir_tag(path):
    flags = os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_NOCTTY
    try:
        with os.fdopen(os.open(os.path.join(path, b'CACHEDIR.TAG'), flags), 'rb') as tag:
            return (stat.S_ISREG(os.fstat(tag.fileno()).st_mode)
                    and tag.read(43) == b'Signature: 8a477f597d28d172789f06886806bc55')
    except OSError:
        return False


def filesystem(config, data_dir):
    roots = sorted(set(normalized(p) for p in config['roots']))
    roots = [p for p in roots if not any(p != r and beneath(p, r) for r in roots)]
    excluded = [normalized(p) for p in config['exclude_paths']] + [os.fsencode(data_dir)]
    patterns = [re.compile(b'^' + b''.join(b'.*' if c == 42 else b'.' if c == 63
                else re.escape(bytes([c])) for c in os.fsencode(p)) + b'$', re.DOTALL)
                for p in config['exclude_names']]
    mounts = set()
    if config['stay_on_filesystem']:
        for line in Path('/proc/self/mountinfo').read_bytes().splitlines():
            mounts.add(re.sub(rb'\\([0-7]{3})', lambda m: bytes([int(m[1], 8)]), line.split()[4]))
    found = {}
    errors = 0
    stack = [(r, 0, None) for r in roots]
    while stack:
        path, depth, parent_dev = stack.pop()
        try:
            md = os.lstat(path)
            kind = 1 if stat.S_ISDIR(md.st_mode) else 0 if stat.S_ISREG(md.st_mode) else 2 if stat.S_ISLNK(md.st_mode) else 3
            found[path] = (kind,) if kind == 1 else (kind, md.st_size, md.st_mtime_ns)
            if kind != 1 or depth > config['max_depth']:
                continue
            if config['stay_on_filesystem'] and parent_dev is not None and md.st_dev != parent_dev and path in mounts:
                continue
            if config['respect_cachedir_tag'] and has_cachedir_tag(path):
                continue
            with os.scandir(path) as listing:
                for entry in listing:
                    name = entry.name
                    if ((not config['index_hidden'] and name.startswith(b'.'))
                            or any(p.fullmatch(name) for p in patterns)
                            or any(beneath(entry.path, p) for p in excluded)):
                        continue
                    stack.append((entry.path, depth + 1, md.st_dev))
        except OSError:
            errors += 1
    return found, errors


def indexed(database):
    uri = database.resolve().as_uri() + '?mode=ro'
    with closing(sqlite3.connect(uri, uri=True, timeout=5)) as db:
        db.execute('PRAGMA query_only=ON')
        db.execute('BEGIN')
        if db.execute('PRAGMA application_id').fetchone()[0] != 0x46534B31:
            raise ValueError('not a FerroSeek index')
        # Both versions have the same entry metadata; matching keys are not read.
        if db.execute('PRAGMA user_version').fetchone()[0] not in (1, 2):
            raise ValueError('unsupported index schema version')
        rows = db.execute('SELECT id,parent,name,kind,size,mtime FROM entries').fetchall()
    remaining = {row[0]: row[1:] for row in rows}
    paths = {}
    result = {}
    while remaining:
        ready = [i for i, row in remaining.items() if row[0] is None or row[0] in paths]
        if not ready:
            raise ValueError('invalid parent tree')
        for i in ready:
            parent, name, kind, size, mtime = remaining.pop(i)
            path = bytes(name) if parent is None else os.path.join(paths[parent], bytes(name))
            paths[i] = path
            result[path] = (kind,) if kind == 1 else (kind, size, mtime)
    return result


def compare(config, database):
    before, errors1 = filesystem(config, database.parent)
    actual = indexed(database)
    after, errors2 = filesystem(config, database.parent)
    stable = before == after and errors1 + errors2 == 0
    missing = len(after.keys() - actual.keys())
    stale = len(actual.keys() - after.keys())
    metadata = sum(after[p] != actual[p] for p in after.keys() & actual.keys())
    return dict(timestamp=int(time.time()), stable=stable, filesystem_entries=len(after),
                indexed_entries=len(actual), missing=missing, stale=stale, metadata_mismatches=metadata,
                filesystem_errors=errors1 + errors2)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--database', type=Path, required=True)
    a = p.parse_args()
    try:
        with a.config.open('rb') as f:
            config = tomllib.load(f)
        result = compare(config, a.database)
        print(json.dumps(result))
        return 2 if not result['stable'] else int(any(result[k] for k in ('missing', 'stale', 'metadata_mismatches')))
    except (OSError, sqlite3.Error, ValueError, KeyError):
        print(json.dumps(dict(timestamp=int(time.time()), stable=False, error='comparison incomplete')))
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
