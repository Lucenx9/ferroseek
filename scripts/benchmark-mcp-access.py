#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Isolated warm-cache access-mode comparison, with an independent fixture oracle."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile

spec = importlib.util.spec_from_file_location('bench', Path(__file__).with_name('benchmark-mcp.py'))
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)


def measure(args):
    rows = []
    sides = ['baseline', 'restricted', 'all_indexed', 'home']
    with tempfile.TemporaryDirectory(prefix='ferro-access-') as tmp:
        work = Path(tmp)
        roots = [work / 'one', work / 'two']
        for root in roots:
            root.mkdir()
            for n in range(10000):
                (root / f'item{n:06}.txt').write_bytes(b'fixture')
        cfg = work / 'engine.toml'
        cfg.write_text('roots = ' + json.dumps([str(r) for r in roots]) +
                       '\nlow_priority = false\nmax_cpu_percent = 100\n')
        env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=str(work / 'config'),
                   FERROSEEK_CONFIG=str(cfg), FERROSEEK_DATA_DIR=str(work / 'data'),
                   DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-ferro-access')
        subprocess.run([str(Path('target/release/ferroseek').resolve()), 'index'],
                       env=env, capture_output=True, check=True)
        cases = [('item000123', None), ('item00012', None),
                 ('item009999', roots[0]), ('no-such-file', None)]
        expected = [
            {str(r / f'item{n:06}.txt') for r in roots if scope is None or r == scope
             for n in range(10000) if query in f'item{n:06}.txt'}
            for query, scope in cases
        ]
        requests = [dict(query=query, limit=100, **({'root': str(scope)} if scope else {}))
                    for query, scope in cases]
        for rep in range(5):
            for side in sides[rep % 4:] + sides[:rep % 4]:
                binary = args.baseline if side == 'baseline' else args.candidate
                cmd = [str(binary.resolve()), '--index', str(work / 'data/index.sqlite')]
                if side != 'baseline':
                    cmd += ['--access-mode', side]
                if side in ['baseline', 'restricted']:
                    cmd += ['--allow', tmp]
                client = b.Client(cmd, env)
                try:
                    client.request('initialize', {
                        'protocolVersion': '2025-06-18', 'capabilities': {},
                        'clientInfo': {'name': 'access-bench', 'version': '1'},
                    })
                    client.notify('notifications/initialized')
                    for request in requests:
                        client.call('search_files', request)
                    cpu = b.cpu_ticks(client.p.pid)
                    for sample in range(10):
                        for case, request in enumerate(requests):
                            ms, _, err, res = client.call('search_files', request)
                            assert not err and not res['truncated_by_size']
                            assert not res.get('incomplete_due_to_changes', False)
                            assert {x['path'] for x in res['results']} == expected[case]
                            rows.append(dict(repetition=rep, side=side, case=case, sample=sample, ms=ms))
                    rows.append(dict(
                        repetition=rep, side=side, metric='resources',
                        cpu_ms=1000 * (b.cpu_ticks(client.p.pid) - cpu) / os.sysconf('SC_CLK_TCK'),
                        rss_kib=b.proc_status(client.p.pid)['VmRSS'],
                    ))
                finally:
                    client.p.stdin.close()
                    assert client.p.wait(timeout=10) == 0
    return dict(
        files=20000, repetitions=5, rows=rows, correctness_mismatches=0,
        baseline_sha256=hashlib.sha256(args.baseline.read_bytes()).hexdigest(),
        candidate_sha256=hashlib.sha256(args.candidate.read_bytes()).hexdigest(),
        note='Warm filesystem cache, fresh MCP processes, rotated order, 40 timed stdio round trips '
             'per process; independent expected sets. No live daemon or index used.',
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, default=Path('target/release/ferroseek-mcp'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('output already exists; choose a new file')
    result = measure(args)
    with args.output.open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')
    for side in ['baseline', 'restricted', 'all_indexed', 'home']:
        medians = [statistics.median(r['ms'] for r in result['rows']
                                    if r['side'] == side and r['repetition'] == rep and 'ms' in r)
                   for rep in range(5)]
        print(side, 'median of run medians ms', round(statistics.median(medians), 3),
              'range', round(min(medians), 3), round(max(medians), 3))


if __name__ == '__main__':
    main()
