#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# End-to-end smoke test of ferroseekd on a private session bus.
# Uses only temp directories; never touches the real session bus or index.
#
#   tests/dbus-smoke.sh [path/to/ferroseekd]
#
# Re-executes itself under dbus-run-session if not already isolated.
set -euo pipefail

BIN=${1:-target/debug/ferroseekd}
BIN=$(realpath "$BIN")
if [[ "${FERROSEEK_SMOKE_ISOLATED:-}" != 1 ]]; then
    exec env FERROSEEK_SMOKE_ISOLATED=1 dbus-run-session -- "$0" "$BIN"
fi

T=$(mktemp -d)
trap 'kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; rm -rf "$T"' EXIT
mkdir -p "$T/root/Documents" "$T/root:colon" "$T/data"
echo hi > "$T/root/Documents/Quarterly Report.pdf"
echo hi > "$T/root/Documents/notes-2024.md"
echo hi > "$T/root:colon/colon-file.txt"
printf 'roots = ["%s", "%s"]\ndebounce_ms = 20\n' "$T/root" "$T/root:colon" > "$T/config.toml"

mkdir -p "$T/bin" "$T/runtime"
chmod 700 "$T/runtime"
cat > "$T/bin/xdg-open" <<'SH'
#!/bin/sh
printf '%s\n' "$XDG_ACTIVATION_TOKEN" "$#" "$1" > "$TEST_OPEN_LOG"
SH
chmod +x "$T/bin/xdg-open"
export PATH="$T/bin:$PATH" XDG_RUNTIME_DIR="$T/runtime" TEST_OPEN_LOG="$T/open-log"

export FERROSEEK_CONFIG="$T/config.toml" FERROSEEK_DATA_DIR="$T/data" FERROSEEK_LOG=debug
"$BIN" 2> "$T/daemon.log" &
PID=$!

SVC=io.github.ferroseek.FerroSeek
call() { gdbus call --session --dest "$SVC" "$@"; }
fail() { echo "FAIL: $*"; echo "--- daemon log"; cat "$T/daemon.log"; exit 1; }
file_status() { call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.FileSearchStatus; }

for _ in $(seq 100); do
    call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status \
        2>/dev/null | grep -q "'state': 'idle'" && break
    sleep 0.05
done

echo "== Status"
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status

echo "== Typed file-search status"
gdbus introspect --session --dest "$SVC" --object-path /io/github/ferroseek/FerroSeek --xml > "$T/control.xml"
python3 - "$T/control.xml" <<'PY'
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
interface = root.find("./interface[@name='io.github.ferroseek.FerroSeek1']")
assert interface is not None, "control interface missing"
method = interface.find("./method[@name='FileSearchStatus']")
assert method is not None, "FileSearchStatus missing"
args = method.findall("arg")
assert [arg.attrib["type"] for arg in args if arg.attrib.get("direction") == "out"] == [
    "s", "t", "t", "t", "b", "t", "as"
], "FileSearchStatus output signature"
assert not [arg for arg in args if arg.attrib.get("direction") == "in"], "FileSearchStatus takes no arguments"
PY
out=$(file_status)
echo "$out"
[[ "$out" == "('idle', uint64 3, uint64 3, "* ]] || fail "status live counts/state"
[[ "$out" == *"'$T/root'"* && "$out" == *"'$T/root:colon'"* ]] || fail "status root array lost a path or split a colon"
[[ "$(file_status)" == "$out" ]] || fail "status read changed idle state"

echo "== Search 'report'"
out=$(call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "report")
echo "$out"
[[ "$out" == *"Quarterly Report.pdf"* ]] || fail "Match did not find the file"
[[ "$out" == *"file://$T/root/Documents/Quarterly%20Report.pdf"* ]] || fail "match id is not a file URL"
[[ "$out" == *"application-pdf"* ]] || fail "icon"

echo "== Live update"
echo x > "$T/root/brand-new-file.txt"
for _ in $(seq 100); do
    out=$(call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "brand-new")
    [[ "$out" == *brand-new-file.txt* ]] && break
    sleep 0.05
done
[[ "$out" == *brand-new-file.txt* ]] || fail "live update not visible"
echo "$out"
for _ in $(seq 100); do
    out=$(file_status)
    [[ "$out" == "('idle', uint64 4, uint64 3, "* ]] && break
    sleep 0.01
done
[[ "$out" == "('idle', uint64 4, uint64 3, "* ]] || fail "status did not report live file count"

echo "== Short and empty queries"
empty() { [[ "$1" == *"[]"* && "$1" != *"file://"* ]]; }
empty "$(call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "//")" || fail "slash-only query"
empty "$(call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "q")" || fail "1-char query"
empty "$(call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "zzzzqqq")" || fail "no-hit query"

echo "== Concurrent independent searches"
for i in $(seq 20); do call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Search "notes" > "$T/c$i" & done
wait $(jobs -p | grep -v "^$PID$") 2>/dev/null || true
for i in $(seq 20); do grep -q "notes-2024.md" "$T/c$i" || fail "concurrent call $i"; done

echo "== Open on a path that is not indexed is refused"
if call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Open "file:///etc/passwd" "" "" > "$T/open-out" 2>&1; then
    fail "unindexed Open succeeded"
fi
grep -q "not in the index" "$T/open-out" || fail "unindexed Open not refused"

echo "== Open indexed URL with activation token and a single path argument"
url="file://$T/root/Documents/Quarterly%20Report.pdf"
call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Open "$url" "" "test-token" > /dev/null
for _ in $(seq 100); do [[ -f "$T/open-log" ]] && break; sleep 0.01; done
[[ "$(cat "$T/open-log")" == "$(printf 'test-token\n1\n%s' "$T/root/Documents/Quarterly Report.pdf")" ]] || fail "indexed Open argument/token forwarding"
for pair in invalid-url unknown-action; do
    if [[ "$pair" == invalid-url ]]; then id="https://example.com"; action=""; else id="$url"; action="unknown"; fi
    if call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Open "$id" "$action" "" > /dev/null 2>&1; then
        fail "$pair was accepted"
    fi
done

echo "== Removed KRunner interface is unavailable"
if call --object-path /runner --method org.kde.krunner1.Match "report" > /dev/null 2>&1; then
    fail "removed interface is still served"
fi

echo "== Release, Pause/Resume"
call --object-path /io/github/ferroseek/Search --method io.github.ferroseek.Search1.Release >/dev/null
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Pause >/dev/null
for _ in $(seq 100); do
    call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status | grep -q "'state': 'paused'" && break
    sleep 0.01
done
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status | grep -q "'state': 'paused'" || fail "pause"
out=$(file_status)
[[ "$out" == "('paused', uint64 4, uint64 3, "* ]] || fail "typed paused status"
echo paused > "$T/root/created-while-paused.txt"
sleep 0.1
for _ in $(seq 3); do
    [[ "$(file_status)" == "$out" ]] || fail "status read mutated or resumed a paused index"
done
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status | grep -q "'state': 'paused'" || fail "status read resumed engine"
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Resume >/dev/null
for _ in $(seq 100); do
    out=$(file_status)
    [[ "$out" == "('idle', uint64 5, uint64 3, "* ]] && break
    sleep 0.05
done
[[ "$out" == "('idle', uint64 5, uint64 3, "* ]] || fail "resume did not reconcile after read-only status"

echo "== Introspection"
gdbus introspect --session --dest "$SVC" --object-path /io/github/ferroseek/Search | grep -E "method|out|in " | head -20

echo "== SIGTERM shutdown"
kill -TERM "$PID"; wait "$PID"; rc=$?
[[ $rc == 0 ]] || fail "exit code $rc"
grep -q "shutting down" "$T/daemon.log" || fail "no graceful shutdown"
echo "PASS"
