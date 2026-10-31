#!/usr/bin/env python3
"""Measure post-KRunner-session idle memory on a private D-Bus session."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

if 'FERROSEEK_IDLE_PRIVATE' not in os.environ:
    raise SystemExit(subprocess.call(['dbus-run-session', '--', sys.executable, *sys.argv],
                                    env=dict(os.environ, FERROSEEK_IDLE_PRIVATE='1')))
p = argparse.ArgumentParser()
p.add_argument('--binary', type=Path, required=True)
p.add_argument('--work', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
work = a.work.resolve()
env = dict(os.environ, FERROSEEK_CONFIG=str(work/'config.toml'), FERROSEEK_DATA_DIR=str(work/'data'))
child = subprocess.Popen([str(a.binary.resolve())], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
def call(method, *args, control=False):
    return subprocess.check_output(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
        '--object-path', '/io/github/ferroseek/FerroSeek' if control else '/runner', '--method',
        ('io.github.ferroseek.FerroSeek1.' if control else 'org.kde.krunner1.')+method, *args],
        stderr=subprocess.DEVNULL, text=True)
try:
    deadline = time.monotonic()+60
    while True:
        try:
            if "'state': 'idle'" in call('Status', control=True): break
        except subprocess.CalledProcessError: pass
        assert time.monotonic()<deadline, 'startup timed out'
        time.sleep(.05)
    assert 'file://' in call('Match', 'report')
    call('Teardown')
    time.sleep(12)  # query workers expire after 10 seconds
    status = Path(f'/proc/{child.pid}/status').read_text()
    rollup = Path(f'/proc/{child.pid}/smaps_rollup').read_text()
    def fields(text, names):
        return {k: v.strip() for line in text.splitlines() if ':' in line
                for k, v in [line.split(':', 1)] if k in names}
    a.output.write_text(json.dumps({
        'status':fields(status, {'VmRSS', 'VmHWM', 'RssAnon', 'RssFile', 'Threads'}),
        'smaps_rollup':fields(rollup, {'Rss', 'Pss', 'Anonymous', 'Private_Clean', 'Private_Dirty'})}, indent=2))
finally:
    child.send_signal(signal.SIGTERM)
    _, errors = child.communicate(timeout=10)
    assert child.returncode == 0, errors
print('PASS: post-session memory captured; daemon stopped')
