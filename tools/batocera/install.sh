#!/bin/sh
# Copies the built launcher, payloads and system profiles into a Batocera emulator
# directory (default: the rgs tree's windows-arcade-loader), keeping its launcher.yaml.
# Usage: tools/batocera/install.sh [DEST]   (run ./build.sh first)
set -e
cd "$(dirname "$0")/../.."
DEST=${1:-$HOME/rgs/Batocera/system/rgs/emulators/windows-arcade-loader}
mkdir -p "$DEST"
cp dist/arcade-launcher "$DEST/"
rm -rf "$DEST/payloads" "$DEST/systemprofiles"
cp -r dist/payloads "$DEST/payloads"
cp -r systemprofiles "$DEST/systemprofiles"
[ -f "$DEST/launcher.yaml" ] || cp tools/batocera/launcher.yaml "$DEST/"
# GUI: its wrapper and icon (the applications menu entry of the rgs tree points at them)
cp tools/batocera/arcade-launcher-gui tools/batocera/icon.png "$DEST/"
# prefixes ready to unpack: the shared one (tools/prefix-archive.sh) and games' own
# (wine-prefix/dotnet11.tar.gz: the .NET 1.1 prefix of Need for Speed Underground and Carbon), if built
for archive in wine-prefix/*.tar.gz; do
    [ -f "$archive" ] || continue
    mkdir -p "$DEST/wine-prefix"
    cp "$archive" "$DEST/wine-prefix/"
    # its version: prefixes unpacked from an older archive are updated
    [ -f "${archive%.tar.gz}.version" ] && cp "${archive%.tar.gz}.version" "$DEST/wine-prefix/"
done
echo "installed in $DEST"
