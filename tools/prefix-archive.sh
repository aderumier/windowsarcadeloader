#!/bin/sh
# Builds the shared prefix archive (wine-prefix/full.tar.gz): a fresh prefix with every
# winetricks verb of prefix_tricks, which the launcher unpacks when the prefix does not exist
# yet (no winetricks downloads on a machine's first start). Built with the runner of the
# machine's launcher.yaml: use the same runner on the target.
# OUT's version (OUT without .tar.gz, plus .version: a build timestamp) makes the launcher
# replace prefixes unpacked from an older archive (saves and run directories kept).
# Usage: tools/prefix-archive.sh [OUT]   (run ./build.sh first)
set -e
cd "$(dirname "$0")/.."
OUT=${1:-wine-prefix/full.tar.gz}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# any game of the shared prefix: prepare installs prefix_tricks
printf 'prefix: %s\n' "$TMP/prefix" > "$TMP/layer.yaml"
dist/arcade-launcher prepare puzzle-bobble --profile "$TMP/layer.yaml"
mkdir -p "$(dirname "$OUT")"
tar czf "$OUT.tmp" -C "$TMP/prefix" .
mv "$OUT.tmp" "$OUT"
date -u +%Y%m%d%H%M%S > "${OUT%.tar.gz}.version"
cat "${OUT%.tar.gz}.version"
ls -la "$OUT"
