#!/usr/bin/env bash
# Install Cascade for the current user only — no root, no sudo.
#
# Builds the release binary and installs the binary + desktop file + icon +
# AppStream metainfo under ~/.local. This is what makes desktop notifications
# show Cascade's name and icon correctly on BOTH Wayland and X11 (they are
# resolved from the installed .desktop file matched by the app id).
#
# Uninstall: packaging/install-local.sh --uninstall
set -euo pipefail

APP_ID="io.github.alexmihai.Cascade"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/scalable/apps"
META_DIR="$DATA_DIR/metainfo"

uninstall() {
    echo "Removing Cascade (user install)…"
    rm -f "$BIN_DIR/cascade"
    rm -f "$APP_DIR/$APP_ID.desktop"
    rm -f "$ICON_DIR/$APP_ID.svg"
    rm -f "$META_DIR/$APP_ID.metainfo.xml"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f "$DATA_DIR/icons/hicolor" || true
    echo "Done."
}

if [[ "${1:-}" == "--uninstall" ]]; then
    uninstall
    exit 0
fi

echo "Building release binary…"
cargo build -p cascade-gui --release

echo "Installing under ~/.local …"
mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR" "$META_DIR"
install -m755 "$REPO_ROOT/target/release/cascade" "$BIN_DIR/cascade"
install -m644 "$REPO_ROOT/packaging/$APP_ID.desktop" "$APP_DIR/$APP_ID.desktop"
install -m644 "$REPO_ROOT/packaging/icons/hicolor/scalable/apps/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
install -m644 "$REPO_ROOT/packaging/$APP_ID.metainfo.xml" "$META_DIR/$APP_ID.metainfo.xml"

command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f "$DATA_DIR/icons/hicolor" || true

echo
echo "Installed. Make sure '$BIN_DIR' is on your PATH, then launch 'cascade'"
echo "from your application menu or terminal."
