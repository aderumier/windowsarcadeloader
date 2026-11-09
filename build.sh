#!/bin/sh
# Builds the Linux launcher and the Windows payload DLLs into dist/.
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
PROFILE=${PROFILE:-release}

# LAUNCHER_FEATURES: extra launcher features (CI: sdl3/build-from-source, no SDL3 package there)
cargo build --profile "$PROFILE" -p wal-launcher ${LAUNCHER_FEATURES:+--features "$LAUNCHER_FEATURES"}
cargo build --profile "$PROFILE" --target i686-pc-windows-gnu -p wal-payload-nesica -p wal-payload-typex -p wal-payload-globalvr -p wal-payload-tsunami -p wal-payload-rawthrills -p wal-payload-ringedge -p wal-payload-seganu -p wal-payload-segashell -p wal-payload-wahlap -p wal-payload-europar -p wal-payload-konami -p wal-payload-namcoes3x -p wal-payload-arcadepc -p wal-payload-unis -p wal-payload-adrenaline -p wal-payload-apm3 -p wal-payload-alls -p wal-loader
cargo build --profile "$PROFILE" --target x86_64-pc-windows-gnu -p wal-payload-namcoes3 -p wal-payload-nesica -p wal-payload-arcadepc -p wal-payload-unis -p wal-payload-konami -p wal-payload-adrenaline -p wal-payload-apm3 -p wal-payload-alls -p wal-payload-seganu
# the AVS boot of the 64-bit Konami game DLLs (Bishi Bashi Channel)
cargo build --profile "$PROFILE" --target x86_64-pc-windows-gnu -p wal-loader --bin wal-avs-boot
# the 64-bit loader (payload_loader: Disney Crossy Road)
cargo build --profile "$PROFILE" --target x86_64-pc-windows-gnu -p wal-loader --bin wal-loader

# copy + rename: works while a previous launcher is running ("Text file busy")
install_file() {
    cp "$1" "$2.tmp" && mv -f "$2.tmp" "$2"
}
mkdir -p dist/payloads
install_file "target/$PROFILE/arcade-launcher" dist/arcade-launcher
for f in wal_nesica.dll wal_typex.dll wal_globalvr.dll wal_tsunami.dll wal_rawthrills.dll wal_ringedge.dll wal_seganu.dll wal_segashell.dll wal_wahlap.dll wal_europar.dll wal_adrenaline.dll wal_apm3.dll wal_konami.dll wal_bngrw.dll wal-loader.exe wal-avs-boot.exe wal-gasguzzlers.exe; do
    install_file "target/i686-pc-windows-gnu/$PROFILE/$f" "dist/payloads/$f"
done
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_namcoes3.dll" dist/payloads/wal_namcoes3.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_nesica.dll" dist/payloads/wal_nesica64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_arcadepc.dll" dist/payloads/wal_arcadepc.dll
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_arcadepc.dll" dist/payloads/wal_arcadepc32.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_unis.dll" dist/payloads/wal_unis.dll
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_unis.dll" dist/payloads/wal_unis32.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_konami.dll" dist/payloads/wal_konami64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_adrenaline.dll" dist/payloads/wal_adrenaline64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_apm3.dll" dist/payloads/wal_apm3_64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_alls.dll" dist/payloads/wal_alls.dll
install_file "target/i686-pc-windows-gnu/$PROFILE/wal_alls.dll" dist/payloads/wal_alls32.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal_seganu.dll" dist/payloads/wal_seganu64.dll
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal-avs-boot.exe" dist/payloads/wal-avs-boot64.exe
install_file "target/x86_64-pc-windows-gnu/$PROFILE/wal-loader.exe" dist/payloads/wal-loader64.exe
# TMNT's managed input bridge (Mono's C# compiler, .NET 2.0 profile as the game's Unity)
mcs -target:library -sdk:2 -out:dist/payloads/WAL.DirectInput.dll.tmp tools/tmnt-direct-input/InputBridge.cs
mv -f dist/payloads/WAL.DirectInput.dll.tmp dist/payloads/WAL.DirectInput.dll
echo "built: dist/arcade-launcher dist/payloads/"
ls dist/payloads
