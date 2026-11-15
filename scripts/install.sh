#!/bin/sh
# Installs zetwerk for this user: the program into ~/.local/bin, and
# what a desktop needs to open .typ files with it (an entry in the
# list of applications, an icon). Nothing is installed system-wide and
# nothing needs root. `./scripts/install.sh remove` takes it all away.
set -eu
cd "$(dirname "$0")/.."
bin=${XDG_BIN_HOME:-$HOME/.local/bin}
data=${XDG_DATA_HOME:-$HOME/.local/share}

if [ "${1:-}" = remove ]; then
    rm -f "$bin/zetwerk" "$data/applications/zetwerk.desktop" \
        "$data/icons/hicolor/scalable/apps/zetwerk.svg"
    echo "removed"
    exit 0
fi

cargo build --release --workspace "$@"
install -Dm755 target/release/zetwerk "$bin/zetwerk"
install -Dm644 packaging/zetwerk.desktop "$data/applications/zetwerk.desktop"
install -Dm644 packaging/zetwerk.svg "$data/icons/hicolor/scalable/apps/zetwerk.svg"
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
echo "installed: $bin/zetwerk"
case ":$PATH:" in
*":$bin:"*) ;;
*) echo "note: $bin is not on your PATH" ;;
esac
echo "to open .typ files with it from the file manager:"
echo "  xdg-mime default zetwerk.desktop text/vnd.typst"
