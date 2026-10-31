#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Benchmark ferroseek-mcp over its real stdio protocol.

    scripts/benchmark-mcp.py --bin target/release/ferroseek-mcp \
        --index ~/.cache/ferroseek-bench-1000000/data/index.sqlite \
        --allow ~/.cache/ferroseek-bench-1000000/corpus [--queries 300] [--seed 7]

Measures, independently of the daemon: startup (spawn → initialize reply),
tools/call round-trip latency per tool, response sizes, and the server's
memory/CPU while idle (from /proc). The index is only read. The user's MCP
config is never read (XDG_CONFIG_HOME points to an empty temp dir).
"""

import argparse
import json
import os
import random
import sqlite3
import statistics
import subprocess
import tempfile
import time


def proc_status(pid):
    out = {}
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            k, _, v = line.partition(":")
            parts = v.split()
            if parts and parts[0].isdigit():
                out[k] = int(parts[0])
    return out


def cpu_ticks(pid):
    with open(f"/proc/{pid}/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def ctx_switches(pid):
    total = 0
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            with open(f"/proc/{pid}/task/{tid}/status") as f:
                for line in f:
                    if "ctxt_switches" in line:
                        total += int(line.split()[1])
        except FileNotFoundError:
            pass
    return total


class Client:
    def __init__(self, cmd, env):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        self.next = 1

    def request(self, method, params):
        rid = self.next
        self.next += 1
        self.p.stdin.write((json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n").encode())
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            msg = json.loads(line)
            if msg.get("id") == rid:
                return msg, len(line)

    def notify(self, method):
        self.p.stdin.write((json.dumps({"jsonrpc": "2.0", "method": method}) + "\n").encode())
        self.p.stdin.flush()

    def call(self, tool, args):
        t = time.perf_counter()
        msg, size = self.request("tools/call", {"name": tool, "arguments": args})
        ms = (time.perf_counter() - t) * 1e3
        res = msg.get("result") or {}
        return ms, size, res.get("isError", False), res.get("structuredContent")


def pct(v, p):
    s = sorted(v)
    return s[round((len(s) - 1) * p)]


def lat(v):
    return f"p50 {pct(v, .5):.2f} / p95 {pct(v, .95):.2f} / p99 {pct(v, .99):.2f} / max {max(v):.2f} ms"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--index", required=True)
    ap.add_argument("--allow", required=True)
    ap.add_argument("--queries", type=int, default=300)
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()
    index, allow = os.path.expanduser(a.index), os.path.expanduser(a.allow)
    rng = random.Random(a.seed)

    # Query set from real indexed names (read-only connection).
    db = sqlite3.connect(f"file:{index}?mode=ro", uri=True)
    total = db.execute("SELECT max(id) FROM entries").fetchone()[0]
    names = []
    while len(names) < a.queries:
        row = db.execute("SELECT key FROM entries WHERE id = ?", (rng.randint(1, total),)).fetchone()
        if row and len(row[0]) >= 4 and not row[0].startswith("."):
            names.append(row[0])
    db.close()
    queries = []
    for n in names:
        k = min(len(n), rng.randint(3, 8))
        s = rng.randint(0, len(n) - k)
        queries.append(n[s:s + k])
    queries += [f"zq{rng.randint(0, 999)}x" for _ in range(30)]

    with tempfile.TemporaryDirectory() as cfg_home:
        env = dict(os.environ, XDG_CONFIG_HOME=cfg_home)
        t0 = time.perf_counter()
        c = Client([a.bin, "--index", index, "--allow", allow], env)
        c.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "bench", "version": "0"}})
        c.notify("notifications/initialized")
        startup = (time.perf_counter() - t0) * 1e3
        pid = c.p.pid
        time.sleep(1)
        st0 = proc_status(pid)

        for q in queries[:20]:
            c.call("search_files", {"query": q})  # warm caches
        search_ms, sizes, info_paths, results = [], [], [], 0
        for q in queries:
            ms, size, err, sc = c.call("search_files", {"query": q, "limit": 20})
            assert not err, (q, sc)
            search_ms.append(ms)
            sizes.append(size)
            results += len(sc["results"])
            info_paths += [e["path"] for e in sc["results"][:1] if not e.get("non_utf8")]
        root = os.path.join(allow, os.listdir(allow)[0])
        root_ms = [c.call("search_files", {"query": q, "root": root})[0] for q in queries[:100]]
        info_ms = []
        for p in info_paths[:200]:
            ms, _, err, _ = c.call("get_file_info", {"path": p})
            info_ms.append(ms)
        status_ms = [c.call("index_status", {})[0] for _ in range(20)]
        st1 = proc_status(pid)

        time.sleep(12)  # blocking-pool threads exit after 10 s idle
        c0, x0, t = cpu_ticks(pid), ctx_switches(pid), time.monotonic()
        time.sleep(30)
        secs = time.monotonic() - t
        cpu = (cpu_ticks(pid) - c0) / os.sysconf("SC_CLK_TCK")
        wake = (ctx_switches(pid) - x0) / secs
        st2 = proc_status(pid)
        c.p.stdin.close()
        c.p.wait(timeout=10)

    mib = lambda k: f"{k / 1024:.1f} MiB"
    print(f"| metric | value |\n|---|---|")
    print(f"| startup: spawn → `initialize` reply | {startup:.1f} ms |")
    print(f"| `search_files` (limit 20, {len(search_ms)} queries, {results} results) | {lat(search_ms)} |")
    print(f"| `search_files` with `root` filter ({len(root_ms)}) | {lat(root_ms)} |")
    print(f"| `get_file_info` ({len(info_ms)}) | {lat(info_ms)} |")
    print(f"| `index_status` ({len(status_ms)}) | {lat(status_ms)} |")
    print(f"| `search_files` response size (JSON-RPC line) | p50 {pct(sizes, .5) / 1024:.1f} KiB / max {max(sizes) / 1024:.1f} KiB |")
    print(f"| RSS after initialize (anon) | {mib(st0['VmRSS'])} ({mib(st0['RssAnon'])}), {st0['Threads']} threads |")
    print(f"| RSS after workload (anon), VmHWM | {mib(st1['VmRSS'])} ({mib(st1['RssAnon'])}), {mib(st1['VmHWM'])} |")
    print(f"| idle RSS 12 s later (anon) | {mib(st2['VmRSS'])} ({mib(st2['RssAnon'])}), {st2['Threads']} threads |")
    print(f"| idle CPU / context switches over {secs:.0f} s | {100 * cpu / secs:.3f} % / {wake:.2f} per s |")


if __name__ == "__main__":
    main()
