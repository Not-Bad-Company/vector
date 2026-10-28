#!/usr/bin/env bash
# Builds vector quietly (low priority, 3 cores) and installs it for this user (~/.local).
# Packagers: see packaging/arch/PKGBUILD for the system-wide layout.
set -euo pipefail
cd "$(dirname "$0")/.."
ID=io.github.not_bad_company.Vector
BIN="$HOME/.local/bin/vector-fm"
nice -n 19 cargo build -j 3 --release -p vector-fm
install -Dm755 target/release/vector-fm "$BIN"
install -Dm644 data/$ID.svg ~/.local/share/icons/hicolor/scalable/apps/$ID.svg
install -Dm644 data/$ID.desktop ~/.local/share/applications/$ID.desktop
mkdir -p ~/.local/share/dbus-1/services
# open/save dialogs of other apps (xdg-desktop-portal FileChooser backend)
sed "s|@BIN@|$BIN|" data/org.freedesktop.impl.portal.desktop.vector.service.in \
  > ~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.vector.service
# "show in folder" from other apps; optional, replaces the file manager that answers it
if [ "${VECTOR_FILEMANAGER1:-1}" = 1 ]; then
  sed "s|@BIN@|$BIN|" data/org.freedesktop.FileManager1.service.in \
    > ~/.local/share/dbus-1/services/org.freedesktop.FileManager1.service
fi
update-desktop-database ~/.local/share/applications 2>/dev/null || true
if [ ! -f /usr/share/xdg-desktop-portal/portals/vector.portal ]; then
  echo "to use vector as the file picker, run once:"
  echo "  sudo install -Dm644 $PWD/data/vector.portal /usr/share/xdg-desktop-portal/portals/vector.portal && systemctl --user restart xdg-desktop-portal"
fi
echo "installed: $BIN"
