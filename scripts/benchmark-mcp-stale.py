#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Reproduce stale-prefix starvation on disposable data; baseline failures are explicit."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

spec=importlib.util.spec_from_file_location('mcp_bench',Path(__file__).with_name('benchmark-mcp.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--baseline',type=Path,required=True)
p.add_argument('--candidate',type=Path,required=True)
p.add_argument('--cli',type=Path,default=Path('target/release/ferroseek'))
p.add_argument('--output',type=Path,required=True)
a=p.parse_args();assert not a.work.exists() and not a.output.exists()
a.work.mkdir(parents=True);work=a.work.resolve();root=work/'corpus'
for i in range(8300):
    d=root/'gone'/f'd{i:04}';d.mkdir(parents=True);(d/'needle.txt').write_bytes(b'x')
(root/'live').mkdir()
expected={str(root/'live'/n) for n in ['a-long-needle-survivor.txt','another-needle-survivor.txt']}
for path in expected:Path(path).write_bytes(b'live')
config=work/'config.toml';config.write_text('roots = '+json.dumps([str(root)])+'\nmax_cpu_percent = 100\nlow_priority = false\n')
env=dict(os.environ,FERROSEEK_CONFIG=str(config),FERROSEEK_DATA_DIR=str(work/'data'),XDG_CONFIG_HOME=str(work/'xdg'),DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-benchmark')
subprocess.run([str(a.cli.resolve()),'index'],env=env,check=True,capture_output=True)
shutil.rmtree(root/'gone')
rows=[]
for rep in range(5):
    for side,binary in [('baseline',a.baseline),('candidate',a.candidate)]:
        c=b.Client([str(binary.resolve()),'--index',str(work/'data/index.sqlite'),'--allow',str(root)],env)
        try:
            c.request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'stale','version':'1'}})
            c.notify('notifications/initialized')
            c.call('search_files',{'query':'needle','limit':20})
            cpu=b.cpu_ticks(c.p.pid)
            for sample in range(5):
                ms,_,err,res=c.call('search_files',{'query':'needle','limit':20})
                assert not err and res is not None
                found={x['path'] for x in res['results']}
                assert found<=expected
                if side=='candidate':assert found==expected and not res['more_available'] and not res['incomplete_due_to_changes']
                rows.append(dict(repetition=rep,side=side,sample=sample,ms=ms,recall=len(found)/len(expected),
                                 returned=len(found),expected=len(expected),omitted_stale=res['omitted_stale']))
            st=b.proc_status(c.p.pid)
            rows.append(dict(repetition=rep,side=side,metric='batch',cpu_ms=1000*(b.cpu_ticks(c.p.pid)-cpu)/os.sysconf('SC_CLK_TCK'),rss_kib=st['VmRSS'],peak_rss_kib=st['VmHWM']))
        finally:
            c.p.stdin.close();assert c.p.wait(timeout=10)==0
        print(rep+1,side,'stale complete',flush=True)
a.output.write_text(json.dumps(dict(stale_entries=8300,live_entries=2,repetitions=5,rows=rows),indent=2)+'\n')
