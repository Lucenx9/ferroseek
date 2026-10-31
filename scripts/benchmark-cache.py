#!/usr/bin/env python3
"""Compare fresh CLI processes after advisory eviction of a benchmark DB.

Does not drop global caches. All daemons using this benchmark index must be
stopped. DONTNEED is advisory: input-block counters are recorded, not assumed.
"""
import argparse
import json
import os
from pathlib import Path
import resource
import subprocess
import time

p = argparse.ArgumentParser()
p.add_argument('--baseline', type=Path, required=True)
p.add_argument('--candidate', type=Path, required=True)
p.add_argument('--work', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
work = a.work.resolve()
assert (work / 'report.md').is_file() and (work / 'corpus').is_dir(), 'benchmark work directory required'
env = dict(os.environ, FERROSEEK_CONFIG=str(work/'config.toml'), FERROSEEK_DATA_DIR=str(work/'data'),
           DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-test-bus')
rows = []
for repetition in range(1, 6):
    for side, binary in [('baseline', a.baseline), ('candidate', a.candidate)]:
        fd = os.open(work/'data/index.sqlite', os.O_RDONLY)
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
        before = resource.getrusage(resource.RUSAGE_CHILDREN)
        t = time.perf_counter()
        result = subprocess.run([str(binary.resolve()/'ferroseek'), 'search', '-0', 'ferroseek-review-absent/'],
                                env=env, capture_output=True)
        wall_ms = (time.perf_counter()-t)*1000
        after = resource.getrusage(resource.RUSAGE_CHILDREN)
        assert result.returncode == 1 and not result.stdout and not result.stderr, (side, result.returncode, result.stderr)
        rows.append(dict(side=side, repetition=repetition, wall_ms=wall_ms,
                         cpu_ms=1000*(after.ru_utime+after.ru_stime-before.ru_utime-before.ru_stime),
                         input_blocks=after.ru_inblock-before.ru_inblock))
a.output.write_text(json.dumps(rows, indent=2))
print('PASS: 10 advisory-eviction missing-path queries returned no matches')
