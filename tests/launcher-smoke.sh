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

export FERROSEEK_SMOKE_PROFILE=${FERROSEEK_SMOKE_PROFILE:-us-1x}
SMOKE_WIDTH=1280; SMOKE_HEIGHT=800; SMOKE_OUTPUTS=1
case "$FERROSEEK_SMOKE_PROFILE" in
    us-multi) SMOKE_LAYOUT=us; SMOKE_SCALE=1; SMOKE_OUTPUTS=2 ;;
    us-1x) SMOKE_LAYOUT=us; SMOKE_SCALE=1 ;;
    fr-1x) SMOKE_LAYOUT=fr; SMOKE_SCALE=1 ;;
    us-1.5x) SMOKE_LAYOUT=us; SMOKE_SCALE=1.5 ;;
    us-short-1x) SMOKE_LAYOUT=us; SMOKE_SCALE=1; SMOKE_WIDTH=640; SMOKE_HEIGHT=360 ;;
    us-layout-1x) SMOKE_LAYOUT=us; SMOKE_SCALE=1; SMOKE_WIDTH=640; SMOKE_HEIGHT=480 ;;
    us-layout-1.5x) SMOKE_LAYOUT=us; SMOKE_SCALE=1.5; SMOKE_WIDTH=960; SMOKE_HEIGHT=640 ;;
    *) echo "FAIL: unknown launcher profile: $FERROSEEK_SMOKE_PROFILE" >&2; exit 1 ;;
esac

export FERROSEEK_SMOKE_MODE=${FERROSEEK_SMOKE_MODE:-auto}
case "$FERROSEEK_SMOKE_MODE" in
    auto|opaque|software|all) ;;
    *) echo "FAIL: unknown launcher mode: $FERROSEEK_SMOKE_MODE" >&2; exit 1 ;;
esac

BIN_DIR=$(realpath "${1:-target/debug}")
OUT=$(realpath -m "${2:-target/launcher-smoke}")
shift $(( $# > 2 ? 2 : $# ))
QUERIES=("$@")
[[ ${#QUERIES[@]} -gt 0 ]] || QUERIES=(report "" "(2+3)*4")

for tool in kwin_wayland spectacle dbus-run-session gdbus; do
    if ! command -v "$tool" >/dev/null; then
        if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
            echo "INCONCLUSIVE: $tool not installed" >&2
            exit 77
        fi
        echo "SKIP: $tool not installed"
        exit 0
    fi
done
if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
    for tool in python3 wayland-scanner cc pkg-config; do
        command -v "$tool" >/dev/null || { echo "INCONCLUSIVE: $tool not installed" >&2; exit 77; }
    done
    if [[ "$FERROSEEK_SMOKE_PROFILE" == us-multi ]]; then
        for tool in wl-copy wl-paste; do
            command -v "$tool" >/dev/null || { echo "INCONCLUSIVE: $tool not installed" >&2; exit 77; }
        done
    fi
    python3 -c 'from PyQt5.QtWidgets import QApplication; import gi; gi.require_version("Gtk", "4.0")' \
        || { echo "INCONCLUSIVE: graphical Python bindings unavailable" >&2; exit 77; }
    python3 - "$OUT" <<'PY'
from pathlib import Path
import sys
output = Path(sys.argv[1])
if output.exists() and (not output.is_dir() or any(output.iterdir())):
    raise SystemExit("FAIL: interaction output directory must be new or empty")
PY
fi

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
mkdir -p "$OUT"
cleanup() {
    kill ${LPID:-} ${DPID:-} ${KPID:-} 2>/dev/null || true
    wait 2>/dev/null || true
    if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
        cp "$T"/*.log "$OUT/" 2>/dev/null || true
    fi
    rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $*"; for l in "$T"/*.log; do echo "--- $l"; tail -20 "$l"; done; exit 1; }

export HOME="$T/home" XDG_CONFIG_HOME="$T/home/.config" XDG_DATA_HOME="$T/home/.local/share" \
    XDG_CACHE_HOME="$T/home/.cache" XDG_STATE_HOME="$T/home/.local/state" \
    XDG_RUNTIME_DIR="$T/runtime" XDG_CONFIG_DIRS="$T/empty-config" XDG_DATA_DIRS="$T/empty-data"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME/applications" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" \
    "$XDG_RUNTIME_DIR" "$XDG_CONFIG_DIRS" "$XDG_DATA_DIRS"
chmod 700 "$XDG_RUNTIME_DIR"
unset XDG_ACTIVATION_TOKEN DESKTOP_STARTUP_ID
# Share only read-only graphics/MIME assets, never the system application catalog.
mkdir -p "$XDG_DATA_DIRS/icons"
for theme in breeze breeze-dark hicolor; do
    [[ -d "/usr/share/icons/$theme" ]] || fail "required icon theme missing: $theme"
    ln -s "/usr/share/icons/$theme" "$XDG_DATA_DIRS/icons/$theme"
done
[[ -d /usr/share/mime ]] || fail "shared MIME data not installed"
ln -s /usr/share/mime "$XDG_DATA_DIRS/mime"
printf '[Plugins]\nblurEnabled=true\n' > "$XDG_CONFIG_HOME/kwinrc"
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
if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
    python3 - "$R" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for directory in range(100):
    folder = root / f"Bench-{directory:03}"
    folder.mkdir()
    for item in range(100):
        (folder / f"report-{item:03}.txt").write_text("Synthetic report\n")
PY
fi
printf 'roots = ["%s"]\ndebounce_ms = 20\n' "$R" > "$T/ferroseek.toml"
# Launch history for the empty-query view: a synthetic app and file.
mkdir -p "$XDG_STATE_HOME/ferroseek"
printf 'app\ttest.report-viewer.desktop\n' > "$XDG_STATE_HOME/ferroseek/launcher-favorites"
printf 'app\t3\t%s\ttest.report-viewer.desktop\nfile\t2\t%s\tfile://%s\n' "$(date +%s)" "$(date +%s)" \
    "$(printf %s "$R/Documents/notes-2024.md")" > "$XDG_STATE_HOME/ferroseek/launcher-usage"
export FERROSEEK_CONFIG="$T/ferroseek.toml"
# The daemon's synthetic document handler must use this same isolated display.
SOCK=wayland-ferroseek-smoke-$$
export WAYLAND_DISPLAY="$SOCK"
if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
    mkdir -p "$T/bin"
    target=$(realpath tests/launcher-activation-probe.py)
    python=$(command -v python3)
    printf '#!/bin/sh\nexec "%s" "%s" "$@"\n' "$python" "$target" > "$T/bin/xdg-open"
    chmod +x "$T/bin/xdg-open"
    export PATH="$T/bin:$PATH" FERROSEEK_ACTIVATION_LOG="$OUT/activation-target.jsonl"
    export GDK_BACKEND=wayland GSK_RENDERER=cairo GTK_A11Y=none
fi
FERROSEEK_DATA_DIR="$T/data" "$BIN_DIR/ferroseekd" 2> "$T/daemon.log" &
DPID=$!
has_owner() {
    gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$1" 2>/dev/null | grep -q true
}
for _ in $(seq 100); do has_owner io.github.ferroseek.FerroSeek && break; sleep 0.05; done
has_owner io.github.ferroseek.FerroSeek || fail "test daemon did not start"
if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
    for _ in $(seq 200); do
        status=$(gdbus call --session --dest io.github.ferroseek.FerroSeek \
            --object-path /io/github/ferroseek/FerroSeek --method io.github.ferroseek.FerroSeek1.Status)
        [[ "$status" == *"'state': 'idle'"* && "$status" == *"'files': '10007'"* ]] && break
        sleep 0.05
    done
    [[ "$status" == *"'state': 'idle'"* && "$status" == *"'files': '10007'"* ]] || fail "synthetic index not ready"
else
    sleep 0.5 # initial index of the tiny tree
fi

# KDE's compositor/capture resources and authorization metadata are system data;
# the launcher and daemon retain their separate synthetic application catalog.
XDG_CONFIG_DIRS=/etc/xdg XDG_DATA_DIRS=/usr/local/share:/usr/share \
KWIN_WAYLAND_NO_PERMISSION_CHECKS=${FERROSEEK_SMOKE_INTERACTION:-0} XKB_DEFAULT_LAYOUT="$SMOKE_LAYOUT" \
    kwin_wayland --virtual --output-count "$SMOKE_OUTPUTS" --socket "$SOCK" --width "$SMOKE_WIDTH" --height "$SMOKE_HEIGHT" --scale "$SMOKE_SCALE" > "$T/kwin.log" 2>&1 &
KPID=$!
for _ in $(seq 100); do [[ -S "$XDG_RUNTIME_DIR/$SOCK" ]] && break; sleep 0.05; done
[[ -S "$XDG_RUNTIME_DIR/$SOCK" ]] || fail "nested KWin did not start"
for _ in $(seq 100); do has_owner org.kde.KWin && break; sleep 0.05; done
has_owner org.kde.KWin || fail "nested KWin did not register its control interface"
export WAYLAND_DISPLAY=$SOCK QT_QPA_PLATFORM=wayland

if [[ "${FERROSEEK_SMOKE_INTERACTION:-}" == 1 ]]; then
    XML=/usr/share/plasma-wayland-protocols/fake-input.xml
    [[ -f "$XML" ]] || fail "plasma-wayland-protocols/fake-input.xml not installed"
    wayland-scanner client-header "$XML" "$T/fake-input-client-protocol.h"
    wayland-scanner private-code "$XML" "$T/fake-input-protocol.c"
    cc -std=c11 -Wall -Wextra -Werror -I"$T" tests/launcher-input.c "$T/fake-input-protocol.c" \
        $(pkg-config --cflags --libs wayland-client) -o "$T/launcher-input"
    if [[ "$FERROSEEK_SMOKE_PROFILE" == us-multi ]]; then
        for protocol in output-device output-management; do
            xml="/usr/share/plasma-wayland-protocols/kde-$protocol-v2.xml"
            [[ -f "$xml" ]] || fail "missing KDE output protocol: $xml"
            wayland-scanner client-header "$xml" "$T/$protocol-client-protocol.h"
            wayland-scanner private-code "$xml" "$T/$protocol-protocol.c"
        done
        cc -std=c11 -Wall -Wextra -Werror -I"$T" tests/launcher-output.c "$T/output-device-protocol.c" "$T/output-management-protocol.c" \
            $(pkg-config --cflags --libs wayland-client) -o "$T/launcher-output"
        export FERROSEEK_SMOKE_OUTPUT_HELPER="$T/launcher-output"
    fi
    python3 tests/launcher-interaction.py "$BIN_DIR/ferroseek-launcher" "$OUT" "$T/launcher-input"
    exit
fi
i=0
for q in "${QUERIES[@]}"; do
    i=$((i + 1))
    "$BIN_DIR/ferroseek-launcher" --query "$q" > "$T/launcher.log" 2>&1 &
    LPID=$!
    for _ in $(seq 100); do has_owner io.github.ferroseek.Launcher && break; sleep 0.05; done
    sleep 1.5
    kill -0 "$LPID" 2>/dev/null || fail "launcher exited for query '$q'"
    XDG_CONFIG_DIRS=/etc/xdg XDG_DATA_DIRS=/usr/local/share:/usr/share \
        timeout 15 spectacle -b -n -f -o "$OUT/query-$i.png" > "$OUT/query-$i.png.log" 2>&1 || fail "screenshot failed"
    [[ -s "$OUT/query-$i.png" ]] || fail "capture tool returned without an image; inspect query-$i.png.log"
    echo "== '$q': $OUT/query-$i.png"
    timeout 5 "$BIN_DIR/ferroseek-launcher" || fail "toggle invocation failed"
    for _ in $(seq 40); do kill -0 "$LPID" 2>/dev/null || break; sleep 0.05; done
    kill -0 "$LPID" 2>/dev/null && fail "second invocation did not close the launcher"
    wait "$LPID" || fail "launcher exit status $?"
    echo "   toggle closed it"
done
echo "OK"
