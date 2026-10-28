#!/usr/bin/env bash
# Renders strata on workspace 9 (never on the user's screen) to a PNG and exits.
#   scripts/shot.sh OUT.png [KEYS] [FOLDER] [extra args…]
set -euo pipefail
out="$1"; keys="${2:-}"; dir="${3:-/tmp/claude-1000/strata-test}"; shift $(( $# < 3 ? $# : 3 ))
bin="$(dirname "$0")/../target/release/strata"
bin="$(realpath "$bin")"
rm -f "$out"
q() { printf '%q ' "$@"; }
hyprctl dispatch exec "[workspace 9 silent] env XDG_CONFIG_HOME=/tmp/claude-1000/strata-cfg $(q "$bin" "$dir" --snapshot "$out" --keys "$keys" --size 1400x860 "$@") 2>/tmp/claude-1000/strata-shots/last.log" >/dev/null
for _ in $(seq 1 150); do [ -s "$out" ] && break; sleep 0.1; done
sleep 0.3
# it quits by itself after the snapshot; close it if it did not
for pid in $(pgrep -f -- "--snapshot $out" || true); do hyprctl dispatch closewindow "pid:$pid" >/dev/null; done
[ -s "$out" ] && echo "$out" || { echo "no snapshot" >&2; exit 1; }
