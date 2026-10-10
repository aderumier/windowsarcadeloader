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
# shared prefix ready to unpack (tools/prefix-archive.sh), if built
if [ -f wine-prefix/full.tar.gz ]; then
    mkdir -p "$DEST/wine-prefix"
    cp wine-prefix/full.tar.gz "$DEST/wine-prefix/"
fi
echo "installed in $DEST"
