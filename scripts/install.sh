#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# User-scoped installation of FerroSeek. No root, no system files touched,
# no KDE configuration changed. Re-running upgrades in place.
#
#   scripts/install.sh [--no-build] [--enable] [--config FILE]
#
#   --no-build     use existing target/release binaries
#   --enable       enable + start the systemd user service now (otherwise
#                  KRunner starts the daemon on demand via D-Bus activation)
#   --config FILE  install FILE as ~/.config/ferroseek/config.toml if no
#                  config exists yet (never overwrites)
set -euo pipefail
cd "$(dirname "$0")/.."

BUILD=1 ENABLE=0 CONFIG=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) BUILD=0 ;;
        --enable) ENABLE=1 ;;
        --config) CONFIG="$2"; shift ;;
        -h|--help) sed -n '3,13p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

if [[ $EUID -eq 0 ]]; then
    echo "refusing to run as root: FerroSeek installs per user" >&2
    exit 1
fi

BINDIR="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
CONF="${XDG_CONFIG_HOME:-$HOME/.config}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

[[ $BUILD == 1 ]] && cargo build --release --locked -p ferroseek-daemon -p ferroseek-cli

render() { sed -e "s|@BINDIR@|$BINDIR|g" -e "s|@VERSION@|$VERSION|g" "$1"; }
put() {  # put MODE SRC DEST (rendered)
    install -d "$(dirname "$3")"
    render "$2" > "$3.tmp.$$"
    chmod "$1" "$3.tmp.$$"
    mv -f "$3.tmp.$$" "$3"
    echo "  $3"
}

echo "Installing FerroSeek $VERSION for $USER:"
install -Dm755 target/release/ferroseekd "$BINDIR/ferroseekd" && echo "  $BINDIR/ferroseekd"
install -Dm755 target/release/ferroseek "$BINDIR/ferroseek" && echo "  $BINDIR/ferroseek"
put 644 data/ferroseek-krunner.desktop "$DATA/krunner/dbusplugins/ferroseek.desktop"
put 644 data/io.github.ferroseek.FerroSeek.service "$DATA/dbus-1/services/io.github.ferroseek.FerroSeek.service"
put 644 data/ferroseek.service "$CONF/systemd/user/ferroseek.service"

if [[ -n "$CONFIG" ]]; then
    if [[ -e "$CONF/ferroseek/config.toml" ]]; then
        echo "  config exists, left untouched: $CONF/ferroseek/config.toml"
    else
        install -Dm600 "$CONFIG" "$CONF/ferroseek/config.toml" && echo "  $CONF/ferroseek/config.toml"
    fi
fi

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

if command -v systemctl >/dev/null; then
    systemctl --user daemon-reload
    reload_bus
    if [[ $ENABLE == 1 ]]; then
        systemctl --user enable --now ferroseek.service
    elif systemctl --user is-active --quiet ferroseek.service; then
        systemctl --user restart ferroseek.service   # pick up the new binary
    fi
fi

cat <<MSG

Done. Next steps:
  - KRunner picks up new runners after a restart:   kquitapp6 krunner
    (it is started again automatically the next time you open it)
  - Index status:                                   ferroseek status
  - Configuration (defaults index your home):      ferroseek config
  - To avoid duplicate file results you may disable Baloo's runner in
    System Settings → Search → Plasma Search → "File Search" (FerroSeek
    never changes this for you). See README.md.
MSG
