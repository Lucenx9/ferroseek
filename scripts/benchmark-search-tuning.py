#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Validate native thread-count tradeoffs without changing matching semantics."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

spec=importlib.util.spec_from_file_location('comparison',Path(__file__).with_name('benchmark-search-tools.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
a=p.parse_args();work=a.work.resolve();assert (work/'repo0').is_dir() and not a.output.exists()
repos=[work/f'repo{i}' for i in range(4)];real=work/'real-repository'
cases=[('single-exact',[repos[0]],'module000501.rs'),('multi-exact',repos,'module000501.rs'),
       ('single-path',[repos[0]],'group005/ module0005'),('multi-path',repos,'group005/ module000501'),
       ('multi-cargo',repos+[real],'Cargo.toml'),('real-repository',[real],'main.rs'),('missing',repos+[real],'no-such-fixture.rs')]
def match(paths,roots,q):
    return {s for s in paths if any(s.startswith(str(r)+'/') for r in roots)
            and all(t.lower() in (s if '/' in t else Path(s).name).lower() for t in q.split())}
expected={}
for name,roots,q in cases:
    paths=[]
    for root in roots:
        for parent,dirs,files in os.walk(root):
            dirs[:]=[d for d in dirs if not d.startswith('.')]
            paths.extend(str(Path(parent)/f) for f in files if not f.startswith('.'))
    expected[name]=match(paths,roots,q)
variants=[(tool,n) for tool in ['fd','rg'] for n in [1,2,4,0]]
rows=[]
for rep in range(5):
    for name,roots,q in cases:
        chosen=next(t for t in q.split() if '/' not in t)
        for tool,n in variants[rep:]+variants[:rep]:
            cmd=([tool,'-a','-t','f','--no-ignore','-i','-F','-0']+(['-j',str(n)] if n else [])+[chosen,*roots]) if tool=='fd' else ([tool,'--files','--no-config','--no-ignore','--null','--iglob','*'+chosen+'*']+(['--threads',str(n)] if n else [])+roots)
            start=time.perf_counter();p,m=b.run(cmd,os.environ,work/'unused')
            actual=match([os.fsdecode(s) for s in p.stdout.split(b'\0') if s],roots,q)
            m['ms']=1000*(time.perf_counter()-start)
            assert actual==expected[name]
            rows.append(dict(repetition=rep,case=name,tool=tool,threads=n,expected=len(actual),**m))
a.output.write_text(json.dumps(dict(rows=rows,errors=0,note='Five rotations of 1/2/4/automatic threads; 0 means tool default. Correctness checked for every sample; this is tuning evidence, not the main repeated-query run.'),indent=2)+'\n')
