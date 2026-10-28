#!/usr/bin/env bash
# Renders the reference screenshots (dummy tree only) into docs/screenshots.
set -euo pipefail
# scratch space for the dummy tree, screenshots and a private config (never real files)
DEV="${VECTOR_DEV:-${TMPDIR:-/tmp}/vector-dev}"
cd "$(dirname "$0")/.."
T=$DEV/tree
[ -d "$T" ] || BIG=1 scripts/make-test-tree.sh >/dev/null
O=docs/screenshots
mkdir -p "$O"
shot() { scripts/shot.sh "$PWD/$O/$1.png" "$2" "${3:-$T}" "${@:4}" >/dev/null && echo "$O/$1.png"; }
shot 01-list         ""
shot 02-grid         "w"                        "$T/Media/photos"
shot 03-split        "sjj<Tab>"
shot 04-preview-code "/main<CR>i"               "$T/Projects"
shot 05-preview-image "ji"                      "$T/Media/photos" --after 1500
shot 06-preview-video "i"                       "$T/Media/clips" --after 2500
shot 07-command-line ":sort <Tab>"
shot 08-confirm      "j<S-Del>"                 "$T/Documents/notes"
shot 09-keys-help    "?"
shot 10-filter-flat  ":flat<CR>/type:image|video<CR>" "$T/Media" --after 1500
shot 11-git-badges   ""                         "$T/Projects/repo"
shot 12-big-folder   "G"                        "$T/big"
