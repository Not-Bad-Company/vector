#!/bin/bash
# Installs the vectorOS system commands (`vector` and its vector-* names) and
# the brand files (logo, screensaver wordmark, version).
set -e
cd "$(dirname "$0")"
BIN="$HOME/.local/bin"
SHARE="$HOME/.local/share/vectoros"

install -Dm755 bin/vector "$BIN/vector"
for n in update install remove setup about version wifi bluetooth sound lock logout restart shutdown; do
  ln -sfn vector "$BIN/vector-$n"
done
# Omarchy's keys and menus open these: route them to vectorOS Settings
for pair in wifi:wifi bluetooth:bluetooth audio:sound; do
  printf '#!/bin/sh\n# vectorOS: Omarchy %s controls open Settings instead (os/install.sh)\nexec vector %s\n' "${pair%%:*}" "${pair##*:}" >"$BIN/omarchy-launch-${pair%%:*}"
  chmod 755 "$BIN/omarchy-launch-${pair%%:*}"
done
install -Dm644 share/logo.txt "$SHARE/logo.txt"
install -Dm644 share/wordmark.txt "$SHARE/wordmark.txt"
v="$(sed -n 's/^version = "\(.*\)"/\1/p' ../Cargo.toml | head -1)"
echo "$v ($(git -C .. rev-parse --short HEAD 2>/dev/null || echo dev))" >"$SHARE/version"
echo "installed: $BIN/vector ($(cat "$SHARE/version"))"
