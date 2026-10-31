#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Removes a user-scoped FerroSeek installation.
#
#   scripts/uninstall.sh [--purge]
#
#   --purge   also delete FerroSeek's own index (~/.local/share/ferroseek)
#             and config (~/.config/ferroseek). Nothing else is touched.
set -euo pipefail
PURGE=0
[[ "${1:-}" == "--purge" ]] && PURGE=1

BINDIR="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"

if command -v systemctl >/dev/null; then
    systemctl --user disable --now ferroseek.service 2>/dev/null || true
fi
for f in "$BINDIR/ferroseekd" "$BINDIR/ferroseek" \
         "$DATA/krunner/dbusplugins/ferroseek.desktop" \
         "$DATA/dbus-1/services/io.github.ferroseek.FerroSeek.service" \
         "$CONF/systemd/user/ferroseek.service"; do
    [[ -e "$f" ]] && rm -f "$f" && echo "removed $f"
done
# dbus-broker (Arch's default session bus) does not watch service
# directories: ask the user's own session bus to rescan them.
reload_bus() {
    if command -v busctl >/dev/null; then
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
    elif command -v gdbus >/dev/null; then
        gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
            --method org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
    fi
}

command -v systemctl >/dev/null && systemctl --user daemon-reload || true

if [[ $PURGE == 1 ]]; then
    for d in "$DATA/ferroseek" "$CONF/ferroseek"; do
        # Only ever FerroSeek's own directories.
        [[ -d "$d" && "$(basename "$d")" == ferroseek ]] && rm -rf -- "$d" && echo "removed $d"
    done
else
    echo "kept index and config (use --purge to remove $DATA/ferroseek and $CONF/ferroseek)"
fi
echo "Restart KRunner to drop the runner: kquitapp6 krunner"
