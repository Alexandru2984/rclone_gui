#!/usr/bin/env bash
# Regenerate po/cascade.pot from the source. Needs `xgettext` (gettext package).
#
# GUI strings are marked with i18n::tr("…"); user-facing data literals that live
# in the GTK-free core crate are marked with the no-op n("…") (see cascade_core::n).
set -euo pipefail
cd "$(dirname "$0")/.."

command -v xgettext >/dev/null || {
    echo "xgettext not found — install gettext (e.g. 'sudo apt install gettext')" >&2
    exit 1
}

mapfile -t sources < <(find crates -name '*.rs' -not -path '*/tests/*')
xgettext \
    --from-code=UTF-8 \
    --language=C \
    --keyword=tr \
    --keyword=n \
    --add-comments=TRANSLATORS \
    --package-name=cascade \
    -o po/cascade.pot \
    "${sources[@]}"
echo "wrote po/cascade.pot — merge into translations with: msgmerge <lang>.po cascade.pot"
