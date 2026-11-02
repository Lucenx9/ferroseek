#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Folder-scoped index queries through the private daemon bus."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from urllib.parse import quote_from_bytes


def main():
    root = Path(sys.argv[1])
    assert os.environ.get('FERROSEEK_SMOKE_ISOLATED') == '1'
    assert Path(os.environ['HOME']).is_relative_to(root)

    def uri(path):
        return 'file://' + quote_from_bytes(os.fsencode(path), safe='/')

    def call(method, *args, ok=True):
        result = subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                 '--object-path', '/io/github/ferroseek/Search', '--method',
                                 f'io.github.ferroseek.Search1.{method}', *args],
                                text=True, capture_output=True, timeout=10)
        assert (result.returncode == 0) == ok, result.stderr or result.stdout
        return set(re.findall(r"'(file://[^']*)'", result.stdout))

    documents = root / 'root/Documents'
    assert uri(documents / 'notes-2024.md') in call('ListDirectory', uri(documents))
    assert uri(documents / 'Quarterly Report.pdf') in call('SearchWithin', uri(documents), 'report')
    folder = documents / 'Scoped folder è:2026'
    sibling = documents / 'Scoped folder è:2026-archive'
    (folder / 'Nested').mkdir(parents=True)
    sibling.mkdir()
    expected = {folder / 'scoped-report.txt', folder / 'Nested/scoped-report-child.txt'}
    for path in (*expected, sibling / 'scoped-report-outside.txt'):
        path.write_text('fixture')
    raw = os.fsencode(documents) + b'/scoped-raw-\xff'
    os.mkdir(raw)
    raw_file = raw + b'/scoped-report-raw.txt'
    with open(raw_file, 'wb') as file:
        file.write(b'fixture')
    link = documents / 'scoped-link'
    link.symlink_to(folder, target_is_directory=True)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if {uri(path) for path in expected} | {uri(raw_file)} <= call('Search', 'scoped-report'):
            break
        time.sleep(0.03)
    else:
        raise AssertionError('folder fixtures were not indexed')
    assert call('SearchWithin', uri(folder), 'scoped-report') == {uri(path) for path in expected}
    assert call('SearchWithin', uri(raw), 'scoped-report') == {uri(raw_file)}
    assert call('ListDirectory', uri(raw)) == {uri(raw_file)}
    assert not call('SearchWithin', uri(folder), '')
    assert not call('SearchWithin', uri(folder), 's')
    outside = root / 'unindexed-directory'
    outside.mkdir()
    for target in ('bad-url', 'file://host/path', 'file:///bad%00path', uri(outside),
                   uri(link), uri(documents / 'Quarterly Report.pdf')):
        call('SearchWithin', target, 'report', ok=False)
    shutil.rmtree(folder)
    call('SearchWithin', uri(folder), 'report', ok=False)
    print('Folder search: subtree boundaries, raw paths, short queries and refusals passed')


if __name__ == '__main__':
    main()
