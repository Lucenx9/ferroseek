#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Private-bus watcher maintenance and MCP visibility on an existing benchmark corpus."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import time

spec=importlib.util.spec_from_file_location('mcp_bench',Path(__file__).with_name('benchmark-mcp.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--bin-dir',type=Path,default=Path('target/release'))
p.add_argument('--output',type=Path,required=True)
a=p.parse_args();work=a.work.resolve();binary=a.bin_dir.resolve()
assert (work/'repo0').is_dir() and (work/'config.toml').is_file() and not a.output.exists()
env=dict(os.environ,FERROSEEK_CONFIG=str(work/'config.toml'),FERROSEEK_DATA_DIR=str(work/'data'),XDG_CONFIG_HOME=str(work/'xdg'))
rows=[]
def io(pid):return {k:int(v) for k,v in (l.split(':') for l in Path(f'/proc/{pid}/io').read_text().splitlines())}
for rep in range(5):
    log=(work/f'watcher-{rep}.log').open('wb')
    daemon=subprocess.Popen(['dbus-run-session','--',str(binary/'ferroseekd')],env=env,stdout=subprocess.DEVNULL,stderr=log,start_new_session=True)
    c=None
    try:
        deadline=time.monotonic()+15;pid=None
        while pid is None:
            children=Path(f'/proc/{daemon.pid}/task/{daemon.pid}/children').read_text().split()
            for child in children:
                if Path(f'/proc/{child}/comm').read_text().strip()=='ferroseekd':pid=int(child)
            assert time.monotonic()<deadline and daemon.poll() is None, 'private daemon failed to start'
            if pid is None:time.sleep(.01)
        c=b.Client([str(binary/'ferroseek-mcp'),'--index',str(work/'data/index.sqlite'),'--config',str(work/'mcp.toml'),'--allow',str(work/'repo0')],env)
        c.request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'watcher','version':'1'}})
        c.notify('notifications/initialized')
        # Initial quick reconciliation plus watch setup must finish first.
        bus = next(v.split(b'=',1)[1].decode() for v in Path(f'/proc/{pid}/environ').read_bytes().split(b'\0') if v.startswith(b'DBUS_SESSION_BUS_ADDRESS='))
        while True:
            status=subprocess.run([str(binary/'ferroseek'),'status'],env=dict(env,DBUS_SESSION_BUS_ADDRESS=bus),capture_output=True,text=True)
            if status.returncode==0 and 'state: idle' in status.stdout:break
            assert time.monotonic()<deadline, 'watcher did not become idle'
            time.sleep(.02)
        db=sqlite3.connect((work/'data/index.sqlite').as_uri()+'?mode=ro',uri=True)
        before_io=io(pid);before_cpu=b.cpu_ticks(pid)
        for sample in range(5):
            path=work/'repo0'/f'live-probe-{rep}-{sample}.rs'
            moved=path.with_name(f'live-moved-{rep}-{sample}.rs')
            assert not path.exists() and not moved.exists()
            for phase in ['create','rename','delete']:
                start=time.perf_counter()
                if phase=='create':path.write_bytes(b'live benchmark')
                elif phase=='rename':path.rename(moved)
                else:moved.unlink()
                query=(path if phase=='create' else moved).name
                expected=0 if phase=='delete' else 1
                polls=0
                while True:
                    ms,_,err,res=c.call('search_files',{'query':query,'limit':20})
                    polls+=1
                    assert not err and res is not None
                    if len(res['results'])==expected:break
                    assert time.perf_counter()-start<10
                    time.sleep(.005)
                if phase=='rename':
                    _,_,err,old=c.call('search_files',{'query':path.name})
                    assert not err and not old['results']
                search_visible_ms=1000*(time.perf_counter()-start)
                while db.execute('SELECT count(*) FROM entries WHERE key=?',(query,)).fetchone()[0]!=expected:
                    assert time.perf_counter()-start<10
                    time.sleep(.005)
                rows.append(dict(repetition=rep,sample=sample,phase=phase,visibility_ms=search_visible_ms,
                                 index_convergence_ms=1000*(time.perf_counter()-start),polls=polls,last_request_ms=ms))
        db.close()
        after_io=io(pid);st=b.proc_status(pid)
        rows.append(dict(repetition=rep,phase='resources',cpu_ms=1000*(b.cpu_ticks(pid)-before_cpu)/os.sysconf('SC_CLK_TCK'),
                         rss_kib=st['VmRSS'],peak_rss_kib=st['VmHWM'],io={k:after_io[k]-before_io[k] for k in before_io}))
    finally:
        if c is not None:c.p.stdin.close();assert c.p.wait(timeout=10)==0
        if pid is not None:
            os.kill(pid,signal.SIGTERM)
        elif daemon.poll() is None:
            os.killpg(daemon.pid,signal.SIGTERM)
        daemon.wait(timeout=10);log.close()
    # Private dbus daemon startup announcements are expected; reject engine errors.
    errors=(work/f'watcher-{rep}.log').read_text()
    assert 'ERROR' not in errors and 'WARN' not in errors and '<3>' not in errors and '<4>' not in errors, 'unexpected watcher diagnostic'
    print(rep+1,'watcher complete',flush=True)
a.output.write_text(json.dumps(dict(repetitions=5,operations_per_kind=25,rows=rows,errors=0,debounce_ms=200,
                                   note='Search drops deleted paths on disk before index removal; delete visibility is not index convergence.'),indent=2)+'\n')
