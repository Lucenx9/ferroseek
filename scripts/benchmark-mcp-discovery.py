#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Fresh authenticated CLI sessions on synthetic data; never uses the live index.

Runs paid/subscription model requests. Credentials are copied into private
short-lived homes, never printed or included in reports. Raw client logs remain
in the requested private output directory. Run only when explicitly requested.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib

SCRIPT = Path(__file__).resolve()
REPO = SCRIPT.parents[1]
PRIMARY_CASES = ['name', 'multi_repo', 'partial_path', 'content', 'recent', 'unindexed', 'ambiguous']
GUIDANCE = ('For filename or partial-path discovery across projects, consider FerroSeek search_files '
            'when available. It only covers authorized indexed paths. Use rg for file contents and '
            'direct filesystem tools for new/unindexed files or exhaustive live results. '
            'An empty index result does not prove absence.\n')


def proxy(log, binary, database, root):
    """Transparent stdio forwarding; record traffic/timing, never alter metadata."""
    child = subprocess.Popen([binary, '--index', database, '--allow', root],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    lock = threading.Lock()
    with open(log, 'a') as trace:
        def record(direction, line):
            try:
                value = json.loads(line)
            except ValueError:
                return
            with lock:
                trace.write(json.dumps(dict(direction=direction, time=time.monotonic(), message=value)) + '\n')
                trace.flush()

        def responses():
            for line in child.stdout:
                record('response', line)
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
        worker = threading.Thread(target=responses, daemon=True)
        worker.start()
        try:
            for line in sys.stdin.buffer:
                record('request', line)
                child.stdin.write(line)
                child.stdin.flush()
        finally:
            child.stdin.close()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.terminate()
                child.wait(timeout=5)
            worker.join(timeout=5)


def fixture(work):
    corpus = work / 'corpus'
    files = {
        'alpha/config/orbit-config.toml': 'enabled = true\n',
        'alpha/release-notes.md': 'Alpha release\n',
        'beta/release-notes.md': 'Beta release\n',
        'gamma/release-notes.md': 'Gamma release\n',
        'gamma/services/payments/retry-policy.yaml': 'attempts: 3\n',
        'beta/src/retry.rs': 'const RETRY_WINDOW_LIMIT: usize = 42;\n',
        'alpha/README.md': 'Alpha project\n',
        'beta/docs/RETRY_WINDOW_LIMIT.md': 'A filename decoy, without the requested source text.\n',
    }
    for name, content in files.items():
        path = corpus / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    roots = [corpus / name for name in ['alpha', 'beta', 'gamma']]
    for root in roots:
        subprocess.run(['git', 'init', '-q', str(root)], check=True, capture_output=True)
    cfg = work / 'engine.toml'
    cfg.write_text('roots = ' + json.dumps([str(p) for p in roots]) + '\nlow_priority = false\n')
    env = dict(os.environ, HOME=str(work), XDG_CONFIG_HOME=str(work / 'xdg'),
               FERROSEEK_CONFIG=str(cfg), FERROSEEK_DATA_DIR=str(work / 'index'),
               DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-discovery-eval')
    subprocess.run([str(REPO / 'target/release/ferroseek'), 'index'], env=env,
                   check=True, capture_output=True)
    # These remain deliberately absent from the frozen index.
    (corpus / 'alpha/fresh-memo.txt').write_text('New memo\n')
    (corpus / 'scratch').mkdir()
    (corpus / 'scratch/archive-map.json').write_text('{}\n')
    cases = {
        'name': (f'Locate the file named orbit-config.toml under {corpus}. Give its path.',
                 ['alpha/config/orbit-config.toml']),
        'multi_repo': (f'Find every release-notes.md across the repositories under {corpus}. Give their paths.',
                       ['alpha/release-notes.md', 'beta/release-notes.md', 'gamma/release-notes.md']),
        'partial_path': (f'Find the files whose path contains services/payments/ under {corpus}. Give their paths.',
                         ['gamma/services/payments/retry-policy.yaml']),
        'content': (f'Which Rust source files under {corpus} contain RETRY_WINDOW_LIMIT? Give paths and matching lines.',
                    ['beta/src/retry.rs']),
        'recent': (f'I just created fresh-memo.txt somewhere under {corpus}. Locate it and give its path.',
                   ['alpha/fresh-memo.txt']),
        'unindexed': (f'Locate archive-map.json in {corpus}/scratch. Give its path.',
                      ['scratch/archive-map.json']),
        'ambiguous': ('Locate README.md in the current repository and give its path.', ['alpha/README.md']),
        'unknown_location': ("Find the path to retry-policy.yaml; I don't remember which project contains it.",
                             ['gamma/services/payments/retry-policy.yaml']),
    }
    return corpus, work / 'index/index.sqlite', cases


def json_lines(path):
    values = []
    if path.exists():
        for line in path.read_text(errors='replace').splitlines():
            try:
                values.append(json.loads(line))
            except ValueError:
                pass
    return values


def run_session(client, variant, binary, corpus, database, case, prompt, expected, args, auth_home):
    label = f'{client}-{variant}-{case}-{args.current_rep}'
    logdir = args.output / label
    logdir.mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix='ferro-agent-') as temp:
        home = Path(temp)
        for d in ['.codex', '.claude', '.config']:
            (home / d).mkdir(mode=0o700)
        auth = '.codex/auth.json' if client == 'codex' else '.claude/.credentials.json'
        if (auth_home / auth).is_file():
            shutil.copyfile(auth_home / auth, home / auth)
            (home / auth).chmod(0o600)
        if args.guidance:
            (home / ('.codex/AGENTS.md' if client == 'codex' else '.claude/CLAUDE.md')).write_text(GUIDANCE)
        env = dict(os.environ, HOME=str(home), CODEX_HOME=str(home / '.codex'),
                   CLAUDE_CONFIG_DIR=str(home / '.claude'), XDG_CONFIG_HOME=str(home / '.config'),
                   XDG_DATA_HOME=str(home / '.local/share'), XDG_STATE_HOME=str(home / '.local/state'),
                   DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-discovery-eval')
        for key in ['CLAUDECODE', 'CLAUDE_CODE_ENTRYPOINT', 'CODEX_THREAD_ID', 'FERROSEEK_CONFIG', 'FERROSEEK_DATA_DIR']:
            env.pop(key, None)
        proxy_args = [str(SCRIPT), '--proxy', str(logdir / 'mcp.jsonl'), str(binary), str(database), str(corpus)]
        registration = dict(command=sys.executable, args=proxy_args)
        if client == 'codex':
            # Reuse the user's chosen model/effort only, not prompts, skills, hooks or servers.
            original = auth_home / '.codex/config.toml'
            settings = tomllib.loads(original.read_text()) if original.exists() else {}
            if settings.get('model_provider', 'openai') != 'openai':
                raise ValueError('custom Codex providers require explicit isolated setup; refusing to substitute one')
            config = ''.join(f'{key} = {json.dumps(settings[key])}\n' for key in
                             ['model', 'model_reasoning_effort'] if key in settings)
            config += ('[mcp_servers.ferroseek]\ncommand = ' + json.dumps(sys.executable) +
                       '\nargs = ' + json.dumps(proxy_args) + '\nrequired = true\n')
            (home / '.codex/config.toml').write_text(config)
            cmd = ['codex', 'exec', '--ephemeral', '--json', '--sandbox', 'read-only',
                   '--skip-git-repo-check', '--ignore-rules', prompt]
        else:
            original = auth_home / '.claude/settings.json'
            settings = json.loads(original.read_text()) if original.exists() else {}
            minimal = {k: settings[k] for k in ['model'] if k in settings}
            minimal['autoMemoryEnabled'] = False
            (home / '.claude/settings.json').write_text(json.dumps(minimal))
            cfg = home / 'mcp.json'
            cfg.write_text(json.dumps(dict(mcpServers={'ferroseek': registration})))
            cmd = ['claude', '-p', '--output-format', 'stream-json', '--verbose', '--no-session-persistence',
                   '--strict-mcp-config', '--mcp-config', str(cfg), '--permission-mode', 'dontAsk',
                   '--allowedTools', 'Read,Glob,Grep,Bash,ToolSearch,mcp__ferroseek__*',
                   '--disable-slash-commands', prompt]
        start = time.monotonic()
        with (logdir / 'client.jsonl').open('w') as out, (logdir / 'stderr.txt').open('w') as err:
            child = subprocess.Popen(cmd, cwd=corpus / 'alpha', env=env, stdout=out, stderr=err,
                                     stdin=subprocess.DEVNULL, start_new_session=True)
            try:
                code = child.wait(timeout=args.timeout)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
                code = 'timeout'
        elapsed = time.monotonic() - start
    # Keep private raw evidence, but export only synthetic tool events for review.
    trace = json_lines(logdir / 'mcp.jsonl')
    requests = {str(e['message']['id']): e for e in trace
                if e['direction'] == 'request' and 'id' in e['message']}
    selected = any(e['message'].get('method') == 'tools/call' for e in requests.values())
    calls, listed, initialized, discovered = [], False, False, False
    for event in trace:
        if event['direction'] != 'response':
            continue
        response = event['message']
        request = requests.get(str(response.get('id')))
        if request is None:
            continue
        msg = request['message']
        ok = 'result' in response
        initialized |= msg.get('method') == 'initialize' and ok
        discovered |= msg.get('method') == 'server/discover' and ok
        listed |= msg.get('method') == 'tools/list' and ok
        if msg.get('method') == 'tools/call':
            result = response.get('result', {})
            calls.append(dict(tool=msg['params']['name'], arguments=msg['params'].get('arguments', {}),
                              ms=(event['time'] - request['time']) * 1000,
                              error=not ok or result.get('isError', False)))
    events = json_lines(logdir / 'client.jsonl')
    tools, final, model, provider_error = [], [], None, False
    for event in events:
        if client == 'claude':
            if event.get('type') == 'system':
                model = event.get('model', model)
            if event.get('type') == 'assistant':
                for block in event.get('message', {}).get('content', []):
                    if block.get('type') == 'tool_use':
                        tools.append(dict(tool=block['name'], arguments=block.get('input', {})))
            if event.get('type') == 'result':
                final.append(event.get('result', ''))
                provider_error |= event.get('is_error', False)
        else:
            item = event.get('item', {})
            if event.get('type') == 'item.completed':
                if item.get('type') == 'agent_message':
                    final.append(item.get('text', ''))
                elif item.get('type') in ['command_execution', 'mcp_tool_call', 'tool_search']:
                    tools.append(item)
            provider_error |= event.get('type') in ['error', 'turn.failed']
    answer = final[-1] if final else ''
    # Candidate success only: manually verify answers and extra/missing results afterward.
    hits = [p for p in expected if p in answer or str(corpus / p) in answer]
    record = dict(client=client, variant=variant, case=case, repetition=args.current_rep,
                  guidance=args.guidance, returncode=code, session_seconds=elapsed,
                  initialized=initialized, server_discovered=discovered, tools_listed=listed, selected=selected,
                  mcp_calls=calls, agent_tools=tools, model=model, provider_error=provider_error,
                  expected=expected, matched_expected=hits, answer=answer)
    # Absolute synthetic temp paths become stable placeholders in public evidence.
    return json.loads(json.dumps(record).replace(str(corpus), '$CORPUS').replace(str(logdir), '$LOG'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--clients', nargs='+', choices=['claude', 'codex'], default=['claude', 'codex'])
    parser.add_argument('--variants', nargs='+', choices=['baseline', 'improved'], default=['baseline', 'improved'])
    parser.add_argument('--cases', nargs='+', choices=PRIMARY_CASES + ['unknown_location'], default=PRIMARY_CASES)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--timeout', type=int, default=90)
    parser.add_argument('--guidance', action='store_true')
    args = parser.parse_args()
    if args.repetitions < 1 or args.timeout < 1:
        parser.error('repetitions and timeout must be positive')
    args.output = args.output.resolve()
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    args.baseline = args.baseline.resolve()
    args.candidate = args.candidate.resolve()
    auth_home = Path.home()
    rows = []
    with tempfile.TemporaryDirectory(prefix='ferro-discovery-') as temp:
        frozen = Path(temp) / 'binaries'
        frozen.mkdir()
        for key in ['baseline', 'candidate']:
            destination = frozen / key
            shutil.copy2(getattr(args, key), destination)
            setattr(args, key, destination)
        binary_hashes = {key + '_sha256': hashlib.sha256(getattr(args, key).read_bytes()).hexdigest()
                         for key in ['baseline', 'candidate']}
        corpus, database, cases = fixture(Path(temp))
        for rep in range(args.repetitions):
            args.current_rep = rep
            for case in args.cases or cases:
                prompt, expected = cases[case]
                for client in args.clients:
                    variants = [('baseline', args.baseline), ('improved', args.candidate)]
                    variants = [(name, binary) for name, binary in variants if name in args.variants]
                    if rep % 2:
                        variants.reverse()
                    for variant, binary in variants:
                        record = run_session(client, variant, binary, corpus, database,
                                             case, prompt, expected, args, auth_home)
                        rows.append(record)
                        (args.output / 'results.json').write_text(json.dumps(rows, indent=2) + '\n')
                        print(client, variant, case, rep, 'exit', record['returncode'],
                              'listed', record['tools_listed'], 'selected', record['selected'],
                              'answer matches', len(record['matched_expected']), '/', len(expected), flush=True)
    metadata = dict(repetitions=args.repetitions, guidance=args.guidance,
                    **binary_hashes,
                    versions={c: subprocess.check_output([c, '--version'], text=True).strip() for c in args.clients})
    (args.output / 'environment.json').write_text(json.dumps(metadata, indent=2) + '\n')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--proxy':
        proxy(*sys.argv[2:])
    else:
        main()
