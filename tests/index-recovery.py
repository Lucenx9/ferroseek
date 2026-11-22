#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check automatic daemon restart recovery against the independent filesystem oracle."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import random
import signal
import subprocess
import sys
import tempfile
import time

SCRIPT = Path(__file__).resolve()
ORACLE = SCRIPT.parents[1] / 'scripts/verify-index.py'
DEFAULT_SEED = 73129


def private_session(binaries, seed):
    with tempfile.TemporaryDirectory(prefix='ferroseek-recovery-', dir='/tmp') as tmp:
        work = Path(tmp)
        # Give the bus a known socket and no desktop activation directories.
        bus = work / 'bus.conf'
        bus.write_text(f"""<busconfig>
  <type>session</type><listen>unix:path={work}/bus</listen><auth>EXTERNAL</auth>
  <policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy>
</busconfig>""")
        env = dict(PATH=os.environ['PATH'], HOME=tmp, XDG_RUNTIME_DIR=tmp,
                   FERROSEEK_CONFIG=str(work / 'config.toml'), FERROSEEK_DATA_DIR=str(work / 'data'))
        return subprocess.call(['dbus-run-session', f'--config-file={bus}', '--', sys.executable,
                                SCRIPT, binaries, '--seed', str(seed), '--private-session'], cwd=work, env=env)


@contextmanager
def running_daemon(binaries):
    daemon = subprocess.Popen([binaries / 'ferroseekd'])
    try:
        wait_for_idle(binaries / 'ferroseek', daemon)
        yield daemon
    finally:
        if daemon.poll() is None:
            daemon.kill()
        daemon.wait(timeout=5)


def wait_for_idle(cli, daemon):
    deadline = time.monotonic() + 20
    status = ''
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise RuntimeError(f'daemon exited during startup with {daemon.returncode}')
        status = subprocess.run([cli, 'status'], capture_output=True, text=True, timeout=3).stdout
        lines = {line.strip() for line in status.splitlines()}
        if {f'daemon: running (pid {daemon.pid})', 'state: idle', 'queued_dirs: 0'} <= lines:
            return
        time.sleep(0.05)
    raise RuntimeError(f'timed out waiting for daemon {daemon.pid} to finish startup\n{status}')


def mutate(folders, rng, phase):
    # Sorting the source pool keeps the workload independent of directory enumeration order.
    sources = sorted(path for folder in folders for path in folder.glob('*.txt'))
    rng.shuffle(sources)
    for step in range(8):
        created_in = rng.choice(folders)
        (created_in / f'{phase}-created-{step}.txt').write_text(f'{phase}-{step}')
        renamed, moved, deleted = sources[3 * step:3 * step + 3]
        renamed.rename(renamed.with_name(f'{phase}-renamed-{step}.txt'))
        dest = rng.choice([folder for folder in folders if folder != moved.parent])
        moved.rename(dest / moved.name)
        deleted.unlink()


def compare(work, phase, expect_drift=False):
    result = subprocess.run([sys.executable, ORACLE, '--config', work / 'effective.toml',
                             '--database', work / 'data/index.sqlite'],
                            capture_output=True, text=True, timeout=10)
    if result.returncode != int(expect_drift):
        raise RuntimeError(f'{phase}: oracle exited {result.returncode}\n{result.stdout}{result.stderr}')
    print(f'{phase}: {result.stdout.strip()}', flush=True)


def exercise(work, binaries, seed):
    if os.environ.get('DBUS_SESSION_BUS_ADDRESS', '').split(',')[0] != f'unix:path={work}/bus':
        raise RuntimeError('expected the recovery test session bus')
    rng = random.Random(seed)
    root = work / 'root'
    folders = [root / name for name in ('letters', 'notes', 'receipts', 'reports')]
    for i, folder in enumerate(folders):
        folder.mkdir(parents=True)
        for j in range(12):
            (folder / f'file-{i}-{j}.txt').write_text(f'{i}-{j}')
    (work / 'config.toml').write_text(f'roots = [{json.dumps(str(root))}]\nreconcile_interval_minutes = 0\n')
    (work / 'effective.toml').write_bytes(subprocess.check_output([binaries / 'ferroseek', 'config'], timeout=5))

    with running_daemon(binaries) as daemon:
        compare(work, 'initial scan')
        mutate(folders, rng, 'live')
        daemon.kill()
        if daemon.wait(timeout=5) != -signal.SIGKILL:
            raise RuntimeError('daemon exited before the planned interruption')

    mutate(folders, rng, 'offline')
    folder = rng.choice(folders)
    folder.rename(root / 'relocated')
    compare(work, 'while stopped', expect_drift=True)
    with running_daemon(binaries):
        compare(work, 'after restart')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binaries', type=Path, help='directory containing ferroseek and ferroseekd')
    parser.add_argument('--seed', type=int, default=DEFAULT_SEED)
    parser.add_argument('--private-session', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    binaries = args.binaries.resolve()
    if args.private_session:
        exercise(Path.cwd(), binaries, args.seed)
        return 0
    print(f'Index recovery seed {args.seed}', flush=True)
    return private_session(binaries, args.seed)


if __name__ == '__main__':
    sys.exit(main())
