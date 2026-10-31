#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Five alternating MCP builds on a read-only synthetic index; no daemon changes."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import random
import sqlite3
import tempfile
import time

spec=importlib.util.spec_from_file_location('mcp_bench',Path(__file__).with_name('benchmark-mcp.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--baseline',type=Path,required=True)
p.add_argument('--candidate',type=Path,required=True)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--output',type=Path,required=True)
a=p.parse_args()
assert not a.output.exists()
work=a.work.resolve();assert (work/'report.md').is_file()
allow=work/'corpus';scope=allow/'Personal';assert scope.is_dir()
dbpath=work/'data/index.sqlite'
with sqlite3.connect(dbpath.as_uri()+'?mode=ro',uri=True) as db:
    maximum=db.execute('SELECT max(id) FROM entries').fetchone()[0]
    rng=random.Random(7);queries=[]
    while len(queries)<100:
        row=db.execute('SELECT key FROM entries WHERE id=?',(rng.randint(1,maximum),)).fetchone()
        if not row or len(row[0])<4 or row[0].startswith('.'):continue
        n=row[0];k=min(len(n),rng.randint(3,8));start=rng.randint(0,len(n)-k)
        queries.append(n[start:start+k])
rows=[];reference={}
with tempfile.TemporaryDirectory() as tmp:
    env=dict(os.environ,XDG_CONFIG_HOME=tmp)
    for rep in range(5):
        for side,binary in [('baseline',a.baseline),('candidate',a.candidate)]:
            c=b.Client([str(binary.resolve()),'--index',str(dbpath),'--allow',str(allow)],env)
            try:
                msg,_=c.request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'paired','version':'1'}})
                assert 'error' not in msg;c.notify('notifications/initialized')
                for scoped in [False,True]:
                    args=lambda q:dict(query=q,limit=20,**({'root':str(scope)} if scoped else {}))
                    for q in queries[:10]:c.call('search_files',args(q))
                    cpu0=b.cpu_ticks(c.p.pid)
                    for i,q in enumerate(queries):
                        cpu=time.process_time();ms,size,err,res=c.call('search_files',args(q));client_cpu=1000*(time.process_time()-cpu)
                        assert not err and res is not None and not res.get('incomplete_due_to_changes',False)
                        paths=[r['path'] for r in res['results']]
                        # Filesystem membership and query predicates are independent of index output.
                        for entry in res['results']:
                            path=entry['path']
                            exists=os.path.lexists(path)
                            if not exists and entry.get('non_utf8'):
                                exists=any(os.fsencode(x).decode('utf8','replace')==path for x in Path(path).parent.iterdir())
                            assert exists and Path(path).is_relative_to(scope if scoped else allow)
                            assert all(t.lower() in (path if '/' in t else Path(path).name).lower() for t in q.split())
                        key=(scoped,i)
                        if key not in reference:reference[key]=paths
                        assert paths==reference[key],(side,scoped,i,'ordered result regression')
                        rows.append(dict(repetition=rep,side=side,scoped=scoped,query=i,ms=ms,response_bytes=size,results=len(paths),client_cpu_ms=client_cpu))
                    rows.append(dict(repetition=rep,side=side,scoped=scoped,metric='batch',cpu_ms=1000*(b.cpu_ticks(c.p.pid)-cpu0)/os.sysconf('SC_CLK_TCK'),rss_kib=b.proc_status(c.p.pid)['VmRSS']))
                print(rep+1,side,'complete',flush=True)
            finally:
                c.p.stdin.close();assert c.p.wait(timeout=10)==0
    a.output.write_text(json.dumps(dict(seed=7,queries=100,repetitions=5,rows=rows,errors=0,ordered_result_mismatches=0),indent=2)+'\n')
