#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Isolated, warm-cache filename comparison; never uses the desktop index.

Five process repetitions; native CLI startup is included, MCP is a resident
stdio server. Every measured result set is checked against an independent
filesystem oracle. No content-search command is used. JSON contains no host paths.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import time

spec = importlib.util.spec_from_file_location('mcp_bench', Path(__file__).with_name('benchmark-mcp.py'))
mb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mb)


class MeasuredProcess(subprocess.Popen):
    # Popen owns/reaps the child; capture its wait4 usage instead of a global
    # RUSAGE_CHILDREN maximum. Verified on the recorded Python version.
    usage = None

    def _try_wait(self, flags):
        pid, status, usage = os.wait4(self.pid, flags)
        if pid:
            self.usage = usage
        return pid, status


def run(cmd, env, metric_file, indexing=False):
    begin = time.perf_counter()
    with MeasuredProcess(list(map(str, cmd)), env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as p:
        stdout, stderr = p.communicate(timeout=120)
        elapsed = (time.perf_counter() - begin) * 1000
        usage = p.usage
        assert usage is not None
        assert p.returncode in (0, 1) and (not stderr or (indexing and stderr.decode().strip() == 'daemon not running; indexing in the foreground…')), (cmd[0], p.returncode, stderr)
        result = subprocess.CompletedProcess(cmd, p.returncode, stdout, stderr)
    return result, dict(ms=elapsed, cpu_ms=1000*(usage.ru_utime+usage.ru_stime),
                        peak_rss_kib=usage.ru_maxrss, input_blocks=usage.ru_inblock, output_blocks=usage.ru_oublock)


def process_io(pid):
    return {k: int(v) for k, v in (l.split(':') for l in Path(f'/proc/{pid}/io').read_text().splitlines())}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--work', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--bin-dir', type=Path, default=Path('target/release'))
    ap.add_argument('--source-ref', default='HEAD', help='public tracked snapshot for the realistic repository')
    ap.add_argument('--files-per-repo', type=int, default=25000)
    ap.add_argument('--repetitions', type=int, default=5)
    ap.add_argument('--queries-per-case', type=int, default=15)
    a = ap.parse_args()
    assert a.files_per_repo >= 1000 and a.repetitions >= 5
    assert not a.work.exists(), 'work directory must be new'
    assert not a.output.exists(), 'output must be new'
    a.work.mkdir(parents=True)
    work = a.work.resolve()
    binary = a.bin_dir.resolve()
    repos = [work/f'repo{i}' for i in range(4)]
    for repo in repos:
        for i in range(a.files_per_repo):
            f = repo/'src'/f'group{i//100:03d}'/f'module{i:06d}.rs'
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_bytes(b'// benchmark fixture\n')
        (repo/'Cargo.toml').write_text('[package]\nname="fixture"\n')
        (repo/'src/main.rs').write_text('fn main() {}\n')
    # A public tracked source tree, not private working-tree files.
    real = work/'real-repository'
    real.mkdir()
    import tarfile, io
    with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',a.source_ref]))) as t:
        t.extractall(real, filter='data')
    repos.append(real)
    private = work/'unauthorized'
    private.mkdir()
    (private/'Cargo.toml').write_text('not authorized')
    cfg = work/'config.toml'
    cfg.write_text('roots = '+json.dumps([str(p) for p in repos+[private]])+'\nexclude_names = []\nmax_cpu_percent = 100\nlow_priority = false\nrespect_cachedir_tag = false\n')
    policy = work/'mcp.toml'
    policy.write_text('deny_defaults = false\nmax_results = 200\n')
    env = dict(os.environ, FERROSEEK_CONFIG=str(cfg), FERROSEEK_DATA_DIR=str(work/'data'),
               XDG_CONFIG_HOME=str(work/'xdg'), DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-benchmark')
    metric_file = work/'metrics.txt'
    cases = [dict(name='single-exact', roots=[repos[0]], query='module000501.rs'),
             dict(name='multi-exact', roots=repos[:4], query='module000501.rs'),
             dict(name='single-path', roots=[repos[0]], query='group005/ module0005'),
             dict(name='multi-path', roots=repos[:4], query='group005/ module000501'),
             dict(name='multi-cargo', roots=repos, query='Cargo.toml'),
             dict(name='real-repository', roots=[real], query='main.rs'),
             dict(name='missing', roots=repos, query='no-such-fixture.rs')]
    rows, maintenance, memory, freshness = [], [], [], []
    tools = ['mcp','fd','rg'] + (['plocate'] if shutil.which('plocate') and shutil.which('updatedb') else [])
    def oracle(roots, query):
        names=[]
        for root in roots:
            for parent, dirs, files in os.walk(root):
                dirs[:] = [d for d in dirs if not d.startswith('.')]
                names.extend(str(Path(parent)/f) for f in files if not f.startswith('.'))
        return match(names, roots, query)
    def match(names, roots, query):
        # Independent literal substring semantics, not any tool's result order.
        roots=[str(r).rstrip('/')+'/' for r in roots]
        tokens=query.lower().split()
        found=set()
        for s in names:
            lower=s.lower();name=lower.rsplit('/',1)[-1]
            if not all(t in (lower if '/' in t else name) for t in tokens):continue
            root=next((r for r in roots if s.startswith(r)),None)
            if root is None or any(p.startswith('.') for p in s[len(root):].split('/')):continue
            if Path(s).is_file():found.add(s)
        return found
    pl_db = work/'plocate.db'
    def build_plocate():
        # updatedb performs its own enumeration; timed maintenance includes it.
        return run(['updatedb','--database-root',work,'--output',pl_db,'--require-visibility','no',
                    '--prunepaths',str(work/'data'), '--prunenames','.git', '--prunefs','', '--prune-bind-mounts','no'],env,metric_file)
    def native(tool, case):
        q, roots = case['query'], case['roots']
        tokens=q.split()
        chosen = next((t for t in tokens if '/' not in t),tokens[0])
        if tool=='fd':
            cmd=['fd','--absolute-path','--type','f','--no-ignore','--ignore-case','--fixed-strings','--print0']
            if '/' in chosen:cmd+=['--full-path']
            cmd += ['--',chosen,*roots]
        elif tool=='rg':cmd=['rg','--files','--no-config','--no-ignore','--null','--iglob','*'+chosen+'*',*roots]
        else:
            cmd=['plocate','-d',pl_db,'-0','-i']
            if '/' not in chosen:cmd+=['-b']
            cmd+=[chosen]
        t=time.perf_counter();selfcpu=time.process_time()
        p,metrics=run(cmd,env,metric_file)
        if tool!='plocate':assert p.returncode==0 or not p.stdout
        names=[os.fsdecode(s) for s in p.stdout.split(b'\0') if s]
        found=match(names,roots,q)
        metrics['ms']=(time.perf_counter()-t)*1000
        metrics['adapter_cpu_ms']=(time.process_time()-selfcpu)*1000
        return found,metrics
    def mcp_query(c,case):
        args={'query':case['query'],'limit':200}
        if len(case['roots'])==1:args['root']=str(case['roots'][0])
        # Dedicated session scopes below are exact for this case.
        selfcpu=time.process_time()
        ms,size,err,res=c.call('search_files',args)
        adapter_cpu_ms=(time.process_time()-selfcpu)*1000
        assert not err and res is not None and not res['truncated_by_size'] and not res.get('incomplete_due_to_changes',False), (case['name'],res)
        return {x['path'] for x in res['results']},dict(ms=ms,response_bytes=size,more_available=res['more_available'],adapter_cpu_ms=adapter_cpu_ms)
    for rep in range(a.repetitions):
        # Fresh independent indices; retain corpus and cache state. Only our data is removed.
        shutil.rmtree(work/'data',ignore_errors=True)
        p,m=run([binary/'ferroseek','index'],env,metric_file,indexing=True);assert p.returncode==0
        maintenance.append(dict(repetition=rep,tool='ferroseek',phase='initial',database_bytes=(work/'data/index.sqlite').stat().st_size,**m))
        if 'plocate' in tools:
            pl_db.unlink(missing_ok=True)
            p,m=build_plocate();assert p.returncode==0
            maintenance.append(dict(repetition=rep,tool='plocate',phase='initial',database_bytes=pl_db.stat().st_size,**m))
        for ci,case in enumerate(cases):
            expected=oracle(case['roots'],case['query'])
            assert len(expected)<200
            started=time.perf_counter()
            c=mb.Client([str(binary/'ferroseek-mcp'),'--index',str(work/'data/index.sqlite'),'--config',str(policy),
                         *[x for r in case['roots'] for x in ('--allow',str(r))]],env)
            init,_=c.request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'compare','version':'1'}})
            assert 'error' not in init
            c.notify('notifications/initialized')
            startup=(time.perf_counter()-started)*1000
            try:
                for tool in tools:
                    found,_=mcp_query(c,case) if tool=='mcp' else native(tool,case)
                    assert found==expected,(tool,case['name'],len(found),len(expected))
                cpu0=mb.cpu_ticks(c.p.pid);io0=process_io(c.p.pid)
                for sample in range(a.queries_per_case):
                    # Rotate first tool between repetitions and samples to avoid drift.
                    offset=(rep+sample)%len(tools)
                    for tool in tools[offset:]+tools[:offset]:
                        found,m=mcp_query(c,case) if tool=='mcp' else native(tool,case)
                        assert found==expected,(tool,case['name'],len(found),len(expected))
                        rows.append(dict(repetition=rep,case=case['name'],tool=tool,sample=sample,
                                         expected=len(expected),returned=len(found),recall=1.0,**m))
                io1=process_io(c.p.pid);st=mb.proc_status(c.p.pid)
                memory.append(dict(repetition=rep,case=case['name'],startup_ms=startup,rss_kib=st['VmRSS'],peak_rss_kib=st['VmHWM'],
                                   cpu_ms=1000*(mb.cpu_ticks(c.p.pid)-cpu0)/os.sysconf('SC_CLK_TCK'),
                                   io={k:io1[k]-io0[k] for k in io0}))
                if ci == len(cases)-1:
                    time.sleep(12)
                    st_idle=mb.proc_status(c.p.pid)
                    cpu_idle=mb.cpu_ticks(c.p.pid);ctx_idle=mb.ctx_switches(c.p.pid)
                    time.sleep(2)
                    memory.append(dict(repetition=rep,case='idle',rss_kib=st_idle['VmRSS'],threads=st_idle['Threads'],
                                       cpu_ms=1000*(mb.cpu_ticks(c.p.pid)-cpu_idle)/os.sysconf('SC_CLK_TCK'),
                                       context_switches=mb.ctx_switches(c.p.pid)-ctx_idle,seconds=2))
            finally:
                c.p.stdin.close();assert c.p.wait(timeout=10)==0
        # Add and remove names while no indexer is running: compare freshness, then refresh.
        added=repos[0]/'fresh-change.rs';removed=repos[0]/'src/group005/module000501.rs'
        added.write_text('new');saved=removed.read_bytes();removed.unlink()
        change=dict(name='changed',roots=[repos[0]],query='fresh-change.rs')
        c=mb.Client([str(binary/'ferroseek-mcp'),'--index',str(work/'data/index.sqlite'),'--config',str(policy),
                     '--allow',str(repos[0])],env)
        c.request('initialize',{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'freshness','version':'1'}})
        c.notify('notifications/initialized')
        try:
            for phase in ['before-refresh','after-refresh']:
                if phase=='after-refresh':
                    p,m=run([binary/'ferroseek','index','--full'],env,metric_file,indexing=True);assert p.returncode==0
                    maintenance.append(dict(repetition=rep,tool='ferroseek',phase='refresh',**m))
                    if 'plocate' in tools:
                        p,m=build_plocate();assert p.returncode==0
                        maintenance.append(dict(repetition=rep,tool='plocate',phase='refresh',**m))
                for case in [change,dict(name='deleted',roots=[repos[0]],query='module000501.rs')]:
                    expected=oracle(case['roots'],case['query'])
                    for tool in tools:
                        found,m=mcp_query(c,case) if tool=='mcp' else native(tool,case)
                        recall=len(found & expected)/len(expected) if expected else 1.0
                        if phase=='after-refresh' or tool in ['fd','rg']:assert found==expected
                        else:assert not found, 'stopped indexes must not invent new files; stale paths must be filtered'
                        freshness.append(dict(repetition=rep,case=case['name'],tool=tool,phase=phase,
                                              recall=recall,false_positives=len(found-expected),**m))
        finally:
            c.p.stdin.close();assert c.p.wait(timeout=10)==0
        added.unlink();removed.write_bytes(saved)
        print('repetition',rep+1,'complete',flush=True)
    result=dict(seed=0,files_per_repo=a.files_per_repo,repetitions=a.repetitions,queries_per_case=a.queries_per_case,
                tools=tools,rows=rows,maintenance=maintenance,memory=memory,freshness=freshness,
                errors=0,versions={tool:subprocess.check_output([tool,'--version'],text=True).splitlines()[0] for tool in tools if tool!='mcp'},
                notes='Warm filesystem cache. MCP resident stdio; CLI subprocess + Python predicate adapter. No content search. wait4 peak RSS can include the inherited Python pre-exec image; use the separate post-exec sampling harness for native memory.')
    a.output.write_text(json.dumps(result,indent=2)+'\n')


if __name__=='__main__':main()
