#!/bin/sh
# Build and install Vector Space for this user: binary to ~/.local/bin, the
# systemd user unit, then (re)start it.
set -eu
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 3 --release -p vector-space
target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
for bin in vector-space vector-space-daemon; do
    install -Dm755 "$target/release/$bin" "$HOME/.local/bin/$bin.new"
    mv -f "$HOME/.local/bin/$bin.new" "$HOME/.local/bin/$bin"
done
install -Dm644 data/vector-space.service "$HOME/.config/systemd/user/vector-space.service"
systemctl --user daemon-reload
systemctl --user enable vector-space.service >/dev/null 2>&1 || true
systemctl --user restart vector-space.service
echo "installed $(vector-space --version)"
