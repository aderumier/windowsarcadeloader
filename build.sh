#!/bin/sh
# Builds the Linux launcher and the Windows payload DLLs into dist/.
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
PROFILE=${PROFILE:-release}

cargo build --profile "$PROFILE" -p wal-launcher
cargo build --profile "$PROFILE" --target i686-pc-windows-gnu -p wal-payload-nesica -p wal-payload-typex -p wal-payload-globalvr -p wal-payload-tsunami -p wal-payload-rawthrills -p wal-payload-ringedge -p wal-payload-segashell -p wal-payload-wahlap -p wal-payload-europar -p wal-payload-konami -p wal-payload-namcoes3x -p wal-payload-arcadepc -p wal-payload-unis -p wal-loader
cargo build --profile "$PROFILE" --target x86_64-pc-windows-gnu -p wal-payload-namcoes3 -p wal-payload-nesica -p wal-payload-arcadepc -p wal-payload-unis -p wal-payload-konami
# the AVS boot of the 64-bit Konami game DLLs (Bishi Bashi Channel)
cargo build --profile "$PROFILE" --target x86_64-pc-windows-gnu -p wal-loader --bin wal-avs-boot

# copy + rename: works while a previous launcher is running ("Text file busy")
install_file() {
    cp "$1" "$2.tmp" && mv -f "$2.tmp" "$2"
}
mkdir -p dist/payloads
install_file "target/$PROFILE/arcade-launcher" dist/arcade-launcher
for f in wal_nesica.dll wal_typex.dll wal_globalvr.dll wal_tsunami.dll wal_rawthrills.dll wal_ringedge.dll wal_segashell.dll wal_wahlap.dll wal_europar.dll wal_konami.dll wal_bngrw.dll wal-loader.exe wal-avs-boot.exe; do
    install_file "target/i686-pc-windows-gnu/$PROFILE/$f" "dist/payloads/$f"
done
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_namcoes3.dll" dist/payloads/wal_namcoes3.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_nesica.dll" dist/payloads/wal_nesica64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_arcadepc.dll" dist/payloads/wal_arcadepc.dll
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_arcadepc.dll" dist/payloads/wal_arcadepc32.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_unis.dll" dist/payloads/wal_unis.dll
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_unis.dll" dist/payloads/wal_unis32.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_konami.dll" dist/payloads/wal_konami64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal-avs-boot.exe" dist/payloads/wal-avs-boot64.exe
# TMNT's managed input bridge (Mono's C# compiler, .NET 2.0 profile as the game's Unity)
mcs -target:library -sdk:2 -out:dist/payloads/WAL.DirectInput.dll.tmp tools/tmnt-direct-input/InputBridge.cs
mv -f dist/payloads/WAL.DirectInput.dll.tmp dist/payloads/WAL.DirectInput.dll
echo "built: dist/arcade-launcher dist/payloads/"
ls dist/payloads
