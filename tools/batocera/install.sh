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
echo "installed in $DEST"
