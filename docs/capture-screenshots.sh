#!/usr/bin/env bash
# Capture screenshots for the README using the GNOME Shell D-Bus API.
#
# PRIVACY: this captures the WHOLE screen. Close other windows / notifications
# and leave only Cascade visible before running. Review the images before
# committing them.
#
# Usage:  docs/capture-screenshots.sh [name]      # default name: dashboard
set -euo pipefail
cd "$(dirname "$0")/.."

name="${1:-dashboard}"
out="docs/screenshots"
mkdir -p "$out"

# Prefer a release build if present, else debug.
bin=target/release/cascade
[ -x "$bin" ] || bin=target/debug/cascade
[ -x "$bin" ] || { echo "build first: cargo build -p cascade-gui"; exit 1; }

"$bin" &
app_pid=$!
trap 'kill "$app_pid" 2>/dev/null || true' EXIT
sleep 2.5  # let the window render

file="$PWD/$out/$name.png"
if command -v gnome-screenshot >/dev/null; then
    gnome-screenshot -w -f "$file"
else
    gdbus call --session --dest org.gnome.Shell \
        --object-path /org/gnome/Shell/Screenshot \
        --method org.gnome.Shell.Screenshot.Screenshot false false "$file" >/dev/null
fi
echo "saved $file"
