#!/usr/bin/env bash
# Compile every <lang>.po into an installable .mo catalog under po/locale/.
# Needs `msgfmt` from the gettext package:  sudo apt install gettext
set -euo pipefail
cd "$(dirname "$0")"

command -v msgfmt >/dev/null || {
    echo "msgfmt not found — install gettext (e.g. 'sudo apt install gettext')" >&2
    exit 1
}

shopt -s nullglob
for po in *.po; do
    lang="${po%.po}"
    dest="locale/$lang/LC_MESSAGES"
    mkdir -p "$dest"
    msgfmt "$po" -o "$dest/cascade.mo"
    echo "built $dest/cascade.mo"
done

echo
echo "Try it:   CASCADE_LOCALE_DIR=$(pwd)/locale LANG=ro_RO.UTF-8 cargo run -p cascade-gui"
echo "Install:  copy po/locale/* into /usr/share/locale/ (packaging does this)."
