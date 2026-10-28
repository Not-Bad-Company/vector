#!/usr/bin/env bash
# Renders vector on workspace 9 (never on the user's screen) to a PNG and exits.
#   scripts/shot.sh OUT.png [KEYS] [FOLDER] [extra args…]
set -euo pipefail
out="$1"; mkdir -p /tmp/claude-1000/vector-shots; keys="${2:-}"; dir="${3:-/tmp/claude-1000/vector-test}"; shift $(( $# < 3 ? $# : 3 ))
bin="$(dirname "$0")/../target/release/vector"
bin="$(realpath "$bin")"
rm -f "$out" /tmp/claude-1000/vector-cfg/vector/settings.ini
q() { printf '%q ' "$@"; }
hyprctl dispatch exec "[workspace 9 silent;render_unfocused on] env XDG_CONFIG_HOME=/tmp/claude-1000/vector-cfg VECTOR_TIMING=1 VECTOR_NO_CLIPBOARD=1 $(q "$bin" "$dir" --snapshot "$out" --keys "$keys" --size 1400x860 "$@") 2>/tmp/claude-1000/vector-shots/last.log" >/dev/null
for _ in $(seq 1 150); do [ -s "$out" ] && break; sleep 0.1; done
sleep 0.3
# it quits by itself after the snapshot; close it if it did not
for pid in $(pgrep -f -- "--snapshot $out" || true); do hyprctl dispatch closewindow "pid:$pid" >/dev/null; done
[ -s "$out" ] && echo "$out" || { echo "no snapshot" >&2; exit 1; }
