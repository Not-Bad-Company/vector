#!/usr/bin/env bash
# Development: renders vector-view on Hyprland workspace 9 (off screen) to a PNG and exits.
#   scripts/view-shot.sh OUT.png [KEYS] [FILE…] [extra args…]
set -euo pipefail
DEV="${VECTOR_DEV:-${TMPDIR:-/tmp}/vector-dev}"
out="$1"; mkdir -p $DEV/shots; keys="${2:-}"; shift $(( $# < 2 ? $# : 2 ))
[ $# -gt 0 ] || set -- "$DEV/media/aurora.jpg"
bin="$(realpath "$(dirname "$0")/../target/${PROFILE:-release}/vector-view")"
rm -rf "$out" $DEV/state
q() { printf '%q ' "$@"; }
hyprctl dispatch exec "[workspace 9 silent;render_unfocused on] env ${VECTOR_ENV:-} XDG_CONFIG_HOME=$DEV/config XDG_CACHE_HOME=$DEV/cache XDG_STATE_HOME=$DEV/state VECTOR_NO_CLIPBOARD=1 VECTOR_DEMO=1 $(q "$bin" "$@" --snapshot "$out" --keys "$keys" --size ${SIZE:-1400x860}) 2>$DEV/shots/view.log"
for _ in $(seq 1 200); do [ -s "$out" ] && break; sleep 0.1; done
sleep 0.3
for pid in $(pgrep -f -- "--snapshot $out" || true); do hyprctl dispatch closewindow "pid:$pid" >/dev/null; done
[ -s "$out" ] && echo "$out" || { echo "no snapshot" >&2; cat $DEV/shots/view.log >&2; exit 1; }
