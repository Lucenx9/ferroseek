# SPDX-License-Identifier: GPL-3.0-or-later
"""Private, durable undo journal for installer-owned file replacements.

No recursive removal of destinations. An interrupted transaction is undone on
next invocation, but never over a file changed by another writer meanwhile.
"""
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import tempfile


class InstallError(Exception):
    pass


def digest(data):
    return hashlib.sha256(data).hexdigest() if data is not None else None


def safe_path(path):
    if not path.is_absolute() or any(ord(c) < 32 or ord(c) == 127 for c in str(path)):
        raise InstallError(f'unsupported control character or relative path: {path!s}')
    for part in [path, *path.parents]:
        if part.is_symlink():
            raise InstallError(f'refusing symlink destination or ancestor: {part}')
    if path.exists() and not path.is_file():
        raise InstallError(f'expected a regular file: {path}')


def read(path):
    safe_path(path)
    return path.read_bytes() if path.exists() else None


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_write(path, data, mode):
    safe_path(path)
    fd, name = tempfile.mkstemp(prefix='.ferroseek-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(data)
            os.fchmod(f.fileno(), mode)
            f.flush()
            os.fsync(f.fileno())
        os.replace(name, path)
        sync_dir(path.parent)
    finally:
        if os.path.exists(name):
            os.unlink(name)


class Transaction:
    def __init__(self, state, targets):
        self.state = state
        self.targets = targets
        self.journal = state / 'transaction.json'
        self.backups = state / 'undo'

    @contextlib.contextmanager
    def locked(self):
        safe_path(self.state / 'lock')
        self.state.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.state.stat().st_uid != os.getuid() or stat.S_IMODE(self.state.stat().st_mode) & 0o077:
            raise InstallError(f'installer state must be owned by you and mode 0700: {self.state}')
        with (self.state / 'lock').open('a') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise InstallError('another FerroSeek installer is running') from None
            self.recover()
            yield

    def recover(self):
        raw = read(self.journal)
        if raw is None:
            return
        journal = json.loads(raw)
        # Paths in the journal cannot redirect recovery to arbitrary files.
        for item in journal['files']:
            if item['id'] not in self.targets or str(self.targets[item['id']]) != item['path']:
                raise InstallError('pending transaction needs the original HOME/XDG/agent environment')
        if not journal.get('committed'):
            for item in journal['files']:
                dest = self.targets[item['id']]
                current = (digest(read(dest)), stat.S_IMODE(dest.stat().st_mode) if dest.exists() else None)
                if current not in ((item['before'], item['mode']), (item['after'], item['after_mode'])):
                    raise InstallError('rollback conflict: a destination changed externally; undo journal retained')
            for i, item in reversed(list(enumerate(journal['files']))):
                dest = self.targets[item['id']]
                current = (digest(read(dest)), stat.S_IMODE(dest.stat().st_mode) if dest.exists() else None)
                if current == (item['before'], item['mode']):
                    continue
                if current != (item['after'], item['after_mode']):
                    raise InstallError('rollback conflict during recovery; undo journal retained')
                if item['before'] is None:
                    dest.unlink(missing_ok=True)
                    sync_dir(dest.parent)
                else:
                    previous = read(self.backups / str(i))
                    if digest(previous) != item['before']:
                        raise InstallError('damaged undo backup; refusing rollback')
                    atomic_write(dest, previous, item['mode'])
            print('Recovered the previous interrupted installation.')
        self.journal.unlink()
        sync_dir(self.state)
        # Only our private backups; not installation or user data directories.
        if self.backups.exists():
            if self.backups.is_symlink():
                raise InstallError('unsafe undo directory')
            shutil.rmtree(self.backups)

    def apply(self, changes):
        # id -> (expected previous bytes, replacement bytes or None, mode)
        for key, (old, _, _) in changes.items():
            if read(self.targets[key]) != old:
                raise InstallError(f'destination changed during planning: {self.targets[key]}')
        if not changes:
            return
        if self.backups.exists():
            if self.backups.is_symlink():
                raise InstallError('unsafe undo directory')
            shutil.rmtree(self.backups)
        self.backups.mkdir(mode=0o700)
        files = []
        for i, (key, (old, new, new_mode)) in enumerate(changes.items()):
            dest = self.targets[key]
            mode = stat.S_IMODE(dest.stat().st_mode) if old is not None else None
            if old is not None:
                atomic_write(self.backups / str(i), old, 0o600)
            files.append(dict(id=key, path=str(dest), before=digest(old), after=digest(new), mode=mode, after_mode=new_mode if new is not None else None))
        journal = dict(files=files, committed=False)
        atomic_write(self.journal, json.dumps(journal).encode(), 0o600)
        try:
            for i, (key, (old, new, mode)) in enumerate(changes.items()):
                dest = self.targets[key]
                safe_path(dest)
                current_mode = stat.S_IMODE(dest.stat().st_mode) if dest.exists() else None
                if read(dest) != old or current_mode != files[i]['mode']:
                    raise InstallError(f'destination changed during installation: {dest}')
                dest.parent.mkdir(parents=True, exist_ok=True)
                if new is None:
                    dest.unlink(missing_ok=True)
                    sync_dir(dest.parent)
                else:
                    atomic_write(dest, new, mode)
            journal['committed'] = True
            atomic_write(self.journal, json.dumps(journal).encode(), 0o600)
        except BaseException:
            self.recover()
            raise
        self.recover()
