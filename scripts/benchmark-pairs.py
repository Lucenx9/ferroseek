#!/usr/bin/env python3
"""Alternate release daemons on existing synthetic corpora; never purge indexes.

Dataset JSON maps generated file counts to existing benchmark work directories.
Library rows always use the harness build. Compare only D-Bus rows across sides.
"""
import argparse
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--baseline', type=Path, required=True)
p.add_argument('--candidate', type=Path, required=True)
p.add_argument('--harness', type=Path, required=True)
p.add_argument('--datasets', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--repetitions', type=int, default=5)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
for size, work in json.loads(a.datasets.read_text()).items():
    for repetition in range(1, a.repetitions + 1):
        for side, binary in [('baseline', a.baseline), ('candidate', a.candidate)]:
            report = a.output / f'{size}-{repetition}-{side}.md'
            with report.open('x') as f:
                result = subprocess.run([str(a.harness.resolve()), '--files', size, '--work', work,
                                         '--reuse-corpus', '--queries-only', '--bin-dir', str(binary.resolve())],
                                        stdout=f, stderr=subprocess.PIPE, text=True)
            report.with_suffix('.log').write_text(result.stderr)
            if result.returncode:
                raise SystemExit(f'{report}: failed with {result.returncode}; see log')
            print(size, repetition, side, 'complete', flush=True)
