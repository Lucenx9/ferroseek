#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Bounded indexed directory listings through the isolated daemon bus."""
import os
from pathlib import Path
import re
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

    def call(method, *args, ok=True, control=False):
        service = 'FerroSeek' if control else 'Search'
        result = subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                 '--object-path', f'/io/github/ferroseek/{service}', '--method',
                                 f'io.github.ferroseek.{service}1.{method}', *args],
                                capture_output=True, text=True, timeout=8)
        assert (result.returncode == 0) == ok, result.stderr or result.stdout
        return result.stdout if ok else result.stderr

    def listing(path):
        output = call('ListDirectory', uri(path))
        entries = re.findall(r"'(file://[^']*)'", output)
        more = re.search(r', (true|false)\)\s*$', output)
        assert more, output
        return entries, more[1] == 'true'

    docs = root / 'root/Documents'
    folder = docs / 'browser-folder è'
    (folder / 'a-dir').mkdir(parents=True)
    (folder / 'Z-dir').mkdir()
    (folder / 'Z-dir/deeper.txt').write_text('nested fixture')
    files = ['a.txt', 'B.txt', "quote's.txt"]
    for name in files:
        (folder / name).write_text('fixture')
    raw = os.fsencode(folder) + b'/raw-\xff.txt'
    with open(raw, 'wb') as file:
        file.write(b'fixture')
    (folder / 'symlink').symlink_to(folder / 'a.txt')
    os.mkfifo(folder / 'pipe')
    link = docs / 'browser-link'
    link.symlink_to(folder, target_is_directory=True)
    bulk = docs / 'browser-bulk'
    bulk.mkdir()
    for i in range(201):
        (bulk / f'item-{i:03}.txt').write_text('')
    replaced = docs / 'browser-replaced'
    replaced.mkdir()
    expected = [uri(folder / name) for name in ['a-dir', 'Z-dir', *files]] + [uri(raw)]
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        found = call('Search', 'browser')
        if all(uri(path) in found for path in (folder, bulk, replaced)):
            entries, more = listing(folder)
            bulk_entries, bulk_more = listing(bulk)
            if entries == expected and not more and len(bulk_entries) == 200 and bulk_more:
                break
        time.sleep(0.03)
    else:
        raise AssertionError('directory fixtures did not settle in the index')
    assert bulk_entries == [uri(bulk / f'item-{i:03}.txt') for i in range(200)]
    assert listing(folder / 'a-dir') == ([], False)
    assert uri(folder / 'Z-dir/deeper.txt') in call('SearchWithin', uri(folder), 'deeper')
    outside = root / 'browser-unindexed'
    outside.mkdir()
    for target in ('bad-url', 'file://host/path', 'file:///bad%00path', uri(outside), uri(link), uri(folder / 'a.txt')):
        call('ListDirectory', target, ok=False)
    call('Pause', control=True)
    try:
        replaced.rename(root / 'held-browser-directory')
        replaced.mkdir()
        call('ListDirectory', uri(replaced), ok=False)
        call('SearchWithin', uri(replaced), 'item', ok=False)
        (folder / 'a-dir').rmdir()
        call('ListDirectory', uri(folder / 'a-dir'), ok=False)
    finally:
        call('Resume', control=True)
    print('Folder browser: direct children, ordering, bounds, raw paths, empty folders and identity refusals passed')


if __name__ == '__main__':
    main()
