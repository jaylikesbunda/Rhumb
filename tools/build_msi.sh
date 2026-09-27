#!/usr/bin/env bash
# Builds the Xplor MSI.
#
# Drives candle and light directly rather than going through cargo-wix, so the
# version in the installer always comes from Cargo.toml with no template engine
# in between. Run from anywhere:
#
#   tools/build_msi.sh
#
# Needs the WiX Toolset v3 on PATH (candle.exe and light.exe). On a GitHub
# windows-latest runner it is already installed; elsewhere install it, or
# download the binaries zip and add its folder to PATH.
#
# Output: dist/xplor-<version>-x64.msi

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for tool in candle light; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found on PATH." >&2
        echo "Install the WiX Toolset v3, or add its bin folder to PATH." >&2
        exit 1
    fi
done

if [ ! -f "target/release/xplor.exe" ]; then
    echo "error: target/release/xplor.exe is missing." >&2
    echo "Run 'cargo build --release' first." >&2
    exit 1
fi

# The version has to reach WiX as up to four numbers. A two or three part
# version is padded, because the MSI schema rejects anything else.
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "$version" ]; then
    echo "error: could not read the version from Cargo.toml" >&2
    exit 1
fi
case "$version" in
    *[!0-9.]*)
        echo "error: version '$version' is not numeric; the MSI schema needs digits" >&2
        exit 1
        ;;
esac
major="${version%%.*}"; rest="${version#*.}"; minor="${rest%%.*}"; patch="${rest#*.}"
patch="${patch%%.*}"
msi_version="$major.$minor.$patch.0"

build="target/msi"
rm -rf "$build"
mkdir -p "$build" dist

echo "Building xplor $msi_version from $version"

# -arch x64 is what puts x64 in the Template Summary property, which ICE80
# checks against the 64-bit components. Without it the link fails.
candle -nologo \
    -arch x64 \
    -dPkgVersion="$msi_version" \
    -out "$build/main.wxs" \
    wix/main.wxs

light -nologo \
    -out "dist/xplor-${version}-x64.msi" \
    "$build/main.wxs"

echo "Wrote dist/xplor-${version}-x64.msi"
ls -l "dist/xplor-${version}-x64.msi"
