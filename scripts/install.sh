#!/usr/bin/env bash
# Builds vector quietly (low priority, 3 cores) and installs it for this user.
set -euo pipefail
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 3 --release -p vector
install -Dm755 target/release/vector ~/.local/bin/vector
install -Dm644 data/vector.svg ~/.local/share/icons/hicolor/scalable/apps/vector.svg
install -Dm644 data/vector.desktop ~/.local/share/applications/vector.desktop
update-desktop-database ~/.local/share/applications 2>/dev/null || true
echo "installed: $(command -v vector)"
