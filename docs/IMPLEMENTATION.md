# Implementation guide

Developer documentation of the Windows arcade loader: how the code is organised, how the
pieces talk to each other, the non-obvious decisions, and how to extend and debug it.
`DESIGN.md` keeps the original research notes (WindowsLoader/WindowsLoader findings).

## 1. Overview

```
 Linux: arcade-launcher (crates/launcher)          Wine: game.exe + payload DLL (crates/payload-<system>)

 SDL3 pads/wheels --+                              +--> TCP client --> 4 virtual sticks (atomics)
 evdev keyboards ---+--> mapping (YAML profile)    |                         |
                         |                         |                         v
                         v                         |    system emulation: the native I/O API
                    4 virtual arcade sticks --TCP--+    the game calls (e.g. iDmacDrv32 FastIO)
                                       127.0.0.1:33700

 YAML profile --> wine env, prefix, graphics, run directory --> spawn wine game.exe
```

* The launcher owns everything physical: devices, mapping, wine runner, prefix, process.
* The payload owns everything native: it exposes the arcade system's I/O API to the game and
  converts the virtual arcade sticks into native values.
* The two sides only share `crates/protocol`. No system-specific knowledge crosses the socket.

## 2. Repository layout

```
Cargo.toml                 workspace; default members = protocol + launcher (Linux build)
.cargo/config.toml         MinGW linkers for the *-pc-windows-gnu targets
build.sh                   builds everything into dist/
crates/
  protocol/                shared: virtual stick model + wire format (std only, no deps)
  launcher/                Linux binary `arcade-launcher`
    src/main.rs            CLI, run loop
    src/config.rs          layered YAML profiles
    src/defaults.yaml      built-in defaults, documents every profile key (include_str!)
    src/mapping.rs         physical source -> virtual target engine
    src/input.rs           SDL3 + evdev devices, player slots (Hub)
    src/server.rs          TCP server
    src/wine.rs            runner env, prefix, dll links, path conversion
    src/rundir.rs          symlinked run directory
    src/systems/           Linux-side description of each system (payload file, hidden files)
  payload-common/          shared by every payload DLL: TCP client, log, mapping, IAT hooks, paths
  loader/                  wal-loader.exe: starts a game with a payload loaded before its entry point
  payload-typex/           Taito Type X payload (JVS I/O board on COM2), loaded by wal-loader
  payload-nesica/          NESiCAxLive payload = replacement iDmacDrv32.dll
    iDmacDrv32.def         export ordinals of the original driver (see §6.2)
    src/keys.rs            built-in crypto service keys (from WindowsLoader)
    src/news.png           NESYS news picture (from WindowsLoader)
    build.rs               passes the .def to the linker
systemprofiles/<system>/<game>.yaml   game templates (shipped)
userprofiles/<system>/<game>.yaml     user overrides (git-ignored)
tools/fake_launcher.py     scripted stand-in for the launcher (payload tests)
tools/scripts/             input scripts for `--input-script` (coin-start-mash.txt)
dist/                      build output: arcade-launcher, payloads/*.dll
games/, wine-runners/, wine-prefix/   local data (git-ignored)
```

## 3. Toolchain and build

* Launcher: host Rust (x86_64-unknown-linux-gnu), links the system SDL3 (`sdl3` crate,
  dynamic) and reads `/dev/input` (user must be in the `input` group).
* Payloads: `i686-pc-windows-gnu` (32-bit games) via rustup in `~/.cargo` (not in the shell
  PATH: `build.sh` prepends it) + `mingw-w64-gcc`. `x86_64-pc-windows-gnu` is installed too
  for future 64-bit games.
* `./build.sh` (env `PROFILE=dev` for debug builds) produces `dist/arcade-launcher` and
  `dist/payloads/wal_nesica.dll`. A new payload must be added to `build.sh`.
* `panic = "abort"` everywhere: no unwinding across the FFI boundary and no libgcc DLL to ship.
  The DLL only imports Windows system DLLs + UCRT api-sets (Wine provides them); check with
  `winedump -j import dist/payloads/<dll>`.
* Known harmless linker warning: `resolving _DllMain by linking to _DllMain@12`.
* Tests: `cargo test -p wal-launcher -p wal-protocol -p wal-payload-common` (host). Keep
  payload logic that can be tested in `payload-common` (pure Rust), not in the Windows-only
  crates.

## 4. Protocol (`crates/protocol`)

### 4.1 Virtual arcade stick

One per player, `MAX_PLAYERS = 4`:

| kind | names | representation |
|---|---|---|
| directions | `up down left right` | bits 0-3 of `buttons` |
| buttons | `b1 b2 b3 b4` (top row), `b5 b6 b7 b8` (bottom row) | bits 4-11 |
| system | `start coin service test` | bits 12-15 |
| card | `card` (toggles the player's card in card readers) | bit 16 |
| sticks | `lx ly rx ry` | i16, centered 0, +y = down |
| pedals | `accel brake` | i16, 0 released .. 32767 pressed |

`StickState::axis_u8()` gives the 0..255 form most arcade I/O boards use (128 = center).
Names are the single vocabulary used by profiles (`button::from_name`, `Axis::from_name`).

### 4.2 Wire format

TCP on 127.0.0.1, launcher = server (`port`, default 33700), payload = client.
Frame: `[u8 kind][u8 0][u16 LE payload_len][payload]`.

| kind | direction | payload |
|---|---|---|
| 1 Hello | both, first message | u16 version, UTF-8 name |
| 2 Input | launcher → game | 4 × (u32 buttons + 6 × i16 axes) = 64 bytes |
| 3 Output | game → launcher | u8 player, u16 id, i32 value |

The launcher sends the full state on every change plus every 100 ms, so a reconnecting payload
is up to date immediately; unknown kinds are skipped (forward compatible). Bump `VERSION` on an
incompatible change. Output ids are system-defined (none used yet: lamps, FFB, recoil are TODO).

### 4.3 Environment contract (launcher → payload)

| variable | meaning |
|---|---|
| `WAL_PORT` | TCP port to connect to |
| `WAL_LOG` | Windows path of the payload log file |
| `WAL_MAP` | `virtual=native,...` override of the payload's default mapping (from profile `native_map`) |
| `WAL_<SYSTEM>_*` | system options, from profile `env` (see the system module docs) |

## 5. Launcher (`crates/launcher`)

### 5.1 Profiles (`config.rs`, `defaults.yaml`)

Layers, deep-merged in order (maps key by key, scalars/lists replaced):

1. `src/defaults.yaml` (compiled in; the reference for every key)
2. `<root>/launcher.yaml` (optional, machine wide)
3. `<root>/systemprofiles/<rel>` (template)
4. `<root>/userprofiles/<rel>` (user overrides)

`arcade-launcher <file>` accepts a file from either profiles directory: `<root>` and `<rel>` are
derived from its path, so both layers are always merged. An id `<system>/<game>` is resolved
against `--root` (default cwd). The merged value is deserialized with `deny_unknown_fields`,
so typos fail loudly; `system` and `exe` are checked first for a clear error.

Adding a profile key: add it to `defaults.yaml` (with a comment) **and** to `Profile` /
`InputConfig`; every key needs a default in the YAML since the struct has no serde defaults.

Mapping tables can disable an inherited entry with the value `none`.

### 5.2 Input (`mapping.rs`, `input.rs`)

* `mapping.rs` is pure and unit tested. A table `source -> target` compiles into
  `Vec<(Source, Target)>`:
  * `Source` = a `RawKey` (pad button/axis, joystick button/axis/hat, key) + a `Range`
    (digital with mask, full axis, positive or negative half).
  * Source names: SDL positional with Xbox aliases (`a b x y l1 r1 l2 r2 l3 r3 dpup ...`),
    raw joystick (`button3 axis1 +axis2 -axis2 hat0up`), evdev keys (`KEY_Z`).
  * `Target` = a button bit, or an axis with optional inversion (`-ly`, `-accel`).
  * Conversions: digital → stick = full deflection (sign from inversion), digital → pedal = full;
    analog → button = past `deadzone`; full axis → pedal = rescaled to 0..32767; triggers and
    half axes are already 0..32767.
* `DeviceState` keeps the raw state of one device (`HashMap<RawKey, i32>`) and computes its
  contribution on demand. `merge()` ORs buttons and keeps the largest-magnitude axis per player,
  then applies `stick_as_dpad`.
* `Hub` (input.rs):
  * SDL3 runs on the main thread (`wait_event_timeout` 4 ms). `SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1`
    is required: the game window has the focus.
  * Device add uses `JoyDeviceAdded` only (SDL also emits `GamepadAdded`). Gamepads known to SDL
    use `input.gamepad` (+ the device's `map` extras); other joysticks (wheels) are only opened
    if an `input.devices` entry matches their name, with raw sources. `raw: true` forces raw
    mode on a gamepad. SDL emits both Joy* and Gamepad* events for gamepads: `Hub::set` keeps
    only the kind matching how the device was opened.
  * Initial joystick axes are read at open time (pedals rest at non-zero values).
  * Player slots: `devices.<name>.player`, else the lowest slot not used by another SDL device.
    Keyboards are fixed per `input.keyboard.pN` and share one global key state.
  * evdev keyboards: one reader thread per device that has KEY_A and KEY_ENTER, no grab (the
    game still receives keys through X11), autorepeat ignored; events reach the main thread
    over an mpsc channel.
  * `exit_combo` (physical gamepad names) sets `exit_requested`.

### 5.3 Wine (`wine.rs`)

* Environment, following batocera-wine:
  * `LD_LIBRARY_PATH` order matters: 32-bit system libs, runner `i386-unix`, 64-bit system libs,
    runner `x86_64-unix`, then the runner's bundled FFmpeg (`lib/{x86_64,i386}-linux-gnu`) last,
    then any inherited value. System dirs are auto-detected and de-duplicated by canonical path
    (`/lib` vs `/usr/lib`).
  * `WINEDLLPATH`, `WINEPREFIX`, `PATH` (runner bin first), ntsync detection
    (`WINEDISABLEFASTSYNC`), quiet DXVK/VKD3D logs, `WINEDEBUG` (a value in the shell wins
    over the profile, for debugging).
  * `wow64: true` (default) sets `WINEARCH=wow64`: GE-Proton runs 32-bit games with the 64-bit
    host libraries. Without it, the runner's `i386-unix/*.so` need 32-bit host libs
    (pulse/alsa/freetype); on a host without them, audio init fails and XACT games exit (AH2
    exits with status 5).
  * `WINEDLLOVERRIDES` is accumulated by `override_dll()` and joined at the end.
* Prefix (`prepare_prefix`): created with `wineboot -u` (mono/gecko disabled) when
  `system.reg` is missing; updated when the mtime of the runner's `share/wine/wine.inf` differs
  from `<prefix>/.wal-update-timestamp` (symlinks in system32/syswow64 are removed first).
  GE-Proton's vkd3d and icu DLLs are always symlinked into system32 (x86_64) / syswow64 (i386).
* Graphics: `setup_d3d` links DXVK (or wine builtin) d3d8/9/10core/11/dxgi into the prefix and
  sets the override to `n` (or `b`). `setup_ddraw`: `wine` = builtin ddraw; `d7vk` = runner d7vk
  as ddraw.dll plus builtin as `ddraw_.dll`; `dgvoodoo` = the game's DDraw.dll/D3DImm.dll
  (`n,b`). Runner layouts differ: `builtin_dir`/`component_dir` probe several locations.
* Paths: `unix_path()` converts the profile's Windows exe path (drive via
  `<prefix>/dosdevices/<x>:`, falling back to drive_c / `/` for C: and Z:), matching each
  component case-insensitively. `windows_path()` is the reverse (drive_c → `C:\`, else `Z:\`).
* `dry_run` makes every disk-changing step a no-op so `--dry-run` prints the exact env/command.

Prefix tricks: profile `tricks` lists winetricks verbs applied once per prefix with the runner's
wine (`winetricks -q <verb>`, `WINEARCH` removed), recorded in `<prefix>/.wal-tricks`. The prefix
is shared, so tricks affect every game using it.

Runner hotfixes: `tools/runner-hotfixes.py [runner...]` patches known bugs of runner builds in
place (exact byte match, `<file>.orig` backup, idempotent), until the fix ships in a release:
* `winedmo-wow64-demuxer-destroy` (GE-Proton11-7 `winedmo.so`): the WoW64 thunk of
  `demuxer_destroy` used the create params layout, so 32-bit games destroyed a garbage demuxer
  handle; Battle Fantasia aborted with `free(): invalid size` when a coin stops the attract
  movie. Upstream Wine 2110c64d89cc; proposed for GE-Proton as a `wine-hotfixes/pending` backport.

### 5.4 Run directory (`rundir.rs`)

`<prefix>/drive_c/wal/<system>/<game>/`: one symlink per top-level game entry, except hidden
files and the files replaced by payloads; payload DLLs are copied in under the replaced name.
The game is started from there (cwd + `C:\wal\...\game.exe`).

* The game directory is never modified, payloads stay per-game (no global DLL override).
* Old symlinks are removed on every launch; regular files are kept: files the game creates
  next to its exe persist in the run dir. Writes inside symlinked sub directories go to the game
  directory.
* Hidden: profile `hide` + `System::hidden()` + `ddraw.dll`/`d3dimm.dll` for the `wine`/`d7vk`
  graphics modes (otherwise dgVoodoo in the game dir would win the DLL search).

### 5.5 Run loop (`main.rs`)

`run()`: load profile → system → `Wine::new` → resolve exe → check payload files → prefix +
graphics → run dir → payload env → print summary → (dry run stops here) → TCP server →
input hub → spawn wine → loop { poll input 4 ms, broadcast on change / 100 ms, exit combo or
Ctrl-C/SIGTERM → `wineserver -k`, child exit → break } → `wineserver -k` for leftovers.

Subcommands: `run` (default), `input-test` (prints the merged sticks; works without a game
profile), `show` (merged profile).

## 6. Payloads

### 6.1 Common (`crates/payload-common`)

* `start(name)`: once; opens the log (`WAL_LOG`), spawns the connection thread (reconnects
  every 500 ms; on disconnect all inputs are released).
* `input(player)`: lock-free read of the stick (atomics); call it from game threads, it never
  blocks. Fields are individually atomic (a frame can tear between two fields, harmless at
  60+ Hz polling).
* `send_output(player, id, value)`.
* `log!(...)`: timestamped lines in the log file; also logs P1 button changes (input tracing).
* `mapping::ButtonMap<N>`: `natives` (name → native id) + defaults (`virtual → native name`),
  then `WAL_MAP` overrides; `for_each_pressed()` iterates the native inputs pressed on a stick.
* `iat::hook(dll, function, replacement)`: patches the **game executable's** import table
  (only the game module's calls are redirected, nothing in system DLLs, no code patching).
  Returns the original pointer. Limits: not for `GetProcAddress`-resolved calls, nor calls from
  other DLLs; use an inline hook library if a system ever needs that.
* `paths::rewrite_drive(path, letter, target)`: generic narrow/wide drive-letter rewrite
  (unit tested), used by the hooked file APIs.
* `dshow`: DirectShow shims for games written against Windows' DirectShow, installed by an IAT
  hook of `ole32!CoCreateInstance` that wraps methods of the graph's shared vtable:
  * `WAL_DSHOW_FIND_FILTER=1` (KOF XIII Climax): `IFilterGraph::FindFilterByName` looks the
    filter up itself by the names filters report (exact, or with Wine's ` 0001` suffix) before
    Wine's. Wine returned the async file reader ("Reader") for `"WMVideo Decoder DMO"`; the game
    then asked it for pin `out0`, connected nothing to its SampleGrabber and crashed reading the
    unchecked `GetConnectedMediaType` (NULL `pbFormat`). With the shim: `Connect(WMVideo Decoder
    DMO:out0 -> SampleGrabber)` and movies play, on Wine's default graph (async reader + winedmo
    ASF splitter + DMO decoders).
  * `WAL_DSHOW_ASF_READER=1`: `RenderFile` of ASF files through the WM ASF Reader (Windows'
    topology). Kept as an option; with Wine's reader the WMA Lossless audio stream of KOF's movies
    is never delivered and playback stalls.
  * `WAL_DSHOW_TRACE=1`: logs FindFilterByName and IGraphBuilder::Connect calls.
  GE-Proton 11 has no GStreamer backend: winedmo (FFmpeg) is the only media path; its
  `demuxer_destroy` faults during probing are caught by Wine.
* `serial`: emulated serial device (IAT hooks of CreateFile/ReadFile/WriteFile/comm calls,
  fake handle, reply queue); used by the NESiCA card reader and the Type X JVS board. Logs the
  first 40 packets and any other COM port the game opens.
* `drive`: `D:\` redirection (moved from NESiCA), folder variable chosen by the system
  (`WAL_NESICA_DDRIVE`, `WAL_TYPEX_DDRIVE`).
* `patches`: `WAL_PATCHES=<rva>:<hex>,...` game code patches from the profile (keeps game
  knowledge in profiles, e.g. WindowsLoader's per-game patches).
* `crash`: vectored exception handler logging the first access violations: address as
  `module+offset`, registers, EBP frame chain, stack scan, and for write overruns the text
  being written (found Cosplay Mahjong's overflow of a D3DX error message).
* `screenshot` (d3d9 shims): `WAL_SCREENSHOT=<s>` writes the back buffer every s seconds
  (`shot-NNNN.bmp` in the run dir, for automated tests); `WAL_D3D9_FULLSCREEN_SIZE=WxH`
  creates/resets fullscreen devices with that size (Type X games ask 1280x768, which Wine does
  not emulate: 1280x800 + Wine's fullscreen scaling). Logs the parameters of failing
  CreateDevice calls.
* `jvs`: JVS packet framing (`E0` sync, `D0` escaping, size, checksum) for emulated I/O
  boards on serial ports (unit tested; output identical to WindowsLoader/ttx_monitor).
* IAT hooks chain: hooking the same import twice makes the second hook call the first one.
  NESiCA installs the D: hooks, then the card reader's CreateFile hooks in front of them.

DllMain rules (all payloads): only cheap work in `DLL_PROCESS_ATTACH` — spawning threads is
fine (they start after the loader lock is released), registry writes and IAT patching are fine
under Wine. Never wait on a thread from DllMain.

### 6.2 NESiCA (`crates/payload-nesica`)

Built as `wal_nesica.dll`, installed as `iDmacDrv32.dll` (Taito Type X FastIO driver). Games
import it statically, so it loads before the game entry point: no injector needed.

* **Ordinals**: games import the driver **by ordinal** (AH2: 1, 2, 5, 6). Rust/MinGW assigns
  ordinals alphabetically (DllMain would be #1), so `iDmacDrv32.def` pins the original ones:
  1 Open, 2 Close, 3 DmaRead, 4 DmaWrite, 5 RegisterRead, 6 RegisterWrite, 7/8 RegisterBuffer
  Read/Write, 9/10 Memory Read/Write, 11/12 MemoryBuffer Read/Write, 13/14 Memory Read/WriteExt.
  Verify with `winedump -j export dist/payloads/wal_nesica.dll`. Any other replacement-DLL
  payload must do the same.
* **FastIO** (`fastio.rs`, port of WindowsLoader `FastIoEmu.cpp`): `iDmacDrvRegisterRead` answers
  per register; inputs are built on demand from the sticks into the WindowsLoader byte layout:

  | register | value |
  |---|---|
  | 0x4120 | bytes 0..3: P1/P2 (byte0 start/service/test/ext, byte1 dirs, byte2 btn1-4, byte3 btn5-6) |
  | 0x41A0 | bytes 10..13: P3/P4, same layout |
  | 0x4128 | byte8 = P1 `lx`, byte9 = P1 `accel` (0..255) |
  | 0x4140 | coin: 1 once per press (edge), the game acks by writing 0x4140 |
  | 0x400, 0x4000, 0x4004, 0x4124, 0x41A4, 0x4150 | constants from WindowsLoader |

  P1/P2 share bytes: P2 is the next bit up of each pair; test/ext are cabinet-wide.
  Native names for `native_map`: `up down left right start coin service test btn1..btn6
  ext1..ext3`. Default mapping: b1-b6 → btn1-btn6. Writes set `*DeviceResult = -1` for the
  acknowledged registers (0x4000, 0x4004, 0x4100-0x410C, 0x4180-0x418C).
  `trace()` logs each register the first time and every change of the input registers.
* **Registry** (`registry.rs`): writes `HKLM\SOFTWARE\TAITO\NESiCAxLive` and `...\TYPEX`
  values at load (32-bit view, Wine redirects to Wow6432Node) instead of hooking the registry
  API. Values the game writes back (GameResult, IOError*) are reset each launch.
  `WAL_NESICA_REG=Name=value,...` overrides (e.g. `Resolution=0` for SD).
* **D: drive** (`drive.rs`): IAT hooks on the game's kernel32 file APIs (CreateFile,
  GetFileAttributes(Ex), Create/RemoveDirectory, DeleteFile, FindFirstFile(Ex), SetCurrentDirectory,
  GetDiskFreeSpaceEx, MoveFile(Ex), CopyFile, A and W) rewrite `D:\...` to
  `<exe dir>\WindowsLoader\...` (`WAL_NESICA_DDRIVE`: relative or absolute). The run dir's
  `WindowsLoader` entry is a symlink, so data lands in the game directory (WindowsLoader-compatible
  saves). The hook table is a macro: index, name, path arguments, other arguments.
* **Crypto server** (`crypto.rs`, port of WindowsLoader `CryptoPipe.cpp`): byte-mode pipe
  `\\.\pipe\TtxAppCtyptPipe`. Frames `FF FD <u32 size> <PUBLICKEYBLOB>` (game RSA key) and
  `FF FE <u32 size> <SIMPLEBLOB>` (content key encrypted for the service key); `FF FF` escapes
  `FF` in the size. The service key (PRIVATEKEYBLOB, 308 bytes) is `WAL_NESICA_KEY`: a built-in
  name from `keys.rs` (extracted from WindowsLoader: magicalbeat, bbcf, persona4arena, bbcp,
  kofxiiiclimax, persona4ultimix, usf4, darius) or a file in the game directory (KOF XIII Climax
  ships `303002.key`, identical to the built-in one). Replies `<u32 len><blob>` (`<u32 0>` on
  error): SIMPLEBLOB for the game key, or for KOF XIII Climax (`WAL_NESICA_CRYPT_REPLY=plaintext`)
  a PLAINTEXTKEYBLOB RSA-encrypted with the game key. Uses CryptoAPI (Wine rsaenh).
  Wine fix: BBCF imports the reply with `CryptImportKey(SIMPLEBLOB, hPubKey = 0)`, which Windows
  resolves to the container's AT_KEYEXCHANGE key but Wine rejects (NTE_BAD_PUBLIC_KEY, then the
  game asserts "Decrypt failed" when a match starts; WindowsLoader under Wine has the same bug).
  The game's `CryptImportKey` is IAT-hooked to pass `CryptGetUserKey(AT_KEYEXCHANGE)`.
* **NESYS message size**: requests are read whole (`ERROR_MORE_DATA` loop); BBCF uploads a
  ~60 KB play log (`UPLOAD_CONFIG`), which used to break the pipe and show "NESiCA offline".
* **BBCF shop hours**: NESiCA is disabled outside the test menu OPEN/CLOSE TIME (red
  "閉店時間を過ぎました" banner); the profile patches the check (`0x7EA80`) to always open.
  `WAL_TRACE_CRYPT=1` logs the game's CryptoAPI calls (`crypttrace.rs`); `WAL_TRACE_FILES=1`
  logs every file the game opens. BBCF data (`TXAC` .pac): `TXAC`, u32 0x10, u32 0x4C, u32 0,
  76-byte SIMPLEBLOB (RC4 40-bit, 11 zero salt bytes), RC4 data from 0x60 (`FPAC`).
  `WAL_NESICA_CRYPT=0` disables it. Verified with KOF XIII: the game decrypts the 17-byte RC4
  content key and loads its resources.
* **Card reader** (`rfid.rs`, port of WindowsLoader `RfidEmu.cpp`): Taito RFID board on `COM2`
  (`WAL_NESICA_RFID_PORT`), JVS framing. IAT hooks on CreateFileA/W (fake handle `0x1337`),
  Read/WriteFile, CloseHandle and the comm functions; written packets are answered into a reply
  queue. Commands: F0 reset, F1 address, 01/03/04/05 Taito, 10 id
  (`TAITO CORP.;RFID CTRL P.C.B.;Ver1.00;`), 11-13 revisions, 14 features
  (`01 07 00 08 00 12 08 00 00 00`), 20 switches, 21 coins, 26 card presence (`0x19`),
  30/31 coins, 32 card data (24 bytes: `04 C2 3D DA 6F 52 80 00` + 16-char card id,
  `WAL_NESICA_CARD_ID`, default `7020392010281502`). The virtual `card` input (F4 by default)
  toggles insertion. `WAL_NESICA_RFID=0` disables it. **Games with a card slot quit by
  themselves when the port cannot be opened** (KOF XIII Climax closed its window ~0.5 s after
  the NESYS connect).
* **Data folder**: the launcher creates `<game>/WindowsLoader` (`System::data_dirs`) before
  building the run dir, the DLL creates it too and writes WindowsLoader's `news.png` (NESYS news
  picture, referenced by the CERT_INIT reply) when missing.
* **NESYS** (`nesys.rs`, port of WindowsLoader `NesysEmu.cpp`, corrected with
  [FakeNesicaService](https://github.com/ArcadeMachinist/FakeNesicaService) whose layouts come
  from captures of the real service). **The connect sequence must be `CONNECT_REPLY` →
  `NWRECOVER_NOTICE` (0x103, network up) → `CERT_INIT_NOTICE`**: without the 0x103 notice
  (missing in WindowsLoader) KOF XIII Climax stays on ネットワークを初期化しています ("initializing
  network"). Real-capture formats used: LOCALNW_INFO_NOTICE interface block = MAC, IP, gateway,
  DNS, `u32 0x157c`, `u32 0`; GLOBALADDR_REPLY = `{1, 0x1a6, ip[16]}`; CERT_INIT news block
  type 8. (WindowsLoader's FREE_TICKET_REPLY constant 0x130 is wrong, the real value is 0x12A.)
  WindowsLoader itself runs NESiCA games with its closed-source core (`EmulatorType: WindowsLoader`),
  not WindowsLoader. Debug knob: `WAL_NESICA_NESYS_EXTRA=0x105,0x10c:01000000` sends extra notices
  after the certificate. message-mode pipe
  `\\.\pipe\nesys_games`, requests `{u32 cmd, u32 len, data}` back to back (next = +8+len),
  replies built with the `Out` helper following the C struct layouts (sizes documented next to
  each reply). Card data: `card_<id>_<type>.bin` in the D: data folder. `WAL_NESICA_NESYS=0`
  disables it; network values via `WAL_NESICA_IP/_MASK/_GATEWAY/_DNS`. AH2 does not use it.
* Not ported (yet): `GetIfEntry` hook, NESYS HTTP access (Groove Coaster), game-specific
  patches from `NesicaGeneric.cpp`. Games also try the NESiCAxLive launcher pipe
  (`\\.\pipe\NxLPipe...`); nobody emulates it and the games tested do not need it. Game detection by CRC is not needed: the profile says which game it is.

### 6.3 Type X (`crates/payload-typex`, `crates/loader`)

Type X / X2 games (and NESiCA games built on Type X I/O like 3D Cosplay Mahjong) talk to a
JVS I/O board on `COM2` and import no driver DLL, so the payload is injected by
**`wal-loader.exe`** (`wal-loader <payload> <game.exe> [args]`): the game is created
suspended, its entry point (from the PEB image base) is replaced by a jump to a stub allocated
in the game that calls `LoadLibraryW(payload)` (address taken locally: Wine maps kernel32 at
the same address), restores the 5 entry bytes and jumps back. No remote thread. The launcher
uses it for systems whose `System::loader()` is set (run dir gets `wal-loader.exe`).

JVS board (`jvs.rs`): port of WindowsLoader's `JvsPackageEmulator` with its
`TaitoTypeXGeneric` settings: Taito stick mode (features `01 02 10 00 02 02 00 ...`: 2 players,
16 switches, 2 coin slots), JVS version 0x30, identifier
`SEGA CORPORATION;I/O BD JVS;837-14572;Ver1.00;2005/10`, WindowsLoader's report-byte quirks.
Switches: start 0x80, service 0x40, up/down/left/right 0x20/0x10/0x08/0x04, btn1 0x02, btn2 0x01,
second byte btn3-6 0x80..0x10, system byte test 0x80; coins counted on release, `30`/`31`
decrease/increase. Native names for `native_map`: `start service test coin up down left right
btn1..btn6`.

## 7. Adding a system

1. Payload crate `crates/payload-<system>` (cdylib, depends on `wal-protocol`,
   `wal-payload-common`, `windows-sys`). In `DllMain`: `wal_payload_common::start("<system>")`,
   then the system's init. Implement the native API, read sticks with `input(player)`, map with
   `ButtonMap` (defaults in code, `native_map` in profiles for per-game differences).
2. Choose the injection:
   * the game statically imports a hardware/driver DLL → replace it (best: loads before the game
     code, no injector, works the same under Wine). Copy its export names **and ordinals**.
   * otherwise proxy a system DLL the game imports (dinput8, winmm, version...) and forward the
     real exports, with the matching `WINEDLLOVERRIDES=<dll>=n,b` set by the launcher.
   * a remote-thread/entry-point injector is the last resort (WindowsLoader documents it as
     unreliable under Wine).
3. Linux side: `crates/launcher/src/systems/<system>.rs` implementing `System` (name, payload
   files and the DLL they replace, always-hidden files) and register it in `systems::by_name`.
4. Add the crate to the workspace members (not `default-members`) and to `build.sh`.
5. Add `systemprofiles/<system>/<game>.yaml` templates.

Hooks for other APIs: `iat::hook` on the game module. Wine-native alternatives are often
simpler than hooks (registry values written at load, files in the run dir).

## 8. Debugging

* `arcade-launcher <profile> --dry-run`: full env and command, nothing changed on disk.
* `arcade-launcher show <profile>`: merged profile and the layers used.
* `arcade-launcher input-test [profile]`: live virtual sticks, device detection messages.
* Payload log: `<prefix>/drive_c/wal/<system>/<game>/wal-<system>.log` (hooks installed,
  launcher connection, FastIO registers seen and their changes, P1 input changes).
* Wine: `WINEDEBUG=+loaddll,+seh,+process,err+all,warn+module arcade-launcher <profile>`.
  `warn+module` shows missing host libraries (e.g. `ELFCLASS64` = a 64-bit lib picked for a
  32-bit `i386-unix` module → missing lib32 package or use `wow64`).
* Imports/exports of game files and payloads: `winedump -j import|export <file>`.

### Testing a payload without the launcher

`tools/fake_launcher.py` replaces the launcher on the port and plays a scripted P1 sequence
(coins, start, directions, buttons):

```sh
S=$(mktemp -d)
python3 tools/fake_launcher.py &                       # listens on 33700
dist/arcade-launcher systemprofiles/nesica/arcana-heart-2.yaml --dry-run 2>/dev/null \
  | grep -v '^cd ' | sed 's/^/export "/; s/$/"/' > $S/env.sh
(cd wine-prefix/common/drive_c/wal/nesica/arcana-heart-2 && . $S/env.sh && \
  timeout 40 "$OLDPWD/wine-runners/GE-Proton11-7-x86_64/bin/wine" 'C:\wal\nesica\arcana-heart-2\game.exe')
(. $S/env.sh && wine-runners/GE-Proton11-7-x86_64/bin/wineserver -k)
grep -E 'input:|0x4120|0x4140' wine-prefix/common/drive_c/wal/nesica/arcana-heart-2/wal-nesica.log
```

(Run the launcher once normally before, so the prefix and run dir exist.)

## 9. Status and TODO

Verified (GE-Proton11-7, WoW64 mode):

* Arcana Heart 2 (builtin ddraw): boots to attract mode, payload loaded via iDmacDrv32
  replacement, registry and D: redirection active, FastIO input verified end to end with
  `tools/fake_launcher.py` (coin edge, start, directions, b1 → btn1, b5 → btn6).
* KOF XIII Climax (d3d9 through DXVK): crypto content keys, card reader, full NESYS boot
  sequence (service version, global address, adapter, local network, income mode, card select,
  config upload, rankings, event data), FastIO input verified by the user, movies play with the
  `dshow` find-filter shim, movie audio (verified by the user). The intro had no sound for ~20 s
  because this dump's `opening.wmv` is badly interleaved (first audio packet 26 MB into the file;
  Wine's splitter and WM ASF Reader read in file order, VLC reads ahead): remuxed once with
  `ffmpeg -c copy`, original kept as `opening.wmv.orig`. Card play to verify.

Games status (scripted test `--input-script tools/scripts/coin-start-mash.txt` + screenshots):

| Profile | System | Result | Needed |
|---|---|---|---|
| nesica/arcana-heart-2 | nesica | in game (user) | - |
| nesica/arcana-heart-3-lmss | nesica | in game (user, GAME_START) | D: WindowsLoader folder |
| nesica/kof-xiii-climax | nesica | in game, movies | key file 303002.key, crypto plaintext reply, dshow find-filter, xact, remuxed opening.wmv |
| nesica/akai-katana-shin | nesica | in game (GAME_START) | `tricks: [d3dx9_37]` (Wine fails its .cfx effects, crash) |
| nesica/blazblue-central-fiction | nesica | in game, NESiCA online | key bbcf, D: WindowsLoader, shop hours patch |
| typex/battle-fantasia | typex | in fight | wal-loader, JVS, 1280x800, WindowsLoader patches, runner hotfix (winedmo) |
| typex/3d-cosplay-mahjong | typex | in game (mahjong hand) | wal-loader, JVS, 1280x800, `tricks: [d3dx9_33]` |
| nesica/aquapazza | nesica | template only, game not available | - |

Launcher test helper: `--input-script FILE` replays `<seconds> p<N> <inputs>` lines (OR-ed with
real devices); NESYS `GAME_START` (0x04) in the payload log means a credit started.

TODO, roughly by priority:

* Live play test with keyboard and SDL gamepads; wheel `devices` mapping on real hardware.
* Graphics modes `dgvoodoo` and `d7vk` untested; DXVK path untested with this game.
* Lightgun backend: port `batocera-wine-guns` (evdev `ID_INPUT_GUN` guns, mouse fallback); needs
  gun axes in the virtual stick or a separate gun state in the protocol (protocol version bump).
* Outputs: payload → launcher (lamps, force feedback, recoil) with system-defined ids.
* NESiCA: verify card insertion/NESYS card data in a real play session; per-game patches.
* Settings the batocera script has and the launcher does not: fonts, redistributable installs,
  save-file linking, virtual desktop, FSR, NVAPI, hidraw.
* More systems (Type X JVS, ...), 64-bit payloads (`x86_64-pc-windows-gnu`).
