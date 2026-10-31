#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Protocol smoke test for ferroseek-mcp (stdio).

    tests/mcp-smoke.py [target/release/ferroseek-mcp] [target/release/ferroseek]

Builds a tiny index in a temp dir with the real CLI, then checks MCP
initialization with several protocol versions, tools/list and one call per
tool. Uses only temp dirs and a dead D-Bus address.
"""

import json
import os
import subprocess
import sys
import tempfile

MCP = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/ferroseek-mcp")
CLI = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else "target/release/ferroseek")


def session(args, env, version):
    p = subprocess.Popen([MCP] + args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    msgs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "smoke", "version": "0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "search_files", "arguments": {"query": "smoke"}}},
        {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "index_status", "arguments": {}}},
    ]
    out, err = p.communicate("".join(json.dumps(m) + "\n" for m in msgs).encode(), timeout=20)
    assert p.returncode == 0, err.decode()
    replies = {m["id"]: m for m in map(json.loads, out.decode().splitlines()) if "id" in m}
    return replies, err.decode()


def main():
    with tempfile.TemporaryDirectory() as t:
        root = os.path.join(t, "files", "docs")
        os.makedirs(root)
        open(os.path.join(root, "smoke-test-file.txt"), "w").write("x")
        open(os.path.join(t, "config.toml"), "w").write(f'roots = ["{os.path.join(t, "files")}"]\n')
        env = dict(os.environ, FERROSEEK_CONFIG=os.path.join(t, "config.toml"), FERROSEEK_DATA_DIR=os.path.join(t, "data"),
                   XDG_CONFIG_HOME=os.path.join(t, "xdg"), DBUS_SESSION_BUS_ADDRESS=f"unix:path={t}/no-bus")
        subprocess.run([CLI, "index"], env=env, check=True, capture_output=True)
        for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]:
            r, stderr = session(["--allow", os.path.join(t, "files")], env, version)
            assert r[1]["result"]["serverInfo"]["name"] == "ferroseek", r[1]
            negotiated = r[1]["result"]["protocolVersion"]
            names = sorted(tool["name"] for tool in r[2]["result"]["tools"])
            assert names == ["get_file_info", "index_status", "search_files"], names
            hits = r[3]["result"]["structuredContent"]["results"]
            assert [os.path.basename(h["path"]) for h in hits] == ["smoke-test-file.txt"], hits
            assert r[4]["result"]["structuredContent"]["available"] is True
            assert stderr == "", stderr  # nothing unexpected on stderr
            print(f"client {version} → negotiated {negotiated}: OK")
    print("PASS")


if __name__ == "__main__":
    main()
