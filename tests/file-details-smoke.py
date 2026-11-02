#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Current file metadata through the daemon's private test bus."""
import ast
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import quote_from_bytes


def main():
    if os.environ.get('FERROSEEK_SMOKE_ISOLATED') != '1':
        raise SystemExit('Run through tests/dbus-smoke.sh on its private bus')
    root = Path(sys.argv[1])
    assert Path(os.environ['HOME']).is_relative_to(root)

    def call(method, *args, ok=True, control=False):
        interface = 'FerroSeek1' if control else 'Search1'
        path = 'FerroSeek' if control else 'Search'
        result = subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                 '--object-path', f'/io/github/ferroseek/{path}', '--method',
                                 f'io.github.ferroseek.{interface}.{method}', *args],
                                text=True, capture_output=True, timeout=10)
        assert (result.returncode == 0) == ok, result.stderr or result.stdout
        if not ok:
            return result.stderr
        if method == 'Search':
            return result.stdout
        return ast.literal_eval(re.sub(r'\b(?:u?int64)\s+', '', result.stdout))

    def uri(path):
        return 'file://' + quote_from_bytes(os.fsencode(path), safe='/')

    pdf = root / 'root/Documents/Quarterly Report.pdf'
    stat = pdf.stat()
    assert call('FileDetails', uri(pdf)) == ('application/pdf', stat.st_size, stat.st_mtime_ns // 10**9)
    # Pausing indexing proves these are current attributes, not stored index values.
    call('Pause', control=True)
    saved = pdf.read_bytes()
    try:
        pdf.write_bytes(b'updated metadata fixture')
        pdf.chmod(0)
        os.utime(pdf, ns=(-10**9, -10**9))
        assert call('FileDetails', uri(pdf)) == ('application/pdf', 24, -1)
    finally:
        pdf.chmod(stat.st_mode)
        pdf.write_bytes(saved)
        os.utime(pdf, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        call('Resume', control=True)

    raw = os.fsencode(root / 'root/Documents') + b'/details-raw-\xff.pdf'
    unknown = root / 'root/Documents/details-unknown.ferrodetails'
    link = root / 'root/Documents/details-link.pdf'
    for path in (raw, unknown):
        with open(path, 'wb') as file:
            file.write(b'abc')
    link.symlink_to(pdf)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        found = call('Search', 'details')
        if uri(raw) in found and uri(unknown) in found:
            break
        time.sleep(0.03)
    else:
        raise AssertionError('metadata fixtures were not indexed')
    assert call('FileDetails', uri(raw))[:2] == ('application/pdf', 3)
    assert call('FileDetails', uri(unknown))[:2] == ('', 3)
    for invalid in ('not-a-url', 'file:///bad%00path', 'file:///etc/passwd', uri(root / 'root/Documents'), uri(link)):
        call('FileDetails', invalid, ok=False)
    unknown.unlink()
    call('FileDetails', uri(unknown), ok=False)
    print('File details: live attributes, raw paths, unknown types and invalid targets passed')


if __name__ == '__main__':
    main()
