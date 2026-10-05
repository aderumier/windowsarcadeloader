# Windows Arcade Loader — design notes

> Research notes. The developer documentation of the implementation is `docs/IMPLEMENTATION.md`.

Research findings and plan, so implementation can resume from here.

## Architecture

```
Linux launcher (Rust, x86_64)                    Wine process (game.exe, i686)
 ├─ input: SDL3 (pads, wheels) + evdev (kbd)       ├─ per-system payload DLL (Rust, i686-pc-windows-gnu)
 ├─ mapping -> virtual arcade sticks (4 players)   │    ├─ payload-common: TCP client, log, mapping, IAT hooks
 ├─ TCP server 127.0.0.1:33700  <────────────────> │    └─ system crate (payload-nesica, ...) translates
 └─ wine runner + prefix setup + launch            │       virtual sticks -> native I/O emulation
```

Crates: `crates/protocol` (shared), `crates/launcher` (`arcade-launcher`), `crates/payload-common`,
`crates/payload-<system>`. Linux side per system: `crates/launcher/src/systems/<system>.rs`.
Build: `./build.sh` -> `dist/arcade-launcher`, `dist/payloads/*.dll`.

### Virtual arcade stick (per player)

d-pad, `b1 b2 b3 b4` (top row) / `b5 b6 b7 b8` (bottom row), start, coin, service, test,
left stick `lx ly`, right stick `rx ry`, pedals `accel brake`.
Wire: `[u8 kind][u8 0][u16 len][payload]`, kinds Hello / Input (4 × {u32 buttons, 6 × i16 axes}) /
Output (player, id, value).

* Linux: physical -> virtual in the profile `input` section. Gamepad defaults (Xbox labels):
  X Y R1 L1 -> b1-b4, A B R2 L2 -> b5-b8.
* Game: virtual -> native in each payload (defaults in code), per game override `native_map` (`WAL_MAP`).

### Game profiles (YAML)

`crates/launcher/src/defaults.yaml` (every key documented) <- `launcher.yaml` (optional) <-
`systemprofiles/<system>/<game>.yaml` (template) <- `userprofiles/<system>/<game>.yaml` (user), deep
merged. `exe` is a Windows path (`Z:\...`, `C:\...`). Run: `arcade-launcher <profile.yaml>`.

## Injection: per-system "best" strategy

* **NESiCA: replace `iDmacDrv32.dll`.** `game.exe` imports it statically (verified with winedump), so our DLL
  loads before the game's entry point. No remote thread, no loader-lock race (known injectors note
  CreateRemoteThread is unreliable under Wine/Box). The game folder is left untouched: the launcher builds a
  per-game run dir (`<prefix>/drive_c/wal/<system>/<game>`) of symlinks to the game files with our DLL in
  place of `iDmacDrv32.dll`, and launches from there.
  **Games import the driver by ordinal** (AH2: 1, 2, 5, 6): the payload pins the original ordinals with
  `crates/payload-nesica/iDmacDrv32.def` (1 Open, 2 Close, 3/4 Dma, 5 RegisterRead, 6 RegisterWrite, ...).
* Generic fallback for other systems: proxy a DLL the game imports (dinput8/winmm/version) with
  `WINEDLLOVERRIDES=<dll>=n,b`, or a small suspended-process + entry-point-patch loader.

## NESiCA emulation


* **FastIO** (`iDmacDrvRegisterRead/Write`, `FastIoEmu.cpp`): `iDmacDrvOpen` sets `*out=284, *flag=0`.
  Reads: 0x400→0x00010201, 0x4000→0x00FF00FF, 0x4004→0x00FF0000, 0x4120→bytes[0..4] (P1/P2),
  0x4124→0x01100000, 0x4128→bytes[8]|bytes[9]<<8, 0x4140→coin (edge: 1 once per press), 0x4144/0x41C4→bytes[5],
  0x41A0→bytes[10..14] (P3/P4), 0x4150→0x1823C, 0x41A4→0x01100000, others 0. `*DeviceResult=0`.
  Writes: set `*DeviceResult=-1` for 0x4000/0x4004/0x4100..0x410C (except 0x4108), 0x4180..0x418C.
* **Byte layout**:
  - byte0: P1 start 0x10, P2 start 0x20, test 0x40, P1 service 0x04, P2 service 0x08, ext1 0x01, ext2 0x02, ext3 0x80
  - byte1: P1 up 0x01 down 0x04 left 0x10 right 0x40; P2 up 0x02 down 0x08 left 0x20 right 0x80
  - byte2: P1 b1 0x01 b2 0x04 b3 0x10 b4 0x40; P2 b1 0x02 b2 0x08 b3 0x20 b4 0x80
  - byte3: P1 b5 0x01 b6 0x04; P2 b5 0x02 b6 0x08
  - byte4: coin1; bytes 8/9 analog; bytes 10–14 same layout for P3/P4; bytes 15–20 analogs.
* **AH2 mapping**: A=Button1, B=Button2, C=Button3, D=Button4,
  **E=Button6**.
* **Registry** (`RegHooks.cpp`): implemented without hooks: the DLL writes `HKLM\SOFTWARE\TAITO\NESiCAxLive` (32-bit view /
  Wow6432Node) from the DLL at attach time: CoinCredit=0 (free play), Resolution=1 (HD; 0=SD), ScreenVertical=0,
  EventModeEnable, UserSelectEnable=0, GameResult, IOErrorCoin, IOErrorCredit, SystemType, ConditionTime=300,
  EventNextTime=900, GameKind=1234, LogLevel=0, TrafficCount=2, UpdateStep=0, Country=1, AppVer=1.
  Same for `SOFTWARE\TAITO\TYPEX`.
* **D:\ redirection** (`D:\x` → `.\WindowsLoader\x`): IAT hooks on game.exe file APIs rewrite
  `D:\` to `<game dir>\WindowsLoader` (`WAL_NESICA_DDRIVE`), no D: drive in the prefix. NESYS card files go
  there too.
* **NESYS** (`NesysEmu.cpp`): message-mode named pipe `\\.\pipe\nesys_games`, header `{u32 cmd, u32 len}`;
  reply table in that file. Ported (`WAL_NESICA_NESYS=0` disables it).
  game.exe references `\\.\pipe\ED1A8B17-4606-47f2-AE0C-CF29207A7F7E` (unknown purpose; check with WINEDEBUG).
* **Crypto pipe** `\\.\pipe\TtxAppCtyptPipe` (`CryptoPipe.cpp`): not needed for AH2 (NesicaKey::None).
* **RFID** on COM2 (`RfidEmu.cpp`): port later if needed.
* Game detection: CRC32 of the first 0x400 bytes of the image. Arcana Heart 2 `game.exe` = `0x42243e28`.

## Graphics (game folder contents)

* `DDraw.dll` + `D3DImm.dll` = dgVoodoo2 2.55 (`dgVoodoo.conf`), `d3d11.dll` = ReShade, `Dinput8.dll` =
  stock Microsoft dinput8 (force `dinput8=b`).
* Configurable modes: `wine` (default: `ddraw,d3dimm,d3d11=b`), `d7vk` (runner `lib/wine/d7vk`),
  `dgvoodoo` (`ddraw,d3dimm=n`, d3d11 = builtin/DXVK; skip ReShade).

## Wine launch (from batocera-wine)

* Runner `wine-runners/GE-Proton11-7-x86_64`: `bin/wine`, `bin/wineserver`, dlls in `lib/wine/{i386,x86_64}-{windows,unix}`.
* Library order (important): `LD_LIBRARY_PATH=<lib32>:<runner>/lib/wine/i386-unix:<lib>:/usr/lib:<runner>/lib/wine/x86_64-unix`,
  then append `<runner>/lib/x86_64-linux-gnu:<runner>/lib/i386-linux-gnu` (FFmpeg for winedmo) last.
  `WINEDLLPATH=<runner>/lib/wine/i386-windows:<runner>/lib/wine/x86_64-windows`.
* GE-Proton needs vkd3d + icu DLLs symlinked into system32/syswow64 after `wineboot -u`
  (`geproton_libs_install`); re-run `wineboot -u` when `share/wine/wine.inf` mtime changes (`.update-timestamp`).
* Env: `WINEDEBUG=-all`, `WINEDLLOVERRIDES=winemenubuilder.exe=;…`, `WINE_ENABLE_HIDRAW`, ntsync (`/dev/ntsync`).
* Prefix: `wine-prefix/common` (default).

## Lightgun

Port `batocera-wine-guns` behaviour (evdev guns via `ID_INPUT_GUN`, mouse fallback, DemulShooter packets to
port 33610) into the launcher's gun input backend, reusing the common ArcadeState.

## Toolchain

rustup (in ~/.cargo, not in the shell PATH: build.sh adds it) with `i686-pc-windows-gnu` +
`x86_64-pc-windows-gnu`, and the `mingw-w64-gcc` pacman package.

## Usage

```
./build.sh                                              # dist/arcade-launcher + dist/payloads/
dist/arcade-launcher systemprofiles/nesica/arcana-heart-2.yaml            # play
dist/arcade-launcher systemprofiles/nesica/arcana-heart-2.yaml --dry-run  # env + command only
dist/arcade-launcher input-test [profile]               # show the virtual sticks live
dist/arcade-launcher show <profile>                     # merged profile
WINEDEBUG=+loaddll,err+all dist/arcade-launcher <profile>                 # wine debug channels
```

Payload log: `<prefix>/drive_c/wal/<system>/<game>/wal-<system>.log` (FastIO commands, input changes).

## Status

* Arcana Heart 2 boots to attract mode (wine builtin ddraw, WoW64 mode). FastIO input verified end to end
  with a scripted launcher (coin, start, directions, buttons, b5 -> Button6).
* Not verified yet: live keyboard/gamepad play session, dgvoodoo/d7vk modes, NESYS (AH2 does not use it).
* TODO: lightgun backend (port of batocera-wine-guns), outputs (lamps/FFB), more systems.
