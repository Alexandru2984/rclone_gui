#!/usr/bin/env bash
# Regenerate po/cascade.pot from the source. Needs `xgettext` (gettext package).
# Strings are marked with the i18n::tr("…") wrapper.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v xgettext >/dev/null || {
    echo "xgettext not found — install gettext (e.g. 'sudo apt install gettext')" >&2
    exit 1
}

mapfile -t sources < <(find crates/cascade-gui/src -name '*.rs')
xgettext \
    --from-code=UTF-8 \
    --language=C \
    --keyword=tr \
    --add-comments=TRANSLATORS \
    --package-name=cascade \
    -o po/cascade.pot \
    "${sources[@]}"
echo "wrote po/cascade.pot — merge into translations with: msgmerge <lang>.po cascade.pot"
