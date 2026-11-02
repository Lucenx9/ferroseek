#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Explicit, bounded file previews on the isolated daemon bus."""
import ast
import io
import os
from pathlib import Path
import subprocess
import sys
import time
import tokenize
from urllib.parse import quote_from_bytes


def main():
    root = Path(sys.argv[1])
    assert os.environ.get('FERROSEEK_SMOKE_ISOLATED') == '1'
    assert Path(os.environ['HOME']).is_relative_to(root)
    data = Path(os.environ['XDG_DATA_HOME']) / 'mime/globs2'
    with data.open('a') as file:
        file.write('50:text/plain:*.txt\n50:text/markdown:*.md\n')

    def uri(path):
        return 'file://' + quote_from_bytes(os.fsencode(path), safe='/')

    def call(method, *args, ok=True, control=False):
        service = 'FerroSeek' if control else 'Search'
        result = subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                 '--object-path', f'/io/github/ferroseek/{service}', '--method',
                                 f'io.github.ferroseek.{service}1.{method}', *args],
                                text=True, capture_output=True, timeout=5)
        assert (result.returncode == 0) == ok, result.stderr or result.stdout
        return result.stdout if ok else result.stderr

    def preview(path):
        output = call('TextPreview', uri(path))
        tokens = list(tokenize.generate_tokens(io.StringIO(output).readline))
        tokens = [(t.type, {'true': 'True', 'false': 'False'}.get(t.string, t.string)
                   if t.type == tokenize.NAME else t.string) for t in tokens]
        return ast.literal_eval(tokenize.untokenize(tokens))

    docs = root / 'root/Documents'
    assert preview(docs / 'notes-2024.md') == ('hi\n', False)
    fixtures = {
        'preview-utf8.txt': 'true false "quotes" and apostrophe\'\n日本語 é\r\nline\rnext'.encode(),
        'preview-empty.txt': b'',
        'preview-binary.txt': b'text\x00binary',
        'preview-invalid.txt': b'bad\xfftext',
        'preview-control.txt': b'\x1b[31mtext',
        'preview-lines.txt': '\n'.join(str(i) for i in range(500)).encode(),
        'preview-chars.txt': ('é' * 20000).encode(),
        'preview-bytes.txt': b'A' * 65535 + '😀'.encode() + b'end',
        'preview-replaced.txt': b'original',
        'preview-pipe.txt': b'original',
        'preview-atomic.txt': b'original',
    }
    for name, content in fixtures.items():
        (docs / name).write_bytes(content)
    parent = docs / 'preview-parent'
    parent.mkdir()
    child = parent / 'preview-child.txt'
    child.write_text('original')
    raw = os.fsencode(docs) + b'/preview-raw-\xff.txt'
    with open(raw, 'wb') as file:
        file.write(b'raw path')
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        found = call('Search', 'preview')
        required = {uri(docs / name) for name in fixtures} | {uri(raw), uri(child)}
        if all(target in found for target in required):
            break
        time.sleep(0.03)
    else:
        raise AssertionError('preview fixtures were not indexed')
    assert preview(docs / 'preview-utf8.txt') == ('true false "quotes" and apostrophe\'\n日本語 é\nline\nnext', False)
    assert preview(docs / 'preview-empty.txt') == ('', False)
    assert preview(docs / 'preview-lines.txt') == ('\n'.join(str(i) for i in range(200)), True)
    assert preview(docs / 'preview-chars.txt') == ('é' * 16384, True)
    assert preview(docs / 'preview-bytes.txt') == ('A' * 16384, True)
    assert preview(raw) == ('raw path', False)
    for name in ('preview-binary.txt', 'preview-invalid.txt', 'preview-control.txt', 'Quarterly Report.pdf'):
        assert 'NotSupported' in call('TextPreview', uri(docs / name), ok=False)
    for controls in (b'hello\x7fworld', 'hello\u0085world'.encode(), 'hello\u009bworld'.encode()):
        (docs / 'preview-binary.txt').write_bytes(controls)
        assert 'NotSupported' in call('TextPreview', uri(docs / 'preview-binary.txt'), ok=False)
    for target in ('bad-url', 'file://host/path', 'file:///bad%00path', 'file:///etc/passwd', uri(docs)):
        call('TextPreview', target, ok=False)
    # Keep the indexed identity unchanged while replacing paths on disk.
    call('Pause', control=True)
    try:
        outside = root / 'outside.txt'
        outside.write_text('outside fixture must not be read')
        replaced = docs / 'preview-replaced.txt'
        replaced.unlink()
        replaced.symlink_to(outside)
        pipe = docs / 'preview-pipe.txt'
        pipe.unlink()
        os.mkfifo(pipe)
        atomic = docs / 'preview-atomic.txt'
        replacement = root / 'replacement.txt'
        replacement.write_text('new inode')
        os.replace(replacement, atomic)
        parent.rename(root / 'held-parent')
        parent.symlink_to(root, target_is_directory=True)
        (root / 'preview-child.txt').write_text('outside ancestor fixture')
        for path in (replaced, pipe, atomic, child):
            call('TextPreview', uri(path), ok=False)
    finally:
        call('Resume', control=True)
    print('Text preview: bounds, encoding, raw paths, unsupported types and safe refusals passed')


if __name__ == '__main__':
    main()
