#!/usr/bin/env python3
"""Read-only /usr/include workload with a filesystem oracle and isolated indexes."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

p = argparse.ArgumentParser()
p.add_argument('--baseline', type=Path, required=True)
p.add_argument('--candidate', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
source = Path('/usr/include')
paths = []
for parent, dirs, files in os.walk(source, followlinks=False):
    paths.extend(Path(parent) / name for name in dirs + files)
queries = ['linux/', 'bits/', 'boost/geometry/', 'include/stdio', 'ferroseek-missing-path/']
expected = {q: {os.fsencode(path) for path in paths if q in str(path).lower()} for q in queries}
rows = []
with tempfile.TemporaryDirectory(prefix='ferroseek-real-', dir=Path.home() / '.cache') as temp:
    root = Path(temp)
    cfg = root / 'config.toml'
    cfg.write_text('roots = ["/usr/include"]\nexclude_names = []\nindex_hidden = true\nrespect_cachedir_tag = false\nstay_on_filesystem = false\n')
    for repetition in range(1, 6):
        for side, binary in [('baseline', a.baseline), ('candidate', a.candidate)]:
            binary = binary.resolve() / 'ferroseek'
            data = root / f'{side}-{repetition}'
            env = dict(os.environ, FERROSEEK_CONFIG=str(cfg), FERROSEEK_DATA_DIR=str(data),
                       DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-test-bus')
            t = time.perf_counter()
            subprocess.run([binary, 'index'], env=env, check=True, capture_output=True)
            row = dict(side=side, repetition=repetition, index_s=time.perf_counter()-t,
                       database_bytes=(data / 'index.sqlite').stat().st_size, queries={})
            for q in queries:
                samples = []
                for sample in range(6):
                    t = time.perf_counter()
                    r = subprocess.run([binary, 'search', '-0', '-n', str(len(paths)+1), q], env=env, capture_output=True)
                    elapsed = (time.perf_counter()-t)*1000
                    assert r.returncode == (0 if expected[q] else 1) and not r.stderr, r.stderr
                    actual = set(r.stdout.split(b'\0')) - {b''}
                    assert actual == expected[q], (side, q, len(actual), len(expected[q]), list(expected[q]-actual)[:3])
                    if sample: samples.append(elapsed)
                row['queries'][q] = dict(ms=samples, expected=len(expected[q]))
            rows.append(row)
            print(side, repetition, 'verified', flush=True)
a.output.write_text(json.dumps(dict(source=str(source), entries=len(paths), rows=rows), indent=2))
