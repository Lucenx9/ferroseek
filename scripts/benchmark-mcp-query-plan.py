#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Read-only query-plan evidence for the generated million-file corpus."""
import argparse
import json
from pathlib import Path
import random
import sqlite3
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--work', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
assert (a.work / 'report.md').is_file() and not a.output.exists()
with sqlite3.connect((a.work.resolve() / 'data/index.sqlite').as_uri() + '?mode=ro', uri=True) as db:
    maximum = db.execute('SELECT max(id) FROM entries').fetchone()[0]
    rng = random.Random(7)
    queries = []
    while len(queries) < 100:
        row = db.execute('SELECT key FROM entries WHERE id=?', (rng.randint(1, maximum),)).fetchone()
        if not row or len(row[0]) < 4 or row[0].startswith('.'):
            continue
        name = row[0]
        n = min(len(name), rng.randint(3, 8))
        start = rng.randint(0, len(name) - n)
        queries.append(name[start:start+n])
    scope = db.execute("SELECT id FROM entries WHERE key='personal' AND kind=1 AND parent IN (SELECT id FROM entries WHERE parent IS NULL)").fetchall()
    assert len(scope) == 1
    cte = 'WITH RECURSIVE dirs(id) AS (SELECT ? UNION SELECT e.id FROM entries e INDEXED BY entries_parent_name JOIN dirs ON e.parent=dirs.id WHERE e.kind=1) '
    directories = db.execute(cte + 'SELECT count(*) FROM dirs', scope[0]).fetchone()[0]
    rows = []
    for i, query in enumerate(queries):
        phrase = '"' + query.replace('"', '""') + '"'
        start = time.perf_counter()
        count = db.execute('SELECT count(*) FROM names WHERE names MATCH ?', (phrase,)).fetchone()[0]
        rows.append(dict(query=i, global_fts_matches=count, count_ms=1000*(time.perf_counter()-start)))
    cols = 'e.id,e.parent,e.name,e.key,e.kind,e.size,e.mtime'
    plans = {}
    for label, sql, params in [
        ('exact', f'SELECT {cols} FROM entries e WHERE e.key=? ORDER BY e.id', ('example',)),
        ('prefix', f'SELECT {cols} FROM entries e WHERE e.key>=? AND e.key<? ORDER BY e.key,e.id', ('example', 'example\U0010ffff')),
        ('fts', f'SELECT {cols} FROM names JOIN entries e ON e.id=names.rowid WHERE names MATCH ? ORDER BY names.rowid', ('"example"',)),
        ('descendants', cte + 'SELECT id FROM dirs', scope[0]),
    ]:
        plans[label] = [r[3] for r in db.execute('EXPLAIN QUERY PLAN ' + sql, params)]
a.output.write_text(json.dumps(dict(scope_directories=directories, plans=plans, rows=rows,
    note='Diagnostic counts are not search latency: candidate caps can stop before all FTS matches are read. Query IDs match the seed-7 paired benchmark.'), indent=2) + '\n')
