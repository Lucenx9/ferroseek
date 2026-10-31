#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Five fresh index builds and explicit rename refreshes on a benchmark-only corpus."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3

spec=importlib.util.spec_from_file_location('comparison',Path(__file__).with_name('benchmark-search-tools.py'))
b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--work',type=Path,required=True)
p.add_argument('--bin-dir',type=Path,default=Path('target/release'))
p.add_argument('--output',type=Path,required=True)
a=p.parse_args();work=a.work.resolve();assert (work/'repo0').is_dir() and (work/'mcp.toml').is_file() and not a.output.exists()
env=dict(os.environ,FERROSEEK_CONFIG=str(work/'config.toml'),FERROSEEK_DATA_DIR=str(work/'data'),DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferroseek-benchmark')
pldb=work/'plocate.db';cli=a.bin_dir.resolve()/'ferroseek'
probe=work/'repo0'/'maintenance-before.rs';moved=probe.with_name('maintenance-after.rs')
assert not probe.exists() and not moved.exists()
commands={'ferroseek':[cli,'index'], 'plocate':['updatedb','--database-root',work,'--output',pldb,'--require-visibility','no','--prunepaths',str(work/'data'),'--prunenames','.git','--prunefs','','--prune-bind-mounts','no']}
rows=[]
for rep in range(5):
    shutil.rmtree(work/'data');pldb.unlink(missing_ok=True)
    probe.write_bytes(b'fixture')
    order=['ferroseek','plocate'] if rep%2==0 else ['plocate','ferroseek']
    for phase in ['initial','rename-refresh']:
        if phase=='rename-refresh':probe.rename(moved)
        for tool in order:
            cmd=commands[tool]+(['--full'] if tool=='ferroseek' and phase=='rename-refresh' else [])
            result,m=b.run(cmd,env,work/'unused',indexing=tool=='ferroseek');assert result.returncode==0
            db=work/'data/index.sqlite' if tool=='ferroseek' else pldb
            if tool=='ferroseek':
                with sqlite3.connect(db.as_uri()+'?mode=ro',uri=True) as conn:
                    expected_name=probe.name if phase=='initial' else moved.name
                    assert conn.execute('SELECT count(*) FROM entries WHERE key=?',(expected_name,)).fetchone()[0]==1
            else:
                import subprocess
                found=subprocess.check_output(['plocate','-d',str(pldb),'-0','maintenance-'])
                assert os.fsencode(probe if phase=='initial' else moved) in found.split(b'\0')
            rows.append(dict(repetition=rep,tool=tool,phase=phase,database_bytes=db.stat().st_size,**m))
    moved.unlink()
    print(rep+1,'fresh index costs complete',flush=True)
a.output.write_text(json.dumps(dict(rows=rows,note='Both indexes removed before each initial run. Refresh applies one rename. Foreground FerroSeek full reconcile versus updatedb, not per-event watcher cost.'),indent=2)+'\n')
