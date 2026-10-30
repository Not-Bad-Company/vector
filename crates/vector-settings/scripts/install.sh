#!/bin/sh
# Build and install vectorOS Settings for this user.
set -eu
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 3 --release -p vector-settings
target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
install -Dm755 "$target/release/vector-settings" "$HOME/.local/bin/vector-settings.new"
mv -f "$HOME/.local/bin/vector-settings.new" "$HOME/.local/bin/vector-settings"
install -Dm644 data/org.vectoros.Settings.desktop "$HOME/.local/share/applications/org.vectoros.Settings.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "installed vector-settings"
