#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Test discovery evaluation fixtures/proxy without credentials or model calls."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    evaluation = load('evaluation', repo / 'scripts/benchmark-mcp-discovery.py')
    bench = load('bench', repo / 'scripts/benchmark-mcp.py')
    with tempfile.TemporaryDirectory(prefix='ferro-discovery-smoke-') as tmp:
        work = Path(tmp)
        corpus, database, cases = evaluation.fixture(work)
        for prompt, expected in cases.values():
            assert not any(word in prompt.lower() for word in ['ferroseek', 'mcp', 'ripgrep', 'codex', 'claude'])
            assert all((corpus / path).is_file() for path in expected)
        env = dict(os.environ, HOME=tmp, XDG_CONFIG_HOME=str(work / 'empty-config'))
        log = work / 'trace.jsonl'
        client = bench.Client([
            sys.executable, str(evaluation.SCRIPT), '--proxy', str(log),
            str(repo / 'target/release/ferroseek-mcp'), str(database), str(corpus),
        ], env)
        try:
            init, _ = client.request('initialize', {
                'protocolVersion': '2025-11-25', 'capabilities': {},
                'clientInfo': {'name': 'discovery-smoke', 'version': '1'},
            })
            assert init['result']['serverInfo']['name'] == 'ferroseek'
            client.notify('notifications/initialized')
            listed, _ = client.request('tools/list', {})
            assert {t['name'] for t in listed['result']['tools']} == {'search_files', 'get_file_info', 'index_status'}
            for case, query in [('name', 'orbit-config.toml'), ('multi_repo', 'release-notes.md'),
                                ('partial_path', 'services/payments/')]:
                _, _, error, response = client.call('search_files', dict(query=query, limit=100))
                assert not error
                assert {p['path'] for p in response['results']} == {str(corpus / p) for p in cases[case][1]}
            for query in ['fresh-memo.txt', 'archive-map.json']:
                _, _, error, response = client.call('search_files', dict(query=query))
                assert not error and response['results'] == [], 'fixture must distinguish live and indexed coverage'
        finally:
            client.p.stdin.close()
            assert client.p.wait(timeout=10) == 0
        trace = evaluation.json_lines(log)
        requests = {e['message']['id']: e for e in trace if e['direction'] == 'request' and 'id' in e['message']}
        for response in [e for e in trace if e['direction'] == 'response']:
            assert response['time'] >= requests[response['message']['id']]['time']
        assert len(requests) == 7
        assert len([e for e in trace if e['direction'] == 'response']) == 7
        # Current Claude Code uses stateless discovery, while Codex still
        # initializes a legacy session. Both must expose the same guidance.
        modern = bench.Client([
            sys.executable, str(evaluation.SCRIPT), '--proxy', str(work / 'modern.jsonl'),
            str(repo / 'target/release/ferroseek-mcp'), str(database), str(corpus),
        ], env)
        meta = {
            'io.modelcontextprotocol/protocolVersion': '2026-07-28',
            'io.modelcontextprotocol/clientInfo': {'name': 'discovery-smoke', 'version': '1'},
            'io.modelcontextprotocol/clientCapabilities': {},
        }
        try:
            discovery, _ = modern.request('server/discover', {'_meta': meta})
            assert '2026-07-28' in discovery['result']['supportedVersions']
            assert discovery['result']['instructions'] == init['result']['instructions']
            modern_tools, _ = modern.request('tools/list', {'_meta': meta})
            assert modern_tools['result']['tools'] == listed['result']['tools']
            response, _ = modern.request('tools/call', {
                '_meta': meta, 'name': 'search_files', 'arguments': {'query': 'orbit-config.toml'},
            })
            assert not response['result'].get('isError', False)
            assert [r['path'] for r in response['result']['structuredContent']['results']] == [
                str(corpus / 'alpha/config/orbit-config.toml')]
        finally:
            modern.p.stdin.close()
            assert modern.p.wait(timeout=10) == 0
    print('PASS: neutral prompts, independent fixture oracle, indexed/live distinction and transparent MCP trace')


if __name__ == '__main__':
    main()
