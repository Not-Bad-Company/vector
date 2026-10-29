#!/bin/sh
# Build and install Vector Bar for this user: binary to ~/.local/bin, the
# systemd user unit, then restart it if it runs.
set -eu
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 target/release/vector-bar "$HOME/.local/bin/vector-bar.new"
mv -f "$HOME/.local/bin/vector-bar.new" "$HOME/.local/bin/vector-bar"
install -Dm644 data/vector-bar.service "$HOME/.config/systemd/user/vector-bar.service"
systemctl --user daemon-reload
if systemctl --user is-active --quiet vector-bar.service; then
    systemctl --user restart vector-bar.service
fi
echo "installed $(vector-bar --version 2>/dev/null || echo vector-bar)"
