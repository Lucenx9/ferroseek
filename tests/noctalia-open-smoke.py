#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check GIO handoff with a fake app and isolated MIME/XDG configuration."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def wait_for(path, seconds=3):
    deadline = time.monotonic() + seconds
    while not path.exists():
        assert time.monotonic() < deadline, f"fake app did not create {path.name}"
        time.sleep(0.02)


def main():
    assert shutil.which('gio'), 'install GLib gio before running this test'
    with tempfile.TemporaryDirectory(prefix='ferroseek-noctalia-open-') as temp:
        root = Path(temp)
        config, data = root / 'config', root / 'data'
        config.mkdir()
        (data / 'applications').mkdir(parents=True)
        started, stopped, done = root / 'started.json', root / 'stopped', root / 'done'
        helper = root / 'fake_app.py'
        helper.write_text('''import json, os, pathlib, sys, time
root = pathlib.Path(__file__).parent
(root / "started.json").write_text(json.dumps({"pid": os.getpid(), "args": sys.argv[1:]}))
deadline = time.monotonic() + 20
while not (root / "done").exists() and time.monotonic() < deadline:
    time.sleep(0.02)
(root / "stopped").touch()
''')
        desktop = data / 'applications/ferroseek-opener-test.desktop'
        desktop.write_text(
            '[Desktop Entry]\nType=Application\nName=FerroSeek opener test\n'
            f'Exec="{sys.executable}" "{helper}" %f\n'
            'Terminal=false\nDBusActivatable=false\nMimeType=text/plain;\n'
        )
        (config / 'mimeapps.list').write_text(
            '[Default Applications]\ntext/plain=ferroseek-opener-test.desktop\n'
        )
        env = {**os.environ, 'XDG_CONFIG_HOME': str(config), 'XDG_DATA_HOME': str(data),
               'XDG_CONFIG_DIRS': str(config), 'XDG_DATA_DIRS': '/usr/share',
               'DBUS_SESSION_BUS_ADDRESS': 'unix:path=' + str(root / 'no-bus')}
        path = root / 'report "caffè"\n$(touch forbidden).txt'
        path.write_text('A plain text test document.\n')
        try:
            # Wait for the launcher process, not EOF on pipes inherited by its
            # application. Noctalia's process runner also returns on child exit.
            opener = subprocess.Popen(['gio', 'open', str(path)], env=env, cwd=root,
                                      stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL)
            try:
                assert opener.wait(timeout=3) == 0, 'GIO handoff failed'
            finally:
                if opener.poll() is None:
                    opener.kill()
                    opener.wait()
            wait_for(started)
            app = json.loads(started.read_text())
            assert app['args'] == [str(path)], 'file path was changed or interpreted'
            # A GUI may stay open indefinitely. This exceeds Noctalia's five-
            # second capture timeout, while the handoff has already succeeded.
            time.sleep(5.2)
            os.kill(app['pid'], 0)
            assert not stopped.exists(), 'app lifetime must be independent of handoff'
            assert not (root / 'forbidden').exists()
            missing = subprocess.run(['gio', 'open', str(root / 'missing.txt')], env=env, cwd=root,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, timeout=3)
            assert missing.returncode != 0, 'real opening errors must remain observable'
        finally:
            done.touch()
            if started.exists():
                wait_for(stopped)
    print('PASS: GIO hands off literal paths, leaves the app alive and reports launch errors')


if __name__ == '__main__':
    main()
