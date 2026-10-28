#!/usr/bin/env bash
# Builds Vector Find and Vector View quietly (low priority, 3 cores) and installs them for this
# user (~/.local). Packagers: see packaging/arch/*/PKGBUILD for the system-wide layout.
#   scripts/install.sh          both
#   scripts/install.sh find     only Vector Find
#   scripts/install.sh view     only Vector View
set -euo pipefail
cd "$(dirname "$0")/.."
which="${1:-all}"
icons() { # id: the SVG plus PNG sizes (some launchers only look up raster icons)
  install -Dm644 data/$1.svg ~/.local/share/icons/hicolor/scalable/apps/$1.svg
  if command -v rsvg-convert >/dev/null; then
    for s in 48 64 128 256; do
      mkdir -p ~/.local/share/icons/hicolor/${s}x${s}/apps
      rsvg-convert -w $s -h $s data/$1.svg -o ~/.local/share/icons/hicolor/${s}x${s}/apps/$1.png
    done
  fi
  install -Dm644 data/$1.desktop ~/.local/share/applications/$1.desktop
}

if [ "$which" != view ]; then
  ID=io.github.not_bad_company.Vector
  BIN="$HOME/.local/bin/vector-find"
  nice -n 19 cargo build -j 3 --release -p vector-find
  install -Dm755 target/release/vector-find "$BIN"
  # the binary was called vector-fm before 0.2
  rm -f "$HOME/.local/bin/vector-fm"
  icons $ID
  mkdir -p ~/.local/share/dbus-1/services
  # open/save dialogs of other apps (xdg-desktop-portal FileChooser backend)
  sed "s|@BIN@|$BIN|" data/org.freedesktop.impl.portal.desktop.vector.service.in \
    > ~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.vector.service
  # "show in folder" from other apps; optional, replaces the file manager that answers it
  if [ "${VECTOR_FILEMANAGER1:-1}" = 1 ]; then
    sed "s|@BIN@|$BIN|" data/org.freedesktop.FileManager1.service.in \
      > ~/.local/share/dbus-1/services/org.freedesktop.FileManager1.service
  fi
  if [ ! -f /usr/share/xdg-desktop-portal/portals/vector.portal ]; then
    echo "to use Vector Find as the file picker, run once:"
    echo "  sudo install -Dm644 $PWD/data/vector.portal /usr/share/xdg-desktop-portal/portals/vector.portal && systemctl --user restart xdg-desktop-portal"
  fi
  echo "installed: $BIN"
fi

if [ "$which" != find ]; then
  nice -n 19 cargo build -j 3 --release -p vector-view
  install -Dm755 target/release/vector-view "$HOME/.local/bin/vector-view"
  icons io.github.not_bad_company.VectorView
  echo "installed: $HOME/.local/bin/vector-view"
fi

touch ~/.local/share/icons/hicolor
update-desktop-database ~/.local/share/applications 2>/dev/null || true
