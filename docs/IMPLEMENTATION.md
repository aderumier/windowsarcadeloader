# Implementation guide

Developer documentation of the Windows arcade loader: how the code is organised, how the
pieces talk to each other, the non-obvious decisions, and how to extend and debug it.
`DESIGN.md` keeps the original research notes.

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
    src/keys.rs            built-in crypto service keys
    src/news.png           NESYS news picture
    build.rs               passes the .def to the linker
systemprofiles/<system>/<game>.yaml   game templates (shipped)
userprofiles/<system>/<gameid>.yaml   user overrides (optional, git-ignored)
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
* Wine runner: the default runner is `GE-Proton11-7-x86_64` (`wine-runners/`); **all game
  tests must run on it**. When rebuilding a wine DLL (e.g. `quartz.dll` hotfixes), build from
  the GE-Proton source tree (local clone `~/code/protonge`, GE patch set applied) — never from
  plain upstream wine: the GE video stack (`quartz` + `winedmo`/`msdmo`, GStreamer-backed) only
  works as a matched set of runner DLLs, and a plain-wine build is not a drop-in replacement
  even for the same sources.
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
3. `<root>/systemprofiles/<system>/<gameid>.yaml` (template)
4. `<root>/userprofiles/<system>/<gameid>.yaml` (user overrides, optional)

Games are launched from their **dump**: `arcade-launcher <dump dir>` (or the file itself). The
dump root holds `<gameid>.windowsloader`, a text file whose first non-comment line is the
executable path relative to the dump root (`game.exe`, `game\Game.exe`; `/` or `\`). The game id
(file name) selects the profile: `systemprofiles/*/<gameid>.yaml`, unique across systems (the
Type X2 builds of games also on NESiCA are `-typex2`, Type X builds `-typex`). Profiles carry no executable path, so the
same profiles work wherever the dumps are. The executable's folder depth below the dump root
replaces the former `exe_depth` (the whole dump root is mirrored into the run directory).
`<root>` is `--root`, else the current directory, else next to the launcher binary. `show` and
`input-test` also take a bare game id. The merged value is deserialized with
`deny_unknown_fields`, so typos fail loudly; `system` is checked first for a clear error.

Adding a profile key: add it to `defaults.yaml` (with a comment) **and** to `Profile` /
`InputConfig`; every key needs a default in the YAML since the struct has no serde defaults.

Mapping tables can disable an inherited entry with the value `none`.

`exe_fixed_base: true`: the run directory gets a copy of the executable without DYNAMIC_BASE, so
Wine maps it at its preferred base (rebuilt executables whose absolute addresses the
relocations miss). `initial_files`: files installed in the dump only when missing (default
settings, calibration the game then updates).

`gamescope.enabled: true` runs the game command in gamescope (`gamescope -w <width> -h
<height> <args> -- wine ...`, default args `-S stretch -f`): an X server of the game's own
screen size, scaled to the real screen. For screen modes the desktop does not have (Wine on
the desktop's Xwayland only offers its modes): Gundam Spirits of Zeon's 2 player mode starts
only on a 1280x480 screen. The stop path is unchanged (`wineserver -k`, gamescope exits with
its client).

ReShade setups of the dumps (`reshade`, `reshade_files`): some dumps rotate vertical games to a
landscape screen with a bezel through ReShade (and what it chains to: d3d8to9, dgVoodoo). The
profile lists those files in `reshade_files`; with `reshade: true` (default) they are used when
the dump has them (DXVK's `n` override already loads the game directory's DLL first; in
wined3d mode these DLLs get `n,b`), `reshade: false` hides them (portrait picture). ReShade
compiles its effects with `d3dcompiler_47`: Wine's (vkd3d) fails the CRT/bezel shaders, so
those profiles add `tricks: [d3dcompiler_47]`.

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

`dxvk_from` (profile): runner whose DXVK is linked when the game's runner ships none (plain
wine builds, e.g. `wine-9.22-amd64` with `dxvk_from: GE-Proton11-7-x86_64`).

Prefix and tricks: all the games share one prefix, `wine-prefix/full`, with every game's
winetricks verbs preinstalled (`prefix_tricks` in `defaults.yaml`; a unit test checks it holds
every system profile's `tricks`). Each profile still lists the verbs it needs in `tricks`
(installed when missing: the record of each game's needs). Verbs are applied once with the
runner's wine (`winetricks -q <verb>`, `WINEARCH` removed; winetricks from the runner's `bin/`,
else the runners directory, else the PATH), recorded in `<prefix>/.wal-tricks`.
`arcade-launcher prepare <dump|id>` creates the prefix and installs them without running a game.
Their DLL overrides (native dsound, DirectMusic, xact, d3dx9...) apply to every game: a game
needing wine's own DLL sets `dll_overrides: {dsound: b}` (WINEDLLOVERRIDES of that game). A
game that cannot share the prefix sets its own `prefix` (and `prefix_tricks`).

Runner hotfixes: `tools/runner-hotfixes.py [runner...]` patches known bugs of runner builds in
place (exact byte match, `<file>.orig` backup, idempotent), until the fix ships in a release:
* `winedmo-wow64-demuxer-destroy` (GE-Proton11-7 `winedmo.so`): the WoW64 thunk of
  `demuxer_destroy` used the create params layout, so 32-bit games destroyed a garbage demuxer
  handle; Battle Fantasia aborted with `free(): invalid size` when a coin stops the attract
  movie. Upstream Wine 2110c64d89cc; proposed for GE-Proton as a `wine-hotfixes/pending` backport.
* `quartz-find-filter` (GE-Proton11-7 `quartz.dll`, i386): rebuilt DLL, not a byte patch.
  `FindFilterByName("WMVideo Decoder DMO")` unconditionally returned the async file source
  ("Reader"), a leftover of the gstreamer-reader era: games that look the decoder up by name
  (KOF XII/XIII) got the reader, `FindPin("out0")` failed (reader pins are "Raw Video N") and
  the game's Sample Grabber was autoplug-connected to the reader's raw pin — bypassing the
  decoder, whose transform then died (`DMO_E_TYPE_NOT_SET`): black intro video with audio
  (KOF XII), crash on `GetConnectedMediaType` (KOF XIII, see `WAL_DSHOW_FIND_FILTER`). GE-Proton
  #823 looks the exact name up first and only falls back to "Reader" when no filter has it;
  the runner's `quartz.dll` (`lib/wine/i386-windows/` and the common prefix `syswow64/`) was
  replaced with a build containing that fix (GE-patched tree sources — see the runner rule in §3, not plain wine), original
  kept as `quartz.dll.orig`. Verified: KOF XII intro plays — the game connects
  `WMVideo Decoder DMO:out0 -> sample_grabber` itself and no `0x80040203` occurs.
* `quartz-vmr7-rgb24-getavailable` (same rebuilt `quartz.dll`, on top of the previous fix;
  previous build kept as `quartz.dll.pre-rgb24`): GE-Proton `patches/ge-video-rework/0073`
  + `0074`. (1) The VMR7 refused RGB24 input: its default presenter only allocates surfaces
  at the desktop depth, so `Grabber -> Video Renderer` failed with `E_FAIL`; the VMR now falls
  back to an RGB32 surface and expands the pixels. (2) The filter graph's
  `IMediaSeeking::GetAvailable` was a stub leaving its outputs untouched; games seeking to
  "earliest" seeked to stack garbage and the movie ended at once. Gouketsuji Ichizoku's opening
  needed both (see `docs/GOUKETSUJI-INTRO-VIDEO-DEBUG.md`). Build: the configured GE tree copy
  `/tmp/opencode/wine-11.0` (i386 only), `make dlls/quartz/i386-windows/quartz.dll`, then copy
  to the runner's `lib/wine/i386-windows/` (read-only: `chmod u+w` first) and the common
  prefix's `syswow64/`.
* `ddraw-overlay-emulation` (GE-Proton11-7 `ddraw.dll`, i386, rebuilt the same way: `make
  dlls/ddraw/i386-windows/ddraw.dll`; original kept as `ddraw.dll.pre-overlay`; the prefix's
  `syswow64/ddraw.dll` is a symlink to the runner's): GE-Proton
  `patches/ge-video-rework/0075`. Wine never drew DirectDraw overlays nor reported
  `DDCAPS_OVERLAY`; CRI Sofdec players (KOF Maximum Impact Regulation A) only play movies
  through a YUY2 overlay. The visible overlay is now drawn over its destination when that is
  presented. Needs wined3d on the game window (`dxvk: false` for a D3D8/9 game that also uses
  the overlay). See `docs/KOF-MIRA-INTRO-VIDEO-DEBUG.md`.
* Games shipping Wine's own DLLs (e.g. a dump with Wine 10.16's `d3d8/ddraw/wined3d.dll`):
  Wine loads those builtin-format DLLs **from the game dir** even with `=b` overrides, mixing
  Wine versions. `hide:` them in the profile (check with `WINEDEBUG=+loaddll`).

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
* `files` (profile): `<name>: <source>` files copied into the run directory next to the
  executable in place of the game's own, like the payloads (SFZ3 gets the `config.ini` its
  dump lacks, from `systemprofiles/nesica/files/`).
* Executable below the dump root (The Rumble Fish 2: `.windowsloader` = `game\Game.exe`, loading
  `..\data`). The whole root is mirrored: folders on the way
  to the executable are real directories, everything else is linked, and the game starts in
  the executable's folder.

### 5.5 Run loop (`main.rs`)

`run()`: load profile → system → `Wine::new` → resolve exe → check payload files → prefix +
graphics → run dir → payload env → print summary → (dry run stops here) → TCP server →
input hub → spawn wine (in gamescope when enabled) → loop { poll input 4 ms, broadcast on change / 100 ms, exit combo or
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
  (`WAL_NESICA_DDRIVE`, `WAL_TYPEX_DDRIVE`). The kernel32 file imports of the C runtimes
  loaded with the game (`msvcrt`, `msvcr70`-`msvcr120`, `ucrtbase`) are hooked too: games
  doing `fopen("D:/...")` go through the CRT (Chaos Code crashed on `fseek(NULL)`, Ikaruga
  showed a storage error).
* `patches`: `WAL_PATCHES=<rva>:<hex>,...` game code patches from the profile (keeps game
  knowledge in profiles, e.g. known per-game patches). `WAL_PATCHES_EXE=<file>` limits
  them to one executable: test menus (`TestMode.exe`) load the payload too.
* `crash`: vectored exception handler logging the first access violations: address as
  `module+offset`, registers, EBP frame chain, stack scan, and for write overruns the text
  being written (found Cosplay Mahjong's overflow of a D3DX error message).
* `screenshot` (d3d9 and d3d8 shims): `WAL_SCREENSHOT=<s>` writes the back buffer every s seconds
  (`shot-NNNN.bmp` in the run dir, for automated tests); `WAL_D3D9_FULLSCREEN_SIZE=WxH`
  creates/resets fullscreen devices with that size (Type X games ask 1280x768, which Wine does
  not emulate: 1280x800 + Wine's fullscreen scaling; many NESiCA Taito/Type X2 ports do the
  same). Logs the parameters of failing CreateDevice calls. Direct3D 8 games
  (`d3d8!Direct3DCreate8`) get the same shims with the d3d8 vtable slots and layouts
  (CreateImageSurface + CopyRects for the capture). `WAL_D3D9_FULLSCREEN=1` creates windowed
  devices fullscreen (Chaos Code's white window border); it broke SFZ3, which then stops
  presenting after its Reset.
* Games loading d3d9/d3d8 at run time (DxLib's Direct3D 9Ex in Crimzon Clover, Magical Beat)
  go through the game's `GetProcAddress`, also hooked: `Direct3DCreate9`, `Direct3DCreate9Ex`
  (`CreateDeviceEx`, `PresentEx`, `ResetEx` wrapped too) and `Direct3DCreate8`.
* Forced fullscreen on a Direct3D 9Ex device passes a `D3DDISPLAYMODEEX` built from the
  present parameters (`CreateDeviceEx`/`ResetEx` require one when not windowed).
* `font`: `gdi32!CreateFontA` hook. `WAL_FONT_CODEPAGE=932` decodes Shift-JIS face names,
  `WAL_FONT_REPLACE=from=to,...` renames faces (wine only knows the Japanese family name of a
  font under a Japanese locale), `WAL_FONT_SCALE=<f>` scales the heights (the CJK font wine
  falls back to can be much larger than the original: Crimzon Clover 0.28). Logs each font.
* `sdl`: `WAL_SDL_FULLSCREEN=1` adds `SDL_FULLSCREEN` to the game's `SDL_SetVideoMode` calls
  (SDL 1.2, Exception).
* `WAL_D3D9_QUERY_FIX=1`: `IDirect3DQuery9::GetData` goes through a scratch buffer and
  copies back only the requested size. KOF '98 UMFE / 2002 UM poll an event query with
  `GetData(&byte, 1, FLUSH)` on the first byte of their stack frame; DXVK writes the whole
  4-byte BOOL (Windows drivers honour the size), overwriting the saved EBP: the next call used
  EBP 0 (`QueryPerformanceCounter(0xffffffc8)`) and the game crashed after its first frame.
* `codepage`: `WAL_ANSI_CODEPAGE=<cp>` makes the game's own `MultiByteToWideChar` /
  `WideCharToMultiByte` calls on CP_ACP use `<cp>` (932): Raiden IV converts its Shift-JIS
  movie name ("raiden4_ｃ50.m1v") itself.
* `dinput`: `WAL_DINPUT_DISABLE=1` gives the game a fake DirectInput (`DirectInputCreateA/W/Ex`,
  `DirectInput8Create` IAT hooks): no devices enumerated, created devices succeed with zeroed
  states and no buffered data. Games that still read the keyboard/joysticks next to their I/O
  board (Raiden IV Type X opened its test menu on the PC keyboard's `2`, also P2 start: the
  launcher does not grab the keyboard) only see the board. A fake rather than a failed creation,
  which some games treat as fatal.
* `window`: `WAL_WINDOW_SIZE=WxH` (or `screen`: the primary monitor's size) forces the size of the game's top-level windows
  (`SetWindowPos`/`MoveWindow` IAT hooks, at 0,0); in window mode Direct3D stretches the back
  buffer to it (Crimzon Clover's DxLib computed a 5-million-pixel high window: X BadAlloc).
  `WAL_WINDOW_POPUP=1` creates the game's top-level windows as borderless popups
  (`CreateWindowExA/W` IAT hooks: `WS_POPUP`, caption/frame/system menu removed). Shikigami no
  Shiro III makes its fullscreen Direct3D 8 device on a plain overlapped window (style 0, so a
  caption and border): the window manager showed an empty frame with the desktop behind.
  Always on: `ShowWindow` minimize requests of DXVK's d3d9 (also behind its d3d8) and wined3d
  are dropped. They minimize a fullscreen window when it is deactivated: a game started while
  another fullscreen window (a terminal) kept the focus was minimized at once, out of the
  taskbar, and stopped presenting.
* `jvs`: JVS packet framing (`E0` sync, `D0` escaping, size, checksum) for emulated I/O
  boards on serial ports (unit tested; output identical to ttx_monitor).
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
* **FastIO** (`fastio.rs`): `iDmacDrvRegisterRead` answers
  per register; inputs are built on demand from the sticks into the FastIO byte layout:

  | register | value |
  |---|---|
  | 0x4120 | bytes 0..3: P1/P2 (byte0 start/service/test/ext, byte1 dirs, byte2 btn1-4, byte3 btn5-6) |
  | 0x41A0 | bytes 10..13: P3/P4, same layout |
  | 0x4128 | byte8 = P1 `lx`, byte9 = P1 `accel` (0..255) |
  | 0x4140 | coin: 1 once per press (edge), the game acks by writing 0x4140 |
  | 0x400, 0x4000, 0x4004, 0x4124, 0x41A4, 0x4150 | constants |

  Options (profile `env`), for games reading the I/O through Taito's `FIO.dll` (Dariusburst):
  * `WAL_FASTIO_COIN=counter`: 0x4140/0x4144/0x41C0/0x41C4 are coin counters (low 14 bits),
    the game consumes coins by writing `-n`. With one-shot reads FIO never credited a coin.
    Every player's coin feeds 0x4140 (Dariusburst: one credit pool, only 0x4140 consumed).
  * `WAL_FASTIO_BOARDS=2`: 0x4004 reports a second board (players 3/4), `0x00FF00FF`
    ("2nd Fast IO"); Dariusburst shows an I/O error without it.

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
  `WindowsLoader` entry is a symlink, so data lands in the game directory (saves).
  Legacy data folders of existing dumps are moved into `WindowsLoader` by the launcher at
  start (`rundir::migrate_data_dir`, merged without overwriting). The hook table is a macro: index, name, path arguments, other arguments.
* **Crypto server** (`crypto.rs`): byte-mode pipe
  `\\.\pipe\TtxAppCtyptPipe`. Frames `FF FD <u32 size> <PUBLICKEYBLOB>` (game RSA key) and
  `FF FE <u32 size> <SIMPLEBLOB>` (content key encrypted for the service key); `FF FF` escapes
  `FF` in the size. The service key (PRIVATEKEYBLOB, 308 bytes) is `WAL_NESICA_KEY`: a built-in
  name from `keys.rs` (built in: magicalbeat, bbcf, persona4arena, bbcp,
  kofxiiiclimax, persona4ultimix, usf4, darius) or a file in the game directory (KOF XIII Climax
  ships `303002.key`, identical to the built-in one). Replies `<u32 len><blob>` (`<u32 0>` on
  error): SIMPLEBLOB for the game key, or for KOF XIII Climax (`WAL_NESICA_CRYPT_REPLY=plaintext`)
  a PLAINTEXTKEYBLOB RSA-encrypted with the game key. Uses CryptoAPI (Wine rsaenh).
  Wine fix: BBCF imports the reply with `CryptImportKey(SIMPLEBLOB, hPubKey = 0)`, which Windows
  resolves to the container's AT_KEYEXCHANGE key but Wine rejects (NTE_BAD_PUBLIC_KEY, then the
  game asserts "Decrypt failed" when a match starts).
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
* **Card reader** (`rfid.rs`): Taito RFID board on `COM2`
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
  building the run dir, the DLL creates it too and writes the built-in `news.png` (NESYS news
  picture, referenced by the CERT_INIT reply) when missing.
* **NESYS** (`nesys.rs`, corrected with
  [FakeNesicaService](https://github.com/ArcadeMachinist/FakeNesicaService) whose layouts come
  from captures of the real service). **The connect sequence must be `CONNECT_REPLY` →
  `NWRECOVER_NOTICE` (0x103, network up) → `CERT_INIT_NOTICE`**: without the 0x103 notice
  KOF XIII Climax stays on ネットワークを初期化しています ("initializing
  network"). Real-capture formats used: LOCALNW_INFO_NOTICE interface block = MAC, IP, gateway,
  DNS, `u32 0x157c`, `u32 0`; GLOBALADDR_REPLY = `{1, 0x1a6, ip[16]}`; CERT_INIT news block
  type 8. (FREE_TICKET_REPLY is 0x12A, not 0x130.)
  Debug knob: `WAL_NESICA_NESYS_EXTRA=0x105,0x10c:01000000` sends extra notices
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

JVS board (`jvs.rs`) with `TaitoTypeXGeneric` settings: Taito stick mode (features `01 02 10 00 02 02 00 ...`: 2 players,
16 switches, 2 coin slots), JVS version 0x30, identifier
`SEGA CORPORATION;I/O BD JVS;837-14572;Ver1.00;2005/10`, the usual report-byte quirks.
`WAL_TYPEX_JVS_LAYOUT=haunted-museum`: the gun cabinets' switch wiring for `20 01 03` (1 player
x 3 bytes): P1 start on up `0x20`, P2 start on down `0x10`; third byte service `0x08`, P2/P1
action `0x40`/`0x80` active low (the generic reply read up/down as both starts). Coins stay on
the coin counter. `WAL_TYPEX_JVS_ANALOG=v0,v1,...` (hex) sets the analog channels (default 0):
the Haunted Museum games read their volume knob on channel 0, silent at 0 (`C000` in their
profiles).
A bus reset (`F0`) makes the board unaddressed again (sense line, `GetCommModemStatus`): K-On!
resets the bus once more after its first polls and only assigns the address when the sense line
says so (JVS_BOARD_NONE, error 0300, otherwise).
Switches: start 0x80, service 0x40, up/down/left/right 0x20/0x10/0x08/0x04, btn1 0x02, btn2 0x01,
second byte btn3-6 0x80..0x10, system byte test 0x80; coins counted on release, `30`/`31`
decrease/increase. Native names for `native_map`: `start service test coin up down left right
btn1..btn6`.
Other `20` layouts (Gaia Attack 4 asks `20 01 03`: 1 player x 3 bytes) get the requested size,
as a generic reply; `67 xx` (unknown, polled by Gaia Attack 4) is acknowledged so
the commands after it are answered (otherwise I/O ERROR).

Lightgun games (`guns.rs`, `WAL_TYPEX_GUNS=gaia-attack-4|music-gungun-2|haunted-museum|haunted-museum-2|block-king-ball-shooter`):
per-game input code. The gun board's port (`WAL_TYPEX_GUN_PORT`, comma separated, default
`COM1`) is a silent serial device (`serial::install_sink`: writes accepted, nothing read; a
`!COMn` entry is absent instead, its open fails as on a PC without it), and a
thread writes every 16 ms each player's trigger, offscreen flag and position (0..=16384) at the
game's RVAs (layout checked against the dumps). Gun = the
player's virtual stick: `lx`/`ly`, trigger `b1`, offscreen = `b2` or the position at a screen
edge. Profiles set `input.guns_enabled: true` (mice act as guns without lightguns).

Write the guns where the game reads them from, not into state the game rebuilds: Haunted
Museum copies its gun board's 20-byte record (`+0x327958` received, `+0x327944` current) into its
gun state every frame, so the guns are written in both records (writing the state alone, as
some loaders do, lost the race: trigger ignored, positions flickering with board junk). A
silent COM3 also fed it junk: `COM1,!COM3`.

Block King Ball Shooter is a touch screen hit by balls (up to 4 players in co-op, the game
cannot tell them apart: any touch fires its cannons): its `lsdrv.dll` sensor driver is answered
as present (`WAL_TYPEX_LSDRV`, `touch.rs`: `OpenDriver` failed, "touch sensor error
(0xffffffff)"), the guns share the touch (a position and a one-frame flag written when a gun
fires; positions 0..65535), the game's own position writes patched out, its DirectInput faked (it reads the PC mouse too:
one click made two touches). Its JVS switches
(`WAL_TYPEX_JVS_LAYOUT=block-king`, found with its switch test): service 0x40, left/right
start 0x20/0x10, cannon 0x08, then SELECT 0x08 / ENTER 0x04 (buttons 4/3).

Gundam Spirits of Zeon (`gundam-spirits-of-zeon`, its "patched I/O" build) reads its guns
from the JVS data in memory: positions in 640x480 pixels, buttons as bits of its switch bytes
(`bits` per gun: trigger, action, start, service, test), coin counters incremented directly.
Its 2 player mode (`gundam-spirits-of-zeon-2p`: second `.windowsloader` file in the same dump)
patches its player count to 2 and draws both screens side by side on one 1280x480 picture
(player 2's X offset 640); it needs a 1280x480 screen: run in gamescope.

Its intro video was cut on the right: the CRI movie texture is created at the video's size
when the device allows non-power-of-two textures (DXVK, wined3d), but drawn as if rounded
up (640 -> 1024). `WAL_D3D9_POW2=1` (`screenshot.rs`) reports power-of-two textures only
(`D3DPTEXTURECAPS_POW2 | NONPOW2CONDITIONAL`, as the GPUs of the time): a 1024x512 texture.
`WAL_D3D9_TRACE=1` logs the caps, textures from 256 pixels and viewports (how it was found).
The d3d9 shims hook `Direct3DCreate9` in the game's DLLs too (its device comes from
`libIGGfx.dll`).

Work area fix (`payload-common/workarea.rs`, always on when it is wrong): Wine on some
Xwayland desktops reports a work area ~5.4 million pixels high. The game executable and the
DLLs of its directory get the screen size from `GetSystemMetrics`, `SystemParametersInfo`
(`SPI_GETWORKAREA`) and `GetMonitorInfo`, garbage or default `CreateWindowEx` sizes are
replaced, and the window options (`WAL_WINDOW_POPUP`/`_SIZE`) apply to the windows those DLLs
create and resize (Gundam's Alchemy engine, `libIG*.dll`).

`WAL_TYPEX_GUN_PLAYERS=2,1` chooses the virtual player of each gun (calibrating gun 2 with a
single mouse).

Reference for new gun games: [DemulShooter](https://github.com/argonlefou/DemulShooter)
(`DemulShooter/Games/Game_<System><Game>.cs`, e.g. `Game_TtxBlockKingBallShooter.cs`,
`Game_TtxHauntedMuseum.cs`, `Game_TtxHauntedMuseum2.cs`, `Game_TtxGundam.cs` /
`_V2`, `Game_TtxGaiaAttack4.cs`, `Game_TtxGungun2.cs`; Global VR, Lindbergh, RingWide... too).
Each module is an external process doing what `guns.rs` does from inside: per game executable,
the RVAs of the gun axes / trigger / offscreen flags it writes (`_AxisX_Offset`...), the game
instructions it NOPs so the game stops overwriting them (`NopStruct(rva, length)`), code caves
for outputs (recoil, damage, lamps: `SetOutputValue(...)` reads). Port those values into a
`WAL_TYPEX_GUNS` game entry; check the RVAs against our dump (DemulShooter targets a given
build: compare the bytes at the NOP addresses before trusting them). Its axis ranges are the
game's native ones (convert from our 0..=16384 / virtual stick range).

Common payload options used by Type X2 games: `WAL_PIN_CWD=1` (`drive.rs`): the game's
`SetCurrentDirectory` stays in its own directory (`.\sh`, `.\data\sh` -> those folders), for Type X2 games (Gaia Attack 4 steps up with `..\`: SOUND ERROR).
`WAL_VFW_CODECS=vidc.wmv3=WMV9VCM.dll` (`vfw.rs`): registers Video for Windows codecs in
`HKLM\...\Drivers32` at startup, the DLL being in the game directory (Gaia Attack 4's WMV9 AVIs,
codec extracted from the dump's `wmv9VCMsetup.exe`).

### 6.4 Batocera integration

A configgen generator (`windows-arcade-loader`, kept in the rgs Batocera tree, not in this
repository) runs `arcade-launcher run <rom> --root <emulator dir> --profile <es options>`:

* Emulator directory `/userdata/system/rgs/emulators/windows-arcade-loader`:
  `arcade-launcher`, `payloads/`, `systemprofiles/`, `launcher.yaml` (machine layer:
  `runners_dir: /userdata/system/wine/custom`, `runner: GE-Proton11-7-x86_64`,
  `payloads_dir: payloads`; template in `tools/batocera/launcher.yaml`). The Wine prefixes are
  created there. `tools/batocera/install.sh [DEST]` copies a build (and the profiles) into it.
* The GE-Proton runner (with the runner hotfixes) is copied to
  `/userdata/system/wine/custom/GE-Proton11-7-x86_64`: Batocera's own Wine builds lack them.
* Roms: the dump directory's `<gameid>.windowsloader` file (ES extension `.windowsloader`), or
  the dump as a `.squashfs` image: configgen mounts it with a writable overlay
  (`writesToRom`, kept in `/userdata/saves/<system>/<image>`) and hands the mounted directory.
* ES options (`es_features_windowsarcadeloader.cfg`): rotation (`reshade`), renderer (`dxvk`),
  mouse as gun (`input.guns_mouse`), keyboard (`input.keyboard_enabled`). Only the options set in
  ES are written to the `--profile` layer (merged last), so a game's own value stays otherwise.
* Hotkey exit runs `pkill -TERM -x arcade-launcher` (the launcher stops the game on SIGTERM).

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
   * a remote-thread/entry-point injector is the last resort (known injectors document it as
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

### Regression test (`tools/regression.py`)

Launches each game of `tools/regression/games.yaml` (the working ones, by dump; options `wait`,
`script`, `skip`) one after the other, with `WAL_SCREENSHOT` on, and checks after `wait`
seconds (default 60):

* started: the payload connected to the launcher (its log);
* running: the game has not exited;
* picture: the last Direct3D 8/9 screenshot is not uniform/black (`-` for other APIs);
* audio: the game's PulseAudio/PipeWire stream peaks above silence. Its streams are moved to a
  temporary null sink looped back to the default one (still heard) and that sink is recorded
  (`pw-record`, `stream.capture.sink`), so only the game counts (`parec --monitor-stream`
  reads silence for wine's streams). A coin and start after 30 s
  (`tools/regression/coin-start.txt`, per game `script`) start the game, as attract modes are
  often silent.

Results in `tools/regression/results/<date>/` (`report.md`, `results.json`, launcher and payload
logs, screenshots per game), compared with the previous run: a check that passed and now
fails is a regression (exit status 1). `--only TEXT` runs a subset, `--compare DIR` picks the
reference run. The prefix is prepared first (`arcade-launcher prepare`). It takes the screen
for ~1 minute per game.

### Screenshots (seeing what the game shows without watching it)

The payload's d3d8/d3d9 shim (`payload-common/src/screenshot.rs`) can dump the game's back
buffer periodically. This is how automated/agent test runs check what is on screen:

```sh
RUN=wine-prefix/full/drive_c/wal/typex/gouketsuji-ichizoku-typex2     # the game's run dir
rm -f $RUN/shot-*.bmp
timeout 120 env WAL_SCREENSHOT=5 dist/arcade-launcher run "games/typex2/Gouketsuji Ichizoku - Matsuri Senzo Kuyou"
ls -l --time-style=+%T $RUN/shot-*.bmp                             # mtime = when it was taken
magick $RUN/shot-0007.bmp -alpha off /tmp/shot-0007.png            # BMP -> PNG to view it
magick $RUN/shot-*.bmp -alpha off -resize 320x +append /tmp/strip.png   # contact strip
```

* `WAL_SCREENSHOT=<seconds>` (min 0.5): in the hooked `Present`/`PresentEx`, when the interval
  has elapsed, the current back buffer is copied (`GetRenderTargetData` into a system-memory
  surface; d3d8: `CreateImageSurface` + `CopyRects`) and written as `shot-NNNN.bmp` (1-based
  counter) in the run dir. `WAL_SCREENSHOT_DIR=<windows dir>` writes them elsewhere.
* Combine with `--input-script tools/scripts/coin-start-mash.txt` to get past attract/title,
  and with `WINEDEBUG=...` to correlate a frame with a trace (match the BMP mtime with the
  log; the payload log `wal-<system>.log` in the run dir is timestamped too).
* Cadence is approximate: a shot is only taken on a `Present`, so expect ~8–10 s between
  shots with `WAL_SCREENSHOT=5` on heavy boots, and none while the game does not present
  (loading, hangs).
* It only sees the **game's own d3d8/d3d9 devices** (created via the hooked
  `Direct3DCreate9(Ex)`/`Direct3DCreate8`, import or `GetProcAddress`). Anything drawn by
  another path is invisible: DirectShow's VMR window (quartz uses its own ddraw), ddraw/GDI
  games, a second device created inside a Wine DLL. A black shot during a movie means "the
  game's back buffer is black", not necessarily "nothing on the monitor"; for those cases
  grab the X screen instead: `import -window root /tmp/screen.png` (or
  `ffmpeg -f x11grab -i $DISPLAY -frames:v 1 /tmp/screen.png`).
* The BMP is 32-bit with an unused 4th byte: tools that read it as alpha report misleading
  statistics (ImageMagick shows a ~25 % "mean" on a black frame). Use `-alpha off`, e.g.
  `magick shot.bmp -alpha off -format '%[fx:mean]' info:` (0 = all black).
* The shell's `WAL_SCREENSHOT` reaches the game through the inherited environment, so no
  profile change is needed — but a profile `env:` entry with the same name overrides the shell
  (only `WINEDEBUG` gives the shell priority).

### Testing a payload without the launcher

`tools/fake_launcher.py` replaces the launcher on the port and plays a scripted P1 sequence
(coins, start, directions, buttons):

```sh
S=$(mktemp -d)
python3 tools/fake_launcher.py &                       # listens on 33700
dist/arcade-launcher "games/nesicax/Arcana Heart 2" --dry-run 2>/dev/null \
  | grep -v '^cd ' | sed 's/^/export "/; s/$/"/' > $S/env.sh
(cd wine-prefix/full/drive_c/wal/nesica/arcana-heart-2 && . $S/env.sh && \
  timeout 40 "$OLDPWD/wine-runners/GE-Proton11-7-x86_64/bin/wine" 'C:\wal\nesica\arcana-heart-2\game.exe')
(. $S/env.sh && wine-runners/GE-Proton11-7-x86_64/bin/wineserver -k)
grep -E 'input:|0x4120|0x4140' wine-prefix/full/drive_c/wal/nesica/arcana-heart-2/wal-nesica.log
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

Games status, one row per game id (`<gameid>.windowsloader` in the dump; scripted test
`--input-script tools/scripts/coin-start-mash.txt` + screenshots):

| Game id | Game | System | Result | Needed |
|---|---|---|---|---|
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | not tested | - |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | `tricks: [d3dx9_37]` (Wine fails its .cfx effects, crash) |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | template only, game not available | - |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | - |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | D: data in WindowsLoader |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | key bbcf, shop hours patch |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | not tested | - |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | d3d8 1280x800, DirectMusic tricks (native dsound) |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user) | native dsound (wine dsound caps made DxLib compute a 5-million-pixel window / overrun its mixer), ranking NULL-check patches, `WAL_D3D9_FULLSCREEN` (9Ex display mode), `WAL_FONT_SCALE: 0.28` |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | key bbcp |
| `dariusburst-another-chronicle-ex` | Dariusburst Another Chronicle EX | nesica | in game, 4 players (user: credits; TODO: P1 controls reported not responding with JVS on) | NESiCA I/O despite the typex2 folder; key darius, `WAL_FASTIO_COIN: counter`, `WAL_FASTIO_BOARDS: 2`, init.ini with JVS on (`files:`), 1.16 right-screen un-flip patch (same addresses); 2720x768 back buffer, fine with GE-Proton without gamescope |
| `dark-awake` | Dark Awake: The King Has No Name | nesica | in fight | same as Chaos Breaker (same engine) |
| `do-not-fall` | Do Not Fall: Run for Your Drink | nesica | works (user) | D: data in WindowsLoader |
| `dragon-dance` | Dragon Dance | nesica | works (user), smoke effect glitches | run game.exe (NxL stand-in), native DirectMusic/dsound; d7vk, wined3d Vulkan, DDrawCompat crash |
| `elevator-action` | Elevator Action Death Parade | nesica | in game | 1280x800 |
| `en-eins-perfektewelt` | EN-Eins Perfektewelt | nesica | works (user) | 1280x800, native dsound (nothing on screen with wine's dsound) |
| `exception` | Exception | nesica | works fullscreen (user) | native dsound (no picture with wine's dsound), `WAL_SDL_FULLSCREEN` |
| `gouketsuji-ichizoku` | Gouketsuji Ichizoku: Matsuri Senzo Kuyou | nesica | works (user) | hide dgVoodoo D3D8/D3D9 |
| `homura` | Homura | nesica | works (user) | native dsound (stuck on NOW LOADING with wine's dsound) |
| `hyper-street-fighter-2` | Hyper Street Fighter II: The Anniversary Edition | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `ikaruga` | Ikaruga | nesica | in game (user) | CRT D: redirection (storage error), `fakejapanese` |
| `kof-2002-um` | The King of Fighters 2002 Unlimited Match | nesica | works (user) | `WAL_D3D9_QUERY_FIX` (event query polled into a 1-byte variable: DXVK writes 4 bytes over the saved EBP) |
| `kof-98-umfe` | The King of Fighters '98 Ultimate Match Final Edition | nesica | works (user) | `WAL_D3D9_QUERY_FIX` (event query polled into a 1-byte variable: DXVK writes 4 bytes over the saved EBP) |
| `kof-xiii-climax` | The King of Fighters XIII Climax | nesica | in game, movies | key file 303002.key, crypto plaintext reply, dshow find-filter, xact, remuxed opening.wmv |
| `magical-beat` | Magical Beat | nesica | works (user) | key magicalbeat, init wait patch |
| `nitroplus-blasterz` | Nitroplus Blasterz: Heroines Infinite Duel | nesica | works (user) | - |
| `persona-4-arena` | Persona 4 The Ultimate in Mayonaka Arena | nesica | in fight (user: may crash with some characters) | key persona4arena, plaintext reply, NESYS on, shop hours patch 0x6C9D0 |
| `persona-4-ultimax` | Persona 4 The Ultimax Ultra Suplex Hold | nesica | works (user) | shop hours patch |
| `psychic-force-2012` | Psychic Force 2012 | nesica | works (user) | run game.exe (NxL stand-in), native dsound prefix, patch 1280x768 preset -> 1280x720 |
| `puzzle-bobble` | Puzzle Bobble | nesica | works (user) | - |
| `raiden-3` | Raiden III | nesica | in game (user); intro movie black | hide dinput8; TODO: movies are uncompressed BGR24 240x320 AVIs played through amstream (`IAMMultiMediaStream`, MediaStreamFilter): AVI Decompressor is added but most connections to the media stream are refused (`VFW_E_TYPE_NOT_ACCEPTED`) |
| `raiden-4` | Raiden IV | nesica | in game (user); intro movie: audio only, black video | `tricks: [d3dx9_31]` (MMShader.fx), hide ReShade, `WAL_ANSI_CODEPAGE: 932` (Shift-JIS movie name); TODO: black video: the game's own TEXTURERENDERER gets RGB24 from winedmo's MPEG Video Decoder, but ffmpeg rejects every packet (`Invalid frame dimensions 0x0`). GE builds ffmpeg with `--disable-everything` and no parsers, so the raw .m1v is fed in 1 KiB chunks; an ffmpeg 8.1 rebuild with `--enable-parsers` gave whole pictures (674 errors instead of 16k) but still `0x0`: not the (whole) fix, reverted |
| `rastan-saga` | Rastan Saga | nesica | works (user) | 1280x800, hide ReShade |
| `senko-no-ronde-duo` | Senko no Ronde DUO: Dis-United Order | nesica | works, sound effects (user) | hide XAudio2_6.dll + manifests (wine's xaudio2) |
| `skullgirls-2nd-encore` | Skullgirls 2nd Encore | nesica | works (user) | - |
| `space-invaders` | Space Invaders | nesica | works (user) | - |
| `strania` | Strania: The Stella Machina | nesica | works (user) | - |
| `street-fighter-3-3rd-strike` | Street Fighter III 3rd Strike: Fight for the Future | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `street-fighter-zero-3` | Street Fighter Zero 3 | nesica | works fullscreen (user) | dump lacks config.ini: Vampire Savior's installed with `files` |
| `the-rumble-fish-2` | The Rumble Fish 2 | nesica | works (user) | `.windowsloader`: `game\Game.exe` (loads `..\data`) |
| `tottemo-e-mahjong` | Tottemo E Mahjong | nesica | works (user); test menu (TestMode.exe) crashes, TODO | run game.exe (NxL stand-in), patch 1280x768 -> 1280x800 limited to game2.exe (`WAL_PATCHES_EXE`) |
| `trouble-witches-ac` | Trouble Witches AC: Amalgam no Joutachi | nesica | works (user) | - |
| `ultra-street-fighter-4` | Ultra Street Fighter 4 | nesica | not tested | - |
| `vampire-savior` | Vampire Savior: The Lord of Vampire | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | wal-loader, JVS, 1280x800, `tricks: [d3dx9_33]` |
| `battle-fantasia` | Battle Fantasia | typex | in fight | wal-loader, JVS, 1280x800, game patches, runner hotfix (winedmo) |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | wal-loader, JVS (`block-king` layout), touch sensor driver answered (`WAL_TYPEX_LSDRV`), shared touch for 4 guns (`WAL_TYPEX_GUNS`), patch of its touch position writes, `WAL_DINPUT_DISABLE` (it also read the PC mouse: a second shot at the cursor for each click) |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | wal-loader, JVS, 1280x800, patch 0xECFD0 |
| `chase-hq-2` | Chase H.Q. 2 | typex | BLOCKED: boot MessageBox, exits 0, window off-screen (user sees nothing) | see docs/CHASE-HQ-2-BOOT-DEBUG.md: Wine sees a 5434188x5434103 X desktop (Xwayland), game sizes its window from it; analog JVS also unemulated (not drivable anyway) |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | wal-loader, JVS, native DirectMusic prefix (exits at start with wine's), `reshade_files` dgVoodoo D3D8 + ReShade dxgi, `tricks: [d3dcompiler_47]`, dgVoodoo.conf with Direct3D 11 output (`files`: the dump asks D3D12, NULL device crash) |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | wal-loader, JVS, native DirectMusic prefix, its window mode (`args: [-window]`) in a screen-sized popup (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE: screen`): Wine drew its fullscreen 640x480 unscaled in the top-left corner |
| `gaia-attack-4` | Gaia Attack 4 | typex | works (user: 100%, 4 guns, coins, sound, videos) | wal-loader, JVS, guns in the gun board record (4 x 10 bytes, `COM1,!COM3`; no patch of its per-frame gun update, which copies it), volume knob on analog 0 (its volume write not patched out), `WAL_PIN_CWD`, WMV9 + Indeo 5 codecs (`WAL_VFW_CODECS`; ir50_32.dll copied into the dump), `dxvk: false` (first attract video crashed), popup 1280x720 window |
| `gouketsuji-ichizoku-typex2` | Gouketsuji Ichizoku - Matsuri Senzo Kuyou | typex | works (user: title, demo match, attract); intro movie never plays | wal-loader, JVS (native 640x480, no override); movie blocked: VMR second wined3d GL context fails — see docs/GOUKETSUJI-INTRO-VIDEO-DEBUG.md |
| `kof-98-um-typex` | The King of Fighters '98 Ultimate Match | typex | works (user: perfect) | wal-loader, JVS, `.windowsloader`: `launcher.exe`, `WAL_D3D9_QUERY_FIX`, A/B/C/D map, hide MS dinput8 |
| `kof-sky-stage` | The King of Fighters Sky Stage | typex | works (user), rotated by the dump's ReShade | wal-loader, JVS, hide MS dinput8, `reshade_files` ReShade d3d9, `tricks: [d3dcompiler_47]` |
| `haunted-museum` | Haunted Museum | typex | works (user: 100%, guns, service/test, sound) | wal-loader, JVS (`haunted-museum` layout, volume knob on analog 0), guns in the gun board record (`WAL_TYPEX_GUNS`, `COM1,!COM3`), `WAL_PIN_CWD`, MUSEUM.ini with WindowsLoader paths (`files`), window created 1286x5434821 at CW_USEDEFAULT: popup 1280x720 at 0,0 (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE`) |
| `gundam-spirits-of-zeon` | Mobile Suit Gundam: Spirits of Zeon | typex | works (user: guns, inputs, coins; intro video fixed with `WAL_D3D9_POW2`) | wal-loader, JVS, guns in its JVS data (`WAL_TYPEX_GUNS`), known patches (`WAL_PATCHES`), popup 640x480 window, work area fix, power-of-two texture caps |
| `gundam-spirits-of-zeon-2p` | Mobile Suit Gundam: Spirits of Zeon (2 players) | typex | works (user: in gamescope, guns) | as `gundam-spirits-of-zeon`, 2 player patch, 1280x480 in `gamescope` |
| `haunted-museum-2` | Haunted Museum II | typex | works (user: guns, inputs, sound, video) | wal-loader, JVS (`haunted-museum` layout, volume knob on analog 0), guns in the gun board record (`COM1,!COM3`), `exe_fixed_base` (rebuilt exe: absolute addresses not relocated, imports without lookup table), known patch restoring its JVSENABLE/GUNENABLE reads (hex-edited to run without boards), both guns pre-calibrated (`initial_files` HM20/HM21.CFG + CRC32), `dxvk: false` (Indeo videos: RADV GPU fault with DXVK), popup 1280x720 window |
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | BLOCKED: error 0002 DISPENSER_ERROR (card dispenser) | wal-loader, JVS (re-init after bus reset), window mode in a screen-sized popup, `WAL_ANSI_CODEPAGE: 932` (d3dx9 DrawTextA); TODO: dispenser (reference "Skip Boot Check") |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works, intro movie (user); intermittent crash at the movie end | wal-loader, JVS, `dxvk: false`, hide the dump's Wine DLLs, patch 0x447C, runner ddraw overlay emulation (docs/KOF-MIRA-INTRO-VIDEO-DEBUG.md) |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | wal-loader, JVS, 1280x800, A/B/C/D button map, runner quartz fix (#823) |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | not tested | - |
| `music-gungun-2` | Music GunGun! 2 | typex | BLOCKED: "Direct3D device enumeration failed" message box | wal-loader, JVS, guns (`WAL_TYPEX_GUNS`), `WAL_PIN_CWD`, patch 0x137C70 |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | wal-loader, JVS; Notice screen ~65 s |
| `raiden-4-typex` | Raiden IV | typex | works (user) | wal-loader, JVS, `tricks: [d3dx9_31]` (MMShader.fx), `WAL_DINPUT_DISABLE` (keyboard 2 opened the test menu) |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | wal-loader, JVS, `WAL_WINDOW_POPUP` (overlapped window: empty frame), `reshade_files` d3d8to9 + ReShade d3d9, `tricks: [d3dcompiler_47]` |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | wal-loader, JVS, native 1280x720, hide xinput1_3 + XAudio2_4 and its manifests (wine's xaudio2), as the NESiCA build |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | wal-loader, JVS |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | wal-loader, JVS, OpenGL, `WAL_WINDOW_POPUP` (overlapped window: empty frame), save folder patch, picture height 448 -> 480 (white bars) |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | wal-loader, JVS, native 1920x1080 (no back buffer override), hide MS dinput8 |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user-confirmed): `-launchGame`, coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | wal-loader, `TsuInput` + `TsuMotion` (idle motion seat) COM objects emulated in the payload, Wine DirectInput with host joysticks hidden, dump's `tsunet.dll` registered in-proc + adapter-walk patched, d7vk; TODO: cabinet env (`C:\Tsunami\`, `launch.reg`) reproducible from the profile — see docs/REVOLT-DEBUG.md |

Wine's builtin DirectSound breaks several games in ways that do not look like sound bugs
(Crimzon Clover's window/mixer sizes, Dragon Dance's crash, Homura stuck loading, Exception and
EN-Eins showing nothing, Psychic Force never opening its I/O): they need native dsound and
DirectMusic (winetricks, their `tricks`). The shared prefix has them for every game; a game
broken by them gets wine's back with `dll_overrides`.

NxL launcher stand-ins: Psychic Force 2012, Tottemo E Mahjong and Dragon Dance ship a small
`game.exe` that creates the NESiCAxLive launcher events/shared memory/pipe (`NxLEvent_*`,
`NxLMMF_*`, `\\.\pipe\NxLPipe*`) and starts the real game (`game2.exe launcher`,
`game_liong.exe -s`): the profiles run that stub, the child loads our iDmacDrv32.dll from the
run directory.

Gamepads: Back inserts a coin, L3/R3 are the service and test switches (keyboard `F1` / `F2`).

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
