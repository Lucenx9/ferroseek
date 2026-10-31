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
mkdir -p "$T/root/Documents" "$T/data"
echo hi > "$T/root/Documents/Quarterly Report.pdf"
echo hi > "$T/root/Documents/notes-2024.md"
printf 'roots = ["%s"]\ndebounce_ms = 20\n' "$T/root" > "$T/config.toml"

export FERROSEEK_CONFIG="$T/config.toml" FERROSEEK_DATA_DIR="$T/data" FERROSEEK_LOG=debug
"$BIN" 2> "$T/daemon.log" &
PID=$!

SVC=io.github.ferroseek.FerroSeek
call() { gdbus call --session --dest "$SVC" "$@"; }
fail() { echo "FAIL: $*"; echo "--- daemon log"; cat "$T/daemon.log"; exit 1; }

for _ in $(seq 100); do
    call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status \
        2>/dev/null | grep -q "'state': 'idle'" && break
    sleep 0.05
done

echo "== Status"
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status

echo "== Actions"
out=$(call --object-path /runner --method org.kde.krunner1.Actions)
echo "$out"; [[ "$out" == *open-parent* ]] || fail "Actions"

echo "== Match 'report'"
out=$(call --object-path /runner --method org.kde.krunner1.Match "report")
echo "$out"
[[ "$out" == *"Quarterly Report.pdf"* ]] || fail "Match did not find the file"
[[ "$out" == *"file://$T/root/Documents/Quarterly%20Report.pdf"* ]] || fail "match id is not a file URL"
[[ "$out" == *"'urls': <['file://"* ]] || fail "urls property"
[[ "$out" == *"application-pdf"* ]] || fail "icon"

echo "== Live update"
echo x > "$T/root/brand-new-file.txt"
for _ in $(seq 100); do
    out=$(call --object-path /runner --method org.kde.krunner1.Match "brand-new")
    [[ "$out" == *brand-new-file.txt* ]] && break
    sleep 0.05
done
[[ "$out" == *brand-new-file.txt* ]] || fail "live update not visible"
echo "$out"

echo "== Short and empty queries"
empty() { [[ "$1" == *"[]"* && "$1" != *"file://"* ]]; }
empty "$(call --object-path /runner --method org.kde.krunner1.Match "q")" || fail "1-char query"
empty "$(call --object-path /runner --method org.kde.krunner1.Match "zzzzqqq")" || fail "no-hit query"

echo "== Concurrent Match calls"
for i in $(seq 20); do call --object-path /runner --method org.kde.krunner1.Match "notes" > "$T/c$i" & done
wait $(jobs -p | grep -v "^$PID$") 2>/dev/null || true
for i in $(seq 20); do grep -q "notes-2024.md\|\[\]" "$T/c$i" || fail "concurrent call $i"; done

echo "== Run on a path that is not indexed is refused"
call --object-path /runner --method org.kde.krunner1.Run "file:///etc/passwd" "" >/dev/null
sleep 0.2
grep -q "not in the index" "$T/daemon.log" || fail "unindexed Run not refused"

echo "== Teardown, Pause/Resume"
call --object-path /runner --method org.kde.krunner1.Teardown >/dev/null
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Pause >/dev/null
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status | grep -q "'state': 'paused'" || fail "pause"
call --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Resume >/dev/null

echo "== Introspection"
gdbus introspect --session --dest "$SVC" --object-path /runner | grep -E "method|out|in " | head -20

echo "== SIGTERM shutdown"
kill -TERM "$PID"; wait "$PID"; rc=$?
[[ $rc == 0 ]] || fail "exit code $rc"
grep -q "shutting down" "$T/daemon.log" || fail "no graceful shutdown"
echo "PASS"
