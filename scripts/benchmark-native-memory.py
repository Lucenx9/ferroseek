#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Separate post-exec /proc memory sampling; not used for latency comparisons.

wait4 maxrss can include the Python launcher's inherited pre-exec image.
Sample VmHWM of the executed tool instead, with coverage counts. Very short
processes can exit before any sample; those are explicitly null, not zero.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import time

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
a=p.parse_args();work=a.work.resolve();assert (work/'repo0').is_dir() and not a.output.exists()
roots=[str(work/f'repo{i}') for i in range(4)]
commands={'fd':['fd','--absolute-path','--type','f','--no-ignore','--ignore-case','--fixed-strings','--print0','module000501.rs',*roots],
          'rg':['rg','--files','--no-config','--no-ignore','--null','--iglob','*module000501.rs*',*roots],
          'plocate':['plocate','-d',str(work/'plocate.db'),'-0','-i','-b','module000501.rs']}
rows=[]
for rep in range(5):
    for tool,cmd in commands.items():
        if not shutil.which(tool):continue
        process=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        stop=threading.Event();samples=[]
        def sample():
            while not stop.is_set():
                try:
                    if Path(f'/proc/{process.pid}/comm').read_text().strip()==tool:
                        status=Path(f'/proc/{process.pid}/status').read_text()
                        fields={k:int(v.split()[0]) for l in status.splitlines() if ':' in l
                                for k,v in [l.split(':',1)] if k in ['VmRSS','VmHWM']}
                        if fields:samples.append(fields)
                except (OSError,ValueError):pass
                stop.wait(.0005)
        thread=threading.Thread(target=sample);thread.start()
        try:
            out,err=process.communicate(timeout=30)
            assert process.returncode==0 and not err and out
        finally:
            stop.set();thread.join()
        rows.append(dict(repetition=rep,tool=tool,samples=len(samples),
                         post_exec_hwm_kib=max((s.get('VmHWM',0) for s in samples),default=None)))
a.output.write_text(json.dumps(dict(rows=rows,note='Independent memory-only runs of multi-exact query. Sampled post-exec VmHWM; no Python launch image.'),indent=2)+'\n')
