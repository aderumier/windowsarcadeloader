#!/bin/sh
# Builds the Linux launcher and the Windows payload DLLs into dist/.
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
PROFILE=${PROFILE:-release}

cargo build --profile "$PROFILE" -p wal-launcher
cargo build --profile "$PROFILE" --target i686-pc-windows-gnu -p wal-payload-nesica

# copy + rename: works while a previous launcher is running ("Text file busy")
install_file() {
    cp "$1" "$2.tmp" && mv -f "$2.tmp" "$2"
}
mkdir -p dist/payloads
install_file "target/$PROFILE/arcade-launcher" dist/arcade-launcher
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_nesica.dll" dist/payloads/wal_nesica.dll
echo "built: dist/arcade-launcher dist/payloads/"
ls dist/payloads
