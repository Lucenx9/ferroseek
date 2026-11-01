#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Visual smoke test of ferroseek-launcher: runs it on a nested, virtual
# KWin (kwin_wayland --virtual) against a test ferroseekd, on a private
# session bus, with temporary HOME and XDG directories and a synthetic file
# tree. Never touches the real desktop, session bus, index or configuration.
#
#   tests/launcher-smoke.sh [target/debug] [OUT_DIR] [QUERY...]
#
# Saves one screenshot per query (spectacle) and checks that a second
# invocation closes the running launcher. Skips when kwin_wayland or
# spectacle is missing.
set -euo pipefail

BIN_DIR=$(realpath "${1:-target/debug}")
OUT=$(realpath -m "${2:-target/launcher-smoke}")
shift $(( $# > 2 ? 2 : $# ))
QUERIES=("$@")
[[ ${#QUERIES[@]} -gt 0 ]] || QUERIES=(report "" "(2+3)*4")

for tool in kwin_wayland spectacle dbus-run-session gdbus; do
    command -v "$tool" >/dev/null || { echo "SKIP: $tool not installed"; exit 0; }
done

if [[ "${FERROSEEK_SMOKE_ISOLATED:-}" != 1 ]]; then
    CONF=$(mktemp /tmp/ferroseek-launcher-bus-XXXXXX.conf)
    SVC=$(mktemp -d /tmp/ferroseek-launcher-svc-XXXXXX)
    # An empty activation directory: nothing may be D-Bus-activated, neither
    # the user's real ferroseekd nor desktop portals (which would outlive
    # the private bus).
    sed "s|@SERVICEDIR@|$SVC|" > "$CONF" <<'XML'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <servicedir>@SERVICEDIR@</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
XML
    rc=0
    FERROSEEK_SMOKE_ISOLATED=1 dbus-run-session --config-file="$CONF" -- "$0" "$BIN_DIR" "$OUT" "${QUERIES[@]}" || rc=$?
    rm -rf "$CONF" "$SVC"
    exit $rc
fi

T=$(mktemp -d /tmp/ferroseek-launcher-smoke-XXXXXX)
cleanup() {
    kill ${LPID:-} ${DPID:-} ${KPID:-} 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $*"; for l in "$T"/*.log; do echo "--- $l"; tail -20 "$l"; done; exit 1; }

export HOME="$T/home" XDG_CONFIG_HOME="$T/home/.config" XDG_DATA_HOME="$T/home/.local/share" \
    XDG_CACHE_HOME="$T/home/.cache" XDG_STATE_HOME="$T/home/.local/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME/applications" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"
printf '[Icons]\nTheme=breeze-dark\n' > "$XDG_CONFIG_HOME/kdeglobals"
printf '[Desktop Entry]\nType=Application\nName=Report Viewer\nComment=Synthetic test entry\nIcon=document-preview\nExec=true\n' \
    > "$XDG_DATA_HOME/applications/test.report-viewer.desktop"

R="$T/home/Corpus"
mkdir -p "$R/Documents/Reports 2026" "$R/Projects/ferro-notes" "$R/Pictures"
for f in "Documents/Reports 2026/Quarterly Report.pdf" "Documents/Reports 2026/budget-report.ods" \
    "Documents/report-draft.odt" "Documents/notes-2024.md" "Projects/ferro-notes/README.md" \
    "Projects/ferro-notes/report.rs" "Pictures/report-cover.png"; do
    echo x > "$R/$f"
done
printf 'roots = ["%s"]\ndebounce_ms = 20\n' "$R" > "$T/ferroseek.toml"
# Launch history for the empty-query view: a synthetic app and file.
mkdir -p "$XDG_STATE_HOME/ferroseek"
printf 'app\t3\t%s\ttest.report-viewer.desktop\nfile\t2\t%s\tfile://%s\n' "$(date +%s)" "$(date +%s)" \
    "$(printf %s "$R/Documents/notes-2024.md")" > "$XDG_STATE_HOME/ferroseek/launcher-usage"
FERROSEEK_CONFIG="$T/ferroseek.toml" FERROSEEK_DATA_DIR="$T/data" "$BIN_DIR/ferroseekd" 2> "$T/daemon.log" &
DPID=$!
has_owner() {
    gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$1" 2>/dev/null | grep -q true
}
for _ in $(seq 100); do has_owner io.github.ferroseek.FerroSeek && break; sleep 0.05; done
has_owner io.github.ferroseek.FerroSeek || fail "test daemon did not start"
sleep 0.5 # initial index of the tiny tree

SOCK=wayland-ferroseek-smoke-$$
kwin_wayland --virtual --socket "$SOCK" --width 1280 --height 800 > "$T/kwin.log" 2>&1 &
KPID=$!
for _ in $(seq 100); do [[ -S "$XDG_RUNTIME_DIR/$SOCK" ]] && break; sleep 0.05; done
[[ -S "$XDG_RUNTIME_DIR/$SOCK" ]] || fail "nested KWin did not start"
sleep 1.5
export WAYLAND_DISPLAY=$SOCK QT_QPA_PLATFORM=wayland

mkdir -p "$OUT"
i=0
for q in "${QUERIES[@]}"; do
    i=$((i + 1))
    "$BIN_DIR/ferroseek-launcher" --query "$q" > "$T/launcher.log" 2>&1 &
    LPID=$!
    for _ in $(seq 100); do has_owner io.github.ferroseek.Launcher && break; sleep 0.05; done
    sleep 1.5
    kill -0 "$LPID" 2>/dev/null || fail "launcher exited for query '$q'"
    timeout 15 spectacle -b -n -f -o "$OUT/query-$i.png" > /dev/null 2>&1 || fail "screenshot failed"
    echo "== '$q': $OUT/query-$i.png"
    timeout 5 "$BIN_DIR/ferroseek-launcher" || fail "toggle invocation failed"
    for _ in $(seq 40); do kill -0 "$LPID" 2>/dev/null || break; sleep 0.05; done
    kill -0 "$LPID" 2>/dev/null && fail "second invocation did not close the launcher"
    wait "$LPID" || fail "launcher exit status $?"
    echo "   toggle closed it"
done
echo "OK"
