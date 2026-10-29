#!/bin/sh
# Build and install Vector Capture for this user: binaries to ~/.local/bin, the
# systemd user unit, then (re)start it.
set -eu
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 3 --release -p vector-capture
target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
for bin in vector-capture vector-capture-daemon; do
    install -Dm755 "$target/release/$bin" "$HOME/.local/bin/$bin.new"
    mv -f "$HOME/.local/bin/$bin.new" "$HOME/.local/bin/$bin"
done
install -Dm644 data/vector-capture.service "$HOME/.config/systemd/user/vector-capture.service"
systemctl --user daemon-reload
systemctl --user enable vector-capture.service >/dev/null 2>&1 || true
systemctl --user restart vector-capture.service
echo "installed $(vector-capture --version)"
