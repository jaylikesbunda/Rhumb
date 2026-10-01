#!/usr/bin/env bash
# Assembles an AppDir for Rhumb from an already-built release binary.
#
# The output is a plain directory tree with no absolute paths, which is exactly
# what appimagetool expects. Keeping this separate from the toolchain means the
# layout is reviewable and can be produced without downloading anything.
#
#   tools/make_appdir.sh [target-dir] [output-dir]
#
# Example:
#   cargo build --release && tools/make_appdir.sh

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-$root/target/release}"
out="${2:-$root/target/rhumb.AppDir}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
arch="$(uname -m)"

if [ ! -x "$target/rhumb" ]; then
    echo "no rhumb binary at $target/rhumb - run 'cargo build --release' first" >&2
    exit 1
fi

rm -rf "$out"
mkdir -p "$out/usr/bin" "$out/usr/share/applications" \
         "$out/usr/share/icons/hicolor/256x256/apps" "$out/usr/share/doc/rhumb"

install -m 0755 "$target/rhumb" "$out/usr/bin/rhumb"
install -m 0644 "$root/linux/rhumb.desktop" \
    "$out/usr/share/applications/rhumb.desktop"
install -m 0644 "$root/assets/icon.png" \
    "$out/usr/share/icons/hicolor/256x256/apps/rhumb.png"
install -m 0644 "$root/README.md" "$out/usr/share/doc/rhumb/README.md"
install -m 0755 "$root/linux/AppRun" "$out/AppRun"

# AppImages mount read-only, so anything writable has to live under $HOME.
cat > "$out/rhumb.sh" <<'EOF'
# Sets XDG dirs so a read-only AppImage mount still has somewhere to write.
: "${XDG_CONFIG_HOME:=$HOME/.config}"
: "${XDG_CACHE_HOME:=$HOME/.cache}"
: "${XDG_DATA_HOME:=$HOME/.local/share}"
export XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME
exec "$(dirname "$(readlink -f "$0")")/AppRun" "$@"
EOF
chmod 0755 "$out/rhumb.sh"

echo "AppDir ready: $out (version $version, $arch)"
