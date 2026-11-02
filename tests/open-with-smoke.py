#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Explicit document-handler protocol checks; called only by dbus-smoke.sh."""
import base64
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from urllib.parse import quote_from_bytes


def main():
    if os.environ.get('FERROSEEK_SMOKE_ISOLATED') != '1':
        raise SystemExit('Run through tests/dbus-smoke.sh on its private bus')
    root = Path(sys.argv[1])
    assert Path(os.environ['HOME']).is_relative_to(root)
    data = Path(os.environ['XDG_DATA_HOME'])
    config = Path(os.environ['XDG_CONFIG_HOME']) / 'mimeapps.list'
    config.write_text('[Default Applications]\napplication/pdf=document-viewer.desktop;\n')
    saved_defaults = config.read_bytes()
    entry = data / 'applications/matrix-handler.desktop'
    helper = root / 'bin/Open Helper'
    log = root / 'selected-args.json'
    # Atomic JSON records preserve arbitrary filename bytes, including newlines.
    helper.write_text('''#!/usr/bin/env python3
import base64, json, os, sys
from pathlib import Path
out = Path(os.environ['TEST_OPEN_LOG']).with_name('selected-args.json')
temp = out.with_name(out.name + '.' + str(os.getpid()))
temp.write_text(json.dumps({'args': [base64.b64encode(os.fsencode(arg)).decode() for arg in sys.argv[1:]], 'token': os.environ.get('XDG_ACTIVATION_TOKEN')}))
os.replace(temp, out)
''')
    helper.chmod(0o755)

    def write_entry(code='%f', extra=''):
        entry.write_text('[Desktop Entry]\nType=Application\nName=Matrix Viewer\n'
                         'Icon=application-pdf\nNoDisplay=true\nMimeType=application/pdf;\n'
                         f'Exec="{helper}" "literal; 100%%" %c %i %k {code}\n{extra}')

    def call(method, *args, ok=True):
        result = subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek',
                                 '--object-path', '/io/github/ferroseek/Search', '--method',
                                 f'io.github.ferroseek.Search1.{method}', *args], text=True,
                                capture_output=True, timeout=10)
        if ok:
            assert result.returncode == 0, result.stderr
        else:
            assert result.returncode != 0, f'{method} accepted an invalid request'
        return result

    def wait_for(check):
        end = time.monotonic() + 8
        while time.monotonic() < end:
            try:
                if check():
                    return
            except (FileNotFoundError, json.JSONDecodeError):
                pass
            time.sleep(0.03)
        raise AssertionError('timed out waiting for the private test service')

    def uri(path):
        return 'file://' + quote_from_bytes(os.fsencode(path), safe='/')

    write_entry()
    # The bytes are fixtures, not values recomputed from the launch implementation.
    names = [b'with spaces; $(literal) "quotes".pdf', '日本語 è %.pdf'.encode(), b'raw-\xff\xfe.pdf', b'line\nbreak.pdf']
    documents = []
    for name in names:
        path = os.fsencode(root / 'root/Documents') + b'/' + name
        with open(path, 'wb') as file:
            file.write(b'pdf fixture')
        documents.append(path)
    for code, path in zip(['%f', '%F', '%u', '%U'], documents):
        write_entry(code)
        url = uri(path)
        wait_for(lambda: 'matrix-handler.desktop' in subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek', '--object-path', '/io/github/ferroseek/Search', '--method', 'io.github.ferroseek.Search1.Applications', url], text=True, capture_output=True, timeout=10).stdout)
        log.unlink(missing_ok=True)
        call('OpenWith', url, 'matrix-handler.desktop', 'matrix-token')
        wait_for(log.exists)
        record = json.loads(log.read_text())
        expected_file = path if code in ('%f', '%F') else url.encode()
        expected = [b'literal; 100%', b'Matrix Viewer', b'--icon', b'application-pdf', os.fsencode(entry), expected_file]
        assert [base64.b64decode(arg) for arg in record['args']] == expected, code
        assert record['token'] == 'matrix-token'

    url = uri(documents[0])
    for method, args in [('Applications', ['file:///etc/passwd']),
                         ('Applications', [uri(root / 'root/Documents')]),
                         ('Applications', ['file:///bad%00path']),
                         ('OpenWith', ['file:///etc/passwd', 'matrix-handler.desktop', '']),
                         ('OpenWith', [url, str(helper), '']),
                         ('OpenWith', [url, 'missing.desktop', ''])]:
        call(method, *args, ok=False)
    # Candidates must be checked again after the list was displayed.
    for code, extra in [('%f', 'Hidden=true\n'), ('', ''), ('%f %U', ''), ('--files=%F', ''), ('%Z', '')]:
        write_entry(code, extra)
        log.unlink(missing_ok=True)
        call('OpenWith', url, 'matrix-handler.desktop', '', ok=False)
        assert not log.exists(), 'a refused request started the handler'
    write_entry()
    # The registered executable exists, but exec cannot start its interpreter.
    # This is a proven pre-dispatch failure, so another choice must remain possible.
    saved_helper = helper.read_bytes()
    for invalid_program in ('#!/nonexistent/ferroseek-test-interpreter\n', 'invalid executable format\n'):
        helper.write_text(invalid_program)
        refused = call('OpenWith', url, 'matrix-handler.desktop', '', ok=False)
        assert 'SpawnExecFailed' in refused.stderr, 'a known pre-dispatch error was reported as uncertain'
    helper.write_bytes(saved_helper)
    # A stub user scope records separate launches without contacting systemd.
    scope_log = root / 'selected-scopes.jsonl'
    systemd = root / 'bin/systemd-run'
    systemd.write_text('''#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
args = sys.argv[1:]
log = Path(os.environ['TEST_OPEN_LOG']).with_name('selected-scopes.jsonl')
with log.open('a') as file:
    file.write(json.dumps(args) + '\\n')
subprocess.run(args[args.index('--') + 1:], check=True)
''')
    systemd.chmod(0o755)
    private = Path(os.environ['XDG_RUNTIME_DIR']) / 'systemd/private'
    private.parent.mkdir(parents=True)
    private.touch()
    for count in (1, 2):
        call('OpenWith', url, 'matrix-handler.desktop', 'scope-token')
        wait_for(lambda: len(scope_log.read_text().splitlines()) == count)
    scopes = [next(arg for arg in json.loads(line) if arg.startswith('--unit='))
              for line in scope_log.read_text().splitlines()]
    assert len(set(scopes)) == 2, 'repeated document opens must use distinct user scopes'
    # Newly installed metadata can connect an alias through a parent type.
    # The editor's valid single-file field is embedded in its option argument.
    with (data / 'mime/globs2').open('a') as file:
        file.write('50:application/x-fixture-alias:*.ferrotest\n')
    (data / 'mime/aliases').write_text('application/x-fixture-alias application/x-fixture-document\n')
    (data / 'mime/subclasses').write_text('application/x-fixture-document text/x-fixture-parent\ntext/x-fixture-parent application/x-fixture-document\n')
    write_entry('--document=%f')
    entry.write_text(entry.read_text().replace('MimeType=application/pdf;', 'MimeType=text/x-fixture-parent;'))
    document = root / 'root/Documents/typed.ferrotest'
    document.write_text('new type fixture')
    url = uri(document)
    wait_for(lambda: 'matrix-handler.desktop' in subprocess.run(['gdbus', 'call', '--session', '--dest', 'io.github.ferroseek.FerroSeek', '--object-path', '/io/github/ferroseek/Search', '--method', 'io.github.ferroseek.Search1.Applications', url], text=True, capture_output=True, timeout=10).stdout)
    log.unlink(missing_ok=True)
    call('OpenWith', url, 'matrix-handler.desktop', 'typed-token')
    wait_for(log.exists)
    record = json.loads(log.read_text())
    assert base64.b64decode(record['args'][-1]) == b'--document=' + os.fsencode(document)
    assert record['token'] == 'typed-token'
    assert config.read_bytes() == saved_defaults, 'opening changed default associations'
    print('PASS: explicit handler arguments, stale choices, refusals, separate scopes and unchanged defaults')


if __name__ == '__main__':
    main()
