#!/usr/bin/env bash
# Development: renders vector on Hyprland workspace 9 (off screen) to a PNG and exits.
#   scripts/shot.sh OUT.png [KEYS] [FOLDER] [extra args…]
set -euo pipefail
# scratch space for the dummy tree, screenshots and a private config (never real files)
DEV="${VECTOR_DEV:-${TMPDIR:-/tmp}/vector-dev}"
out="$1"; mkdir -p $DEV/shots; keys="${2:-}"; dir="${3:-$DEV/tree}"; shift $(( $# < 3 ? $# : 3 ))
bin="$(dirname "$0")/../target/${PROFILE:-release}/vector-find"
bin="$(realpath "$bin")"
rm -rf "$out" $DEV/config/vector/settings.ini $DEV/state
q() { printf '%q ' "$@"; }
hyprctl dispatch exec "[workspace 9 silent;render_unfocused on] env ${VECTOR_ENV:-} XDG_CONFIG_HOME=$DEV/config XDG_CACHE_HOME=$DEV/cache XDG_STATE_HOME=$DEV/state VECTOR_TIMING=1 VECTOR_NO_CLIPBOARD=1 VECTOR_NO_LAUNCH=1 VECTOR_DEMO=1 ${VECTOR_EDIT_CMD:+VECTOR_EDIT_CMD=$VECTOR_EDIT_CMD }$(q "$bin" "$dir" --snapshot "$out" --keys "$keys" --size 1400x860 "$@") 2>$DEV/shots/last.log" >/dev/null
for _ in $(seq 1 150); do [ -s "$out" ] && break; sleep 0.1; done
sleep 0.3
# it quits by itself after the snapshot; close it if it did not
for pid in $(pgrep -f -- "--snapshot $out" || true); do hyprctl dispatch closewindow "pid:$pid" >/dev/null; done
[ -s "$out" ] && echo "$out" || { echo "no snapshot" >&2; exit 1; }
