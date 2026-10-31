#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
exec python3 -B "$(dirname -- "$0")/installer.py" --uninstall "$@"
