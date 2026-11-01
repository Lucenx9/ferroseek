#!/usr/bin/env python3
"""Check benchmark work-directory safety and the explicit corpus reuse path."""
from pathlib import Path
import subprocess
import tempfile

binary = Path('target/release/ferroseek-bench').resolve()
bin_dir = binary.parent
with tempfile.TemporaryDirectory(prefix='ferroseek-bench-test-') as tmp:
    work = Path(tmp) / 'home/work'
    work.mkdir(parents=True)
    sentinel = work / 'keep-me'
    sentinel.write_bytes(b'untouched')

    def run(*flags, files='1000'):
        return subprocess.run([str(binary), '--bin-dir', str(bin_dir), '--work', str(work),
                               '--files', files, *flags], capture_output=True, timeout=60)

    for flags, message in [([], b'new or empty directory'),
                           (['--queries-only'], b'requires --reuse-corpus'),
                           (['--reuse-corpus'], b'requires --only-initial or --queries-only'),
                           (['--reuse-corpus', '--only-initial'], b'existing benchmark report required')]:
        result = run(*flags)
        assert result.returncode != 0 and message in result.stderr, result.stderr
        assert sentinel.read_bytes() == b'untouched'
    sentinel.unlink()
    for flags in [['--only-initial'], ['--only-initial', '--reuse-corpus', '--cpu', '50']]:
        result = run(*flags)
        assert result.returncode == 0, result.stderr
        assert (work / 'data/index.sqlite').is_file()
    before = (work / 'report.md').read_bytes()
    result = run('--reuse-corpus', '--queries-only')
    assert result.returncode == 0, result.stderr
    assert b'errors/mismatches: 0' in result.stdout, result.stdout
    assert b'Activating service' not in result.stderr, result.stderr
    result = run('--only-initial', '--reuse-corpus', files='2000')
    assert result.returncode != 0 and b'matching generated benchmark' in result.stderr
    assert (work / 'report.md').read_bytes() == before
print('PASS: safe corpus reuse, indexing and launcher queries; 165 checked responses without service activation')
