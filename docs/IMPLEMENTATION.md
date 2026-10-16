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
    src/crypto.rs          Taito crypto service (NESiCA, Type X)
    src/crypto_keys.rs     built-in crypto service keys
  loader/                  wal-loader.exe: starts a game with a payload loaded before its entry point
  payload-typex/           Taito Type X payload (JVS I/O board on COM2), loaded by wal-loader
  payload-rawthrills/      Raw Thrills PC payload (JAMMA USB board reports), loaded by wal-loader
  payload-ringedge/        Sega RingEdge payload (amLib devices, keychip/master PCP services), loaded by wal-loader
  payload-nesica/          NESiCAxLive payload = replacement iDmacDrv32.dll
    iDmacDrv32.def         export ordinals of the original driver (see §6.2)
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
| 4 Relaunch | game → launcher | UTF-8 arguments, may be empty |

Relaunch: when the game exits, the launcher starts it again (same run directory, input server
kept) with the profile's arguments plus these (split on spaces), `WAL_RELAUNCH=1` in its
environment (its payload appends to the log). The latest request wins; Esc still stops. The
Swarm: one run of the game per stage (payload `swarm_campaign.rs`).

The launcher sends the full state on every change plus every 100 ms, so a reconnecting payload
is up to date immediately; unknown kinds are skipped (forward compatible). Bump `VERSION` on an
incompatible change. Output ids (`protocol::output`): force feedback states for `player` (0..3),
sent on change (and refreshed every second by the payload): `FFB_CONSTANT` (1,
-10000 pushed left ..= 10000 pushed right), `FFB_SPRING` (2, centering, 0..=10000),
`FFB_VIBRATION` (3, 0..=10000). Lamps and recoil are TODO.

### 4.3 Environment contract (launcher → payload)

| variable | meaning |
|---|---|
| `WAL_PORT` | TCP port to connect to |
| `WAL_LOG` | Windows path of the payload log file |
| `WAL_MAP` | `virtual=native,...` override of the payload's default mapping (from profile `native_map`; `native+native` presses several) |
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

`payload_install: [<path>, ...]`: where the system's payload is installed, paths from the dump
root, in place of its default name (Samurai Shodown loads `NxL2Core.dll` at run time from its
Unreal plugin folder `Onion/Plugins/NxL2CorePlugin/`).

`exe_fixed_base: true`: the run directory gets a copy of the executable without DYNAMIC_BASE, so
Wine maps it at its preferred base (rebuilt executables whose absolute addresses the
relocations miss). `initial_files`: files installed in the dump only when missing (default
settings, calibration the game then updates).

`drives: {f: F, g: G}` maps cabinet drive letters to persistent directories inside the dump
root, creating them when missing. C: and Z: are reserved. Mario Kart Arcade GP DX uses these
two separate volumes for redundant Test Mode saves (`F:\\TestMode`, `G:\\TestMode`),
bookkeeping and other cabinet data. Without the mappings its temporary save files fail to
open, so operator settings revert on every launch. The game creates timestamped settings
files itself; saves do not require a cabinet shutdown.

Mario Kart's 32-bit payload identifies v1.10.22 and v1.18.16 by their PE timestamp and
image size before applying dongle patches; each patch also checks its original bytes.
The v1.18.16 dongle persistence helper uses `ret 4`, unlike v1.10.22's `ret`.
Both versions have guarded offline Content Router bypasses. v1.18.16 also skips
the router subsystem's error-registrar calls, including E05-02 when a LAN interface
discovers a non-cabinet router; unrelated device errors still use the normal handler.
The offline ShareK IPC
adapter exposes the 61-slot dual COM interface from `ipccl.dll` (including the
process-reboot getter v1.18.16 calls during startup); it does not connect to ShareK.

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
  * Calibrated wheels (`wheels.rs`, `userprofiles/wheels.yaml`, machine-wide by SDL joystick
    name, written by the GUI's "Wheel calibration" wizard): opened raw (even when SDL maps them
    as a gamepad, as Batocera's `SDL_GAMECONTROLLERCONFIG` does), their raw events turned into
    gamepad ones and driven by `input.wheel` over `input.gamepad` (the wheel's mapping defaults to
    the gamepad's; the GUI's Wheel tab shows that merged table and saves its changes in
    `input.wheel`, "none" removing a gamepad entry): steering -> `leftx` (calibrated full left / right
    = -32768 / 32767), gas -> `r2`, brake -> `l2`, clutch -> `+righty` (released 0, pressed
    32767: the gamepad triggers' range, the reference of the games' test menu calibrations), each
    end with a 3 % band giving exactly its value; wheel buttons and hat directions -> the gamepad
    buttons chosen. A game's `input.devices` entry for the device wins (raw map, no calibration).
    The GUI reads them as gamepads in its Wheel tab captures (their own SDL gamepad events
    dropped); the Gamepad tab takes only the other gamepads.
  * Initial joystick axes are read at open time (pedals rest at non-zero values).
  * `keyboard_ramp_ms` (default 0): a key driving an axis (sticks, pedals) reaches full
    deflection in that time, as an analog stick or pedal pushed progressively (`Hub::frame`:
    each player's keyboards merged, then `mapping::ramp` per axis); released, the axis is back at
    once, reversed it starts from the center. Super Bikes: 1000 (lean and throttle).
  * Player slots: `devices.<name>.player`, else the lowest slot not used by another SDL device.
    Keyboards are fixed per `input.keyboard.pN` and share one global key state.
  * evdev keyboards: one reader thread per device that has KEY_A and KEY_ENTER, no grab (the
    game still receives keys through X11), autorepeat ignored; events reach the main thread
    over an mpsc channel.
  * `exit_combo` (physical gamepad names) sets `exit_requested`.
  * Force feedback (`ffb.rs`, `input.ffb`: `enabled`, `gain` %, `invert`): the server passes the
    game's outputs to the main loop (mpsc), `Hub::output` plays them on the SDL devices of that
    player. A device whose evdev node (`SDL_GetJoystickPathForID`) supports FF_CONSTANT is a
    wheel: effects through evdev, as linuxloader's evdevFfb.c (constant force at direction
    0x4000, positive levels pulling left; FF_SPRING centering, else the wheel's autocenter; a
    sine for the vibration), each uploaded once then updated in place, its autocenter off (the
    game centers it), FF_GAIN set (else the levels scaled). Gamepads rumble through SDL (strong
    motor: vibration, weak motor: the constant force, 1.5 s, refreshed by the game). Not SDL's
    haptic API for wheels.

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
  `system.reg` is missing; updated when the newest mtime of the runner's `share/wine/wine.inf`
  and builtin PE DLLs (`lib/wine/{i386,x86_64}-windows`) differs from
  `<prefix>/.wal-update-timestamp` (symlinks in system32/syswow64 are removed first). Wine
  loads the prefix's copies of the builtin DLLs: a replaced runner DLL (hotfix, rebuild) only
  reaches the games through that update, which copies it in.
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
A new prefix is unpacked from `<prefix>.tar.gz` (`wine-prefix/full.tar.gz`) when present:
`tools/prefix-archive.sh` builds it (`prepare` into a fresh prefix, packed), so a machine's
first start downloads no winetricks verbs (Batocera: `install.sh` ships it). It is unpacked
aside then renamed (no partial prefix), without its runner stamp: the runner update rewrites
what pointed into the building machine's runner (font paths, vkd3d/icu links). Verbs added
to `prefix_tricks` after it was built are installed as usual (its `.wal-tricks`). Without an archive the
prefix is created from scratch. The archive is versioned: `prefix-archive.sh` writes
`<prefix>.version` (a build timestamp) next to it, the unpacked prefix keeps it in
`.wal-prefix-version`. A prefix of another version is replaced by the archive's at the next
start, the old prefix's `drive_c/users` (the games' saves) and `drive_c/wal` (run directories)
moved into it: a new archive reaches a machine that has a prefix already (Batocera), without
winetricks downloads there.
Verbs whose defaults would affect the other games of the shared prefix get registry settings
right after their install (`TRICK_SETTINGS` in `wine.rs`, none at the moment: ffdshow, tried for
Aliens Extermination's DivX movies, registers DirectShow decoders of high merit for every
game and asked each one whether to use it; the game's movie error boxes are patched out
instead).
The verbs' native DLLs (DirectMusic, xact, d3dx9, d3dcompiler...) serve every game, as the
old common prefix did: many games rely on one they do not list (Chaos Code's d3dx9_37). Native
dsound is the exception, native only for the games listing it (wine's builtin for the others,
in their WINEDLLOVERRIDES): it crashes the CRI audio games (Gaia Attack 4, Haunted Museum,
Street Fighter IV...). Native DirectPlay (`directplay`, Chase H.Q. 2) is kept the same way,
for the games listing it only. `physx` (PhysX system software, legacy 2.x runtimes) serves the
PhysX 2.8.1 games whose `PhysXLoader.dll` looks the runtime up in the registry (Elevator Action
Death Parade: no PhysX scene without it, crash at the first stage). `d3dx9_42`: Million Arthur
Arcana Blood imports it. `dll_overrides` (`<dll>: n | b | ...`) has the last word. The overrides
are written `*<dll>`: wine looks up `*dsound` before `dsound`, in the environment then the
registry, and winetricks writes `*dsound` keys, which a plain `dsound=b` would not beat. A
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
* `ge-master-video-dlls` (2026-10-09, supersedes the two `quartz` entries above): the video
  DLLs `winedmo` (PE and `winedmo.so`), `quartz`, `qasf` and `wmvcore`, i386 and x86_64, rebuilt
  from GE-Proton master (fork `aderumier/proton-ge-custom`, branch `windowsloader` = master plus
  the patches not merged yet). Master's patch 0080 stops the DirectShow parsers flushing on
  Run -> Pause: a paused renderer never got its first frame and Music GunGun! 2 hung on a
  black attract movie (`GetState(INFINITE)`). 0073 (VMR7 RGB24) is dropped on that branch: it
  no longer applied after 0067 and Gouketsuji's opening plays without it. Build: a worktree of
  the GE `wine` submodule next to links to the GE `patches`, `wine-staging`, `wineopenxr`; the
  wine section of `patches/protonprep-valve-staging.sh`; `make_vulkan -x vk.xml -X video.xml`
  (Vulkan-Headers registry) and `tools/make_specfiles`; `configure --enable-archs=i386,x86_64`
  with `FFMPEG_CFLAGS` = the GE `ffmpeg` submodule (62.x, the runner's) + its generated
  `libavutil/avconfig.h`, `FFMPEG_LIBS` = the runner's `lib/x86_64-linux-gnu` (`-rpath-link`).
  PE and unix sides must come from the same tree (the unixlib interface changed since 11-7).
  Installed in the runner (previous files `*.pre-ge-master`) **and** in the prefix
  (`syswow64/`, `system32/`): Wine loads the prefix's copies, a runner-only change is ignored.
* `winedmo-seek-flush` (2026-10-11, `winedmo.dll` i386 and x86_64, same tree, previous files
  `*.pre-seek-flush`): GE's DirectShow parser (`dlls/winedmo/winedmo_quartz_parser.c`)
  `dmo_seeking_SetPositions` took each output pin's `flushing_cs` before flushing downstream;
  its stream thread holds that lock during `Receive`, and a renderer may block in `Receive`
  until it is flushed: a seek deadlocked. SNK Heroines (its own renderer, looping movies)
  hung right after START. The downstream BeginFlush now comes first (as Windows and Wine's
  other parsers). Found with the hang watchdog (`crash.rs`, §6.1).
* Backups of replaced runner files go to `<runner>/hotfix-backups/<dll dir>/`, never next to
  the runner's DLLs: the prefix update (`wineboot -u`) copies everything in
  `lib/wine/*-windows/` into `system32`/`syswow64` and registers it, and the old
  `qasf.dll.pre-ge-master` became the registered DMO Wrapper Filter and WM ASF Reader
  (`{94297043-...}`, `{187463A0-...}`, 32 and 64 bits): DirectShow movies mixed the old qasf
  with the new winedmo. To repair a prefix: delete the `*.dll.<suffix>` copies in
  `system32`/`syswow64`, then `wine regsvr32 qasf.dll` and
  `wine C:\windows\syswow64\regsvr32.exe qasf.dll` (or let the next prefix update do it).
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
* `serial`: emulated serial devices (IAT hooks of CreateFile/ReadFile/WriteFile/comm calls,
  a fake handle and reply queue per device, one device per port: each `install` adds one);
  used by the NESiCA card reader, the Type X JVS board and the medal board. Calls passed
  through to Windows keep their last error untouched (no environment lookup or logging after
  them changes it): Unreal's overlapped `ReadFile` returns FALSE with `ERROR_IO_PENDING`, and a
  lazy `WAL_TRACE_FILES` lookup in the hook turned it into 203 (Samurai Shodown: "ReadFile
  Failed! Error code = cb"). Overlapped calls
  complete at once (`OVERLAPPED` result set, event signaled), `ClearCommError` reports the
  queued reply bytes. Logs the first 40 packets (`WAL_SERIAL_TRACE=<n>`: the first n). Any other COM port the
  game opens is absent (open fails, logged), whatever ports the machine has: Wine maps the machine's own
  (Batocera: COM1 = /dev/ttyS0), where nothing answers (D1GP's card reader probe: card error at boot).
* `drive`: `D:\` redirection (moved from NESiCA), folder variable chosen by the system
  (`WAL_NESICA_DDRIVE`, `WAL_TYPEX_DDRIVE`). The kernel32 file imports of the C runtimes
  loaded with the game (`msvcrt`, `msvcr70`-`msvcr120`, `ucrtbase`) are hooked too: games
  doing `fopen("D:/...")` go through the CRT (Chaos Code crashed on `fseek(NULL)`, Ikaruga
  showed a storage error). `winmm!mmioOpenA/W` paths are redirected too (Battle Gear 4 opens its
  music WAVs through them: sound effects but no music otherwise).
  `WAL_GAME_DRIVE=<letter>`: the game runs from the root of that drive, as on the cabinet:
  paths on it go to the game directory, `GetCurrentDirectoryA/W` answer `<letter>:\`,
  `GetLogicalDrives` and `GetDriveTypeA/W` report C:, the redirected drive and it as fixed
  disks (game executable's imports). Battle Gear 4 (`E`) registers the current directory's drive
  and every fixed drive in its file library and loads `\data\...` from there: under Wine it
  polled `C:\data\2D\Effects\sepia.*` forever at boot (white window, "not responding").
  Paths the game passes to Wine itself are redirected too: `winmm!mmioOpenA/W` (Battle Gear 4's
  music) and a DirectShow filter graph's `RenderFile` / `AddSourceFilter` (`CoCreateInstance` of
  CLSID_FilterGraph as IGraphBuilder / IFilterGraph2: vtable methods 13 / 14 patched once): Battle
  Gear 4 (2005) plays `E:/data/Scene/demo/attract/attract_demo.wmv` that way, not found otherwise,
  then crashed ~20 s after boot stopping its never created movie (NULL `IMediaControl`).
* `patches`: `WAL_PATCHES=<rva>:<hex>,...` game code patches from the profile (keeps game
  knowledge in profiles, e.g. known per-game patches). `WAL_PATCHES_EXE=<file>` limits
  them to one executable: test menus (`TestMode.exe`) load the payload too.
* `crash`: hang watchdog (`heartbeat()`, called by a per-frame game API: the NESiCA 2
  `NxL2Update`): when it stops for 5 s every thread of the game is suspended in turn and its
  stack logged (`hang:` lines; on x64 the exact call stack from the unwind tables, else a
  stack scan). A frozen game ("not responding", no crash) shows what it waits on.
* `memdiff`: `WAL_MEMDIFF=<module>,...` compares those modules' loaded code sections with their
  files 30 s and 90 s after start and logs each changed range (`memdiff: module+rva file ->
  memory`): what another loader's DLL patches in memory (none for the packed NxL2Core.dll
  wrapper tried with Samurai Shodown).
* `crash`: vectored exception handler logging the first access violations and MSVC C++
  exceptions (thrown type, `std::exception` message): address as
  `module+offset`, registers, EBP frame chain, stack scan, and for write overruns the text
  being written (found Cosplay Mahjong's overflow of a D3DX error message).
* `screenshot` (d3d9 and d3d8 shims): `WAL_SCREENSHOT=<s>` writes the back buffer every s seconds
  (`shot-NNNN.bmp` in the run dir, for automated tests; 32-bit and 16-bit back buffers);
  `WAL_MEMDUMP=<rva>-<rva>` also writes that range of the executable's memory with each shot
  (`mem-NNNN.bin`: finding the variables behind what a frame shows); `WAL_D3D9_FULLSCREEN_SIZE=WxH`
  creates/resets fullscreen devices with that size (Type X games ask 1280x768, which Wine does
  not emulate: 1280x800 + Wine's fullscreen scaling; many NESiCA Taito/Type X2 ports do the
  same). Logs the parameters of failing CreateDevice calls. Direct3D 8 games
  (`d3d8!Direct3DCreate8`) get the same shims with the d3d8 vtable slots and layouts
  (CreateImageSurface + CopyRects for the capture). `WAL_D3D9_FULLSCREEN=1` creates windowed
  devices fullscreen (Chaos Code's white window border); it broke SFZ3, which then stops
  presenting after its Reset. `WAL_D3D9_WINDOWED_SIZE=WxH` creates/resets every device windowed
  with that back buffer size and drops the `Present` source/destination rectangles, so window
  mode stretches the whole back buffer to the window: games drawing a smaller picture in the
  corner of their back buffer, of a size no fullscreen mode has (Yatagarasu: 854x480 of
  1280x720, presented to 0,0 1280x720). `WAL_D3D9_ASPECT=W:H` presents windowed devices to the
  largest centered rectangle of that aspect in the window (black bars): games stretching a 4:3
  picture over a wide back buffer (Elevator Action: 1280x768). `WAL_D3D9_MAX_FPS=N` paces
  `Present` to N frames per second: windowed devices get no vsync under Xwayland (Elevator
  Action asks interval one and ran at ~400 fps, 6 times too fast; DXVK's maxFrameRate had no
  effect). `WAL_D3D9_TRACE` also logs presents per second. Failing Resets are logged.
  Always on: a fullscreen device that cannot be created (Direct3D 8 and 9) is retried windowed
  with the same back buffer, the game window made a borderless window over the whole screen
  (Direct3D stretches the picture to it), and later devices and resets stay windowed. Under
  Xorg Wine only offers the output's real modes: on a TV without 1280x800 the 1280x768 Type X /
  NESiCA games (`WAL_D3D9_FULLSCREEN_SIZE: 1280x800`) failed `CreateDevice` with
  D3DERR_NOTAVAILABLE and quit at boot (Chaos Breaker's log on an AMD Batocera; Battle
  Fantasia and Block King had the same symptom there, working only with a forced odd video mode).
  Direct3D 8 windowed devices need a `FullScreen_PresentationInterval` of 0: the retry clears
  it (SuperCars asked 1360x768 with interval one, D3DERR_INVALIDCALL windowed otherwise).
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
  `WAL_ANSI_CODEPAGE_DLLS=<name>,...` applies those hooks and `GetACP` to named game DLLs
  as they load (up to 60 seconds), for Source engine's text conversion in Cyber Diver.
* `wcase`: `WAL_FAST_WCASE=1` makes the game's own `_wcsicmp`, `_wcsnicmp`, `towlower`,
  `towupper` (C runtime imports) fold ASCII themselves (others: the original functions). Wine's
  `towlower` maps each character with `LCMapStringW`: Fighting EX Layer stopped ~5 s twice after
  each match (GNOME "not responding").
* `dinput`: `WAL_DINPUT_DISABLE=1` gives the game a fake DirectInput (`DirectInputCreateA/W/Ex`,
  `DirectInput8Create` IAT hooks): no devices enumerated, created devices succeed with zeroed
  states and no buffered data. Games that still read the keyboard/joysticks next to their I/O
  board (Raiden IV Type X opened its test menu on the PC keyboard's `2`, also P2 start: the
  launcher does not grab the keyboard) only see the board. A fake rather than a failed creation,
  which some games treat as fatal. `WAL_KEYBOARD_STATE_DISABLE=1`: the game's
  `user32!GetKeyboardState` reports no key pressed (Wacky Races read START / VIEW on Enter /
  Right Shift that way). `WAL_DINPUT_WHEEL=<product name>`: the same fake DirectInput (A or W,
  by the interface asked for) lists one DirectInput 8 driving wheel under that name: X, Y, Z
  absolute axes taking the game's `DIPROP_RANGE`, X player 1's `lx` (`WAL_DINPUT_WHEEL_AXIS`,
  `-` inverts), Y / Z released, no buttons. D1GP Arcade's cabinet wheel is a USB "Immersion
  TouchSense Steering Wheel": it steers with its X axis, loads `<product name>.txt` (spaces
  as `_`, shipped in the dump) and merges the JVS switches only in its per-device loop.
* `window`: `WAL_WINDOW_SIZE=WxH` (or `screen`: the primary monitor's size) forces the size of the game's top-level windows
  (`SetWindowPos`/`MoveWindow` IAT hooks, at 0,0); in window mode Direct3D stretches the back
  buffer to it (Crimzon Clover's DxLib computed a 5-million-pixel high window: X BadAlloc).
  `WAL_WINDOW_POPUP=1` creates the game's top-level windows as borderless popups
  (`CreateWindowExA/W` IAT hooks: `WS_POPUP`, caption/frame/system menu removed). Shikigami no
  Shiro III makes its fullscreen Direct3D 8 device on a plain overlapped window (style 0, so a
  caption and border): the window manager showed an empty frame with the desktop behind.
  `WAL_HIDE_CURSOR=1`: `LoadCursorA/W` return no cursor and `SetCursor` sets none (Battle
  Gear 4's window class showed the arrow over the game).
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

Battle Fantasia and Spica Adventure's NESiCA releases retain JVS on `COM2` rather than
FastIO. Their profiles live under `systemprofiles/nesica/` but select `system: typex` to
load `wal_typex.dll` through wal-loader. Separate ids `battle-fantasia-nesica` and
`spica-adventure-nesica` keep them distinct from the existing Type X profiles and saves.
Both are user-confirmed working 100%, with picture and audio verified during testing.
Battle Fantasia (`game.exe` CRC32 `0xed6d3c26`) uses the existing A/B/C/D/G button layout
and `WAL_D3D9_FULLSCREEN_SIZE=1280x800`; the older executable's code patches are not
applied. Spica Adventure (`0x739bde93`) uses the standard JVS buttons and needs no patches.

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
* **Crypto server** (`payload-common/src/crypto.rs`, also served by the Type X payload when a
  profile sets `WAL_NESICA_KEY`: KOF XIII Climax's Type X2 build): byte-mode pipe
  `\\.\pipe\TtxAppCtyptPipe`. Frames `FF FD <u32 size> <PUBLICKEYBLOB>` (game RSA key) and
  `FF FE <u32 size> <SIMPLEBLOB>` (content key encrypted for the service key); `FF FF` escapes
  `FF` in the size. The service key (PRIVATEKEYBLOB, 308 bytes) is `WAL_NESICA_KEY`: a built-in
  name from `crypto_keys.rs` (built in: magicalbeat, bbcf, persona4arena, bbcp,
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

### 6.2b NESiCAxLive 2 (system `nesica2`, `crates/payload-nesica/src/nxl2.rs`)

NESiCAxLive 2 games (Million Arthur Arcana Blood) import `NxL2Core.dll` instead of the FastIO
driver. The original talks to the cabinet's NESiCAxLive 2 system program (shared memory
`NxLMMF_<guid>`, a launcher mapping `E408D848-...` holding the game settings as an `N2CD` blob of
typed name/value entries, `NxL2Init` fails without it and the game quits), to `iDmacDrv32.dll`
and to the card reader on a COM port. The NESiCA payload exports the 33 `NxL2*` functions (by
name, cdecl; `iDmacDrv32.def`) and the `nesica2` system installs it as `NxL2Core.dll`
(`iDmacDrv32.dll` is then not loaded):
* `NxL2GetInput`: player 1's FastIO switches rearranged as the original does (one player per
  cabinet: the API has a single player; versus is a link between cabinets): service 0x1, start
  0x2, test 0x4, up/down/left/right 0x8-0x40, buttons 1-6 0x80-0x1000. `native_map` applies.
* Coins: the FastIO coin counter, `NxL2DecreaseCoinCount` consumes it; coin and I/O status 0 (OK).
* Settings by name (`NxL2Get/SetGameSetting*`, `NxL2GetSystemSetting*`): `int Get(name, int
  *found)` returns the value (0 when unknown) and writes 1 / 0 to `found` when not NULL (seen in
  the original DLL). It used to be read as a `default` argument: the games pass a pointer (or
  NULL), returned as the value of every unknown setting (Samurai Shodown's
  `IsOnlineMatchEnabled`, `CPUTime`, `WinLimit`... got a pointer value). Values: built-in
  defaults (volumes 100, 1 coin 1 credit...), the profile's `WAL_NXL2_SETTINGS=Name=value,...`,
  then `nxl2-settings.txt` of the D: data folder (written by `NxL2UpdateGameSetting`, test menu
  changes).
* `NxL2GetAutoEndRemainderTime` -1 (0 made the game quit at boot), `NxL2CheckStoreOpenTime` -1
  (no closing announcement), `NxL2GetGameID` the registry `GameKind`.
* Card: `NxL2IsRFIDRead` is true while the `card` virtual input's card is inserted,
  `NxL2GetRFIDID` its 16-character id (`WAL_NESICA_CARD_ID`). `WAL_NESICA_CARD_AUTO=1` inserts it
  when the game starts a card read (`NxL2RequestRFID`; a gamepad has no card input).
* NESYS card requests of Fighting EX Layer, after the card is read: reissue test (0x10) and
  reissue (0x11), answered 0x117 / 0x118 (the replies have no force takeover entry: the request
  number + 0x107 from 0xF on) with a card status (status 1, the client parses 0x111-0x119
  alike; without a reply the game waits on its card screen until its countdown). Then card
  selects by XML (`<root><info>` with card_id, tenpo, mac_addr, game_id), the card server's
  request types: 259 card (`/root/card`: card_id, player_name, score_i1, achieve_status,
  fcol1-3; empty = a new card, NO-NAME), 264 binary data (`/root/card_bdata`), 401 session
  (`/root/session`: card_id, mac_addr, session_id, expires, player_id, answered by the payload;
  empty = "このNESiCAはご利用いただけません", card refused). `WAL_NESYS_HTTP_DUMP=1` also logs
  the card select parameters and card updates.
* Card progress (Fighting EX Layer): the game writes type 771 (`<data>` player_name, score_i1,
  achieve_status, fcol1-3) and 776 (`<data>` bdata: base64 of an encrypted blob, "ZACY" + size,
  fixed key: 77 bytes differ between two saves; bdata_size) after each match. A write is its
  read + 512: its `<data>` is saved as the read's answer (`/root/card`, `/root/card_bdata`, the
  card id added) in `card_<id>_<read type>.bin`. The card screen's wins/losses/rank count
  versus matches between cabinets (not changed by CPU matches).
* The player's card (`crates/launcher/src/nesica_card.rs`, `userprofiles/nesica-card.yaml`,
  the GUI's "NESiCA card" window: card id, "New card" = `7020392` + 9 random digits, player
  name): the launcher sets `WAL_NESICA_CARD_ID` / `WAL_NESICA_PLAYER_NAME` for the nesica and
  nesica2 systems, before the profile's `env` (which wins). The saves are by card id.
* `WAL_NESICA_PLAYER_NAME`: the `<player_name>` of a saved card's answer (a new card is
  registered as NO-NAME; there is no name entry, the card server's web site set it). The game
  then writes the card back with that name.
* Every function logs its first call (`nxl2: <name> called`).
* 64-bit games (The King of Fighters XIV: Direct3D 11, XAudio2.8) get the payload's x86_64
  build, `wal_nesica64.dll` (`System::payloads64`, chosen when the executable is PE32+; same
  source, `build.sh` builds both).
* `WAL_NESICA_NESYS_HOST`: the server list of the NESYS certificate (`CERT_INIT_NOTICE`,
  `key=value,...`, default `card_id=...`). KOF XIV also reads `ms` and `ss` from it and crashed
  on the missing keys (NULL string, `game.exe+0x41d87`) right after the certificate.
* HTTP access through the service (`HTTPACCESS_GET/POST_REQUEST` 0x1A/0x1B, `http_access` in
  `nesys.rs`): request `{url[1024], proxy[1024], auth 32+32, 6 x u32 (port at +0x844, body size
  at +0x854), header[1025], body}`, answered `HTTPACCESS_START` {1} then `HTTPACCESS_REPLY`
  `{header[1025] aligned 4, u32 status 200, error 0, sequence, size, body}`. Million Arthur posts
  card-service reads to the certificate's `ms` server (`card_id=...&command=256&data=<xml>&
  type=8456` / `9220`) and parses the body from its third byte (`1\n` then the XML) with MSXML:
  `/root/match_conf` (its `record` children), a `_com_error` E_POINTER abort ("Runtime Error!")
  when missing. The reply is `1\n` + `<root><match_conf></match_conf></root>` (no online
  match). `WAL_NESYS_HTTP_DUMP=1` writes each request to `http-<seq>.bin` in the data folder.
* `WAL_NESICA_NESYS_OFFLINE=1`: the service answers the start and connect requests, then sends
  `NW_ERROR` (0x101, code 0x20000011 of a real capture) instead of the network notice and the
  certificate: the game plays offline. Without any service (`WAL_NESICA_NESYS=0`) Samurai
  Shodown waits forever on "initializing network". Samurai Shodown runs this way (as other
  loaders do): online, it reads its card object storage before each fight (below) and waits.
* `<module>!<rva>:<hex>` entries of `WAL_PATCHES` patch a DLL loaded after the payload (an
  Unreal plugin's library), applied as soon as it is loaded.
* The second u32 after the reply header is what the games' NESYS libraries read as the
  request's error code (Samurai Shodown's `NxL2NESYSLib.dll`: -1 = failed, 0 = parse the body;
  nothing compares an HTTP status with 200). Samurai Shodown's card object storage (multipart
  posts: `object_data`, `bucket_id`, `index`, `type` 442, read before the first fight) is
  answered 404 for now; its expected reply body is not known yet (the game stays on
  "loading..."). Experiments: `WAL_NESYS_OBJECT_REPLY=<status>:<body>` (`\n` escapes) and
  `WAL_NESYS_HTTP_SWAP=1` (status in the second field, object storage only).
* `WAL_NESYS_TRACE=1` (`nesystrace.rs`): logs the game's messages on the NESYS pipe both ways,
  whatever serves it (finding what a game expects; Million Arthur opens the pipe once at boot).

**NESYS online for a new NESiCAxLive 2 game (how KOF XIV and Million Arthur were brought up).**
These games embed their own NESYS client library (`NxLNesys*` classes in KOF XIV) and talk to
the `nesys_games` pipe themselves (not through `NxL2Core.dll`). Expected sequence, all logged by
`nesys:` lines (`WAL_NESYS_TRACE=1` for the raw bytes): `CLIENT_START` 0x1 -> 0x10D;
`CONNECT_REQUEST` 0x2 -> 0x10E, notices 0x103 (network up) and 0x107 (certificate); then the
game asks the service version 0x18, global address 0x15, adapter 0x17, local network 0x14,
income mode 0x20, and (Million Arthur) HTTP posts 0x1B; later 0x4 game start, 0x1D income start.
* Start with NESYS on and `WAL_NESICA_NESYS_HOST: card_id=7020392010281502,ms=127.0.0.1,ss=127.0.0.1`
  (both games need `ms` and `ss`; the NESiCA 1 games only read `card_id`, hence not the default).
* Nothing sent after the 0x107 certificate, stuck on 初期化中です (the green NESiCA icon only
  means certified): the game's session waits for a value the certificate lacks. Million Arthur:
  its CERT_INIT handler (found through the `card_id` string's references) stores `ms` at
  session +0x20, and the session's state machine (step 1 -> 8, its `+0x1c`, each step gated by
  a field set from a notice, then a request) never leaves step 1 while it is NULL. KOF XIV
  crashed on it instead (NULL `ms`, `game.exe+0x41d87`).
* `crash: access violation` right after a NESYS message: look at the faulting code's string
  arguments (keys looked up in the certificate's list, XML paths...).
* "Microsoft Visual C++ Runtime Library: Runtime Error! ... terminate it in an unusual way":
  an uncaught C++ exception; the crash log names it (`crash: C++ exception .?AV_com_error@@` =
  a COM/MSXML smart-pointer call, HRESULT 0x80004003 = a NULL node) with the return addresses
  of the thrower; the XPath strings next to the call give the node the game wants
  (`/root/match_conf`).
* An unanswered request logs `nesys: unhandled command 0x..` and the game waits: the command
  ids (requests 0x1-0x2B, replies and notices 0x101-0x12A) are listed in `nesys.rs` (`cmd`).
* HTTP posts: dump them (`WAL_NESYS_HTTP_DUMP=1`), the form's `type` says what is read; reply
  formats differ per game (Million Arthur skips 2 bytes and wants XML; the Groove Coaster card
  server format is `1\n1,1\n<xml>`). Only the replies the game parses matter: find the parse
  code from the reply handler (0x122: the client copies the body to its session object) and
  its XPath strings.
* NxL2 settings the game reads are logged (`nxl2: setting <name> = ...`): e.g. KOF XIV's
  `Resolution` (0 = 720p, 1 = 1080p).

**Million Arthur white screen (2026-10-11, offline only).** Not seen with NESYS online (user:
training and story modes fine); offline (NESYS off), with DXVK the picture turns white at the
first coin after the title (~25 s) and stays white through character select: only the HUD
(credits, icons, "Please Wait") draws over it. It is not the .pmt movies: a reference build of
the game (a Wine bottle image) goes on to the selection menu on the Batocera box. Already the
first boot dialogs (初期化中です, the offline notice) are washed out to white/pink with our DXVK,
dark red with wined3d (which goes black later). Tried without effect: `Resolution=2`. That
reference differs from us by:
* its DXVK: Batocera's Wine 9.22 (`wine-tkg` core, `dxvk=1`) with Batocera's DXVK, ours the
  GE-Proton runner's (next test: run with that DXVK build);
* its `NxL2Core.dll` replacement, set from `NXL2Core.ini`: `RESOLUTION=2`,
  `IS_GAME_SELECT_ENABLE=1`, `AMIC_HARDWARE_SUPPORT=1`, `CHARA_SELECT_TIME=1`, volumes 100 (our
  payload reports `IsGameSelectEnable=0`);
* ReShade 4.5.4 as the game folder's `d3d9.dll` (a CRT shader preset), between the game and
  DXVK; it logs a 3840x2160 back buffer;
* XAudio2_5 / XACTEngine3_5 registered with regsvr32 before the run.

### 6.3 Type X (`crates/payload-typex`, `crates/loader`)

Cyber Diver starts `hl2.exe` directly with `-ac -io 1 -nesys 0`; `-io 0` selects
dummy controls. Its serial calls live in the late-loaded `bin/xioman.dll`, loaded and
hooked before startup with `WAL_TYPEX_JVS_DLLS`. Its combined input poll ends with
`25 01`: XIO requires a report byte and four coordinate bytes for that command even
without a touch device. Omitting them rejects the whole switch/coin/analog update.
With the corrected reply, credits work. The start screen (`gamescene.dll`) checks
XIO system switch 1: JVS down (`0x100000` in XIO). JVS button 3 is a handle button.
The cabinet's normalized axes use analog channels
1/2 for aim and 3/4, inverted, for movement; all unused channels are centered at
`0x8000`. `WAL_TYPEX_XIO_CALIBRATION=1` supplies a symmetric `1..65535` range for
each invalid equal min/max pair passed to XIO (this dump loads `0..0`, causing full
deflection at rest); valid calibration is preserved. The `cyber-diver` layout inverts
the two active-low handle buttons (JVS buttons 7/8). Gameplay and controls are
user-confirmed working 100%. Code page 932 hooks are enabled in the Source DLLs, but some
dynamic Japanese labels remain garbled. Test launches `testmode.exe`, absent from this dump;
the failed external-process launch hangs the client.

Type X / X2 games (and NESiCA games built on Type X I/O like 3D Cosplay Mahjong) talk to a
JVS I/O board on `COM2` and import no driver DLL, so the payload is injected by
**`wal-loader.exe`** (`wal-loader <payload> <game.exe> [args]`): the game is created
suspended, its entry point (from the PEB image base) is replaced by a jump to a stub allocated
in the game that calls `LoadLibraryW(payload)` (address taken locally: Wine maps kernel32 at
the same address), restores the 5 entry bytes and jumps back. No remote thread. The launcher
uses it for systems whose `System::loader()` is set (run dir gets `wal-loader.exe`): Type X,
and Global VR (the payload under its `USBIOExtreme.dll` name, so Far Cry's import of it gets
the same module; Aliens Extermination imports nothing to replace). The game is created with
`SW_SHOWNORMAL`, as `start` does: without it Wine gives `SW_SHOWDEFAULT` (10), and Aliens
Extermination, which passes its `WinMain` show command as `main`'s argc, crashed in
`glutInit` reading 10 arguments.

JVS board (payload-common `jvs_board.rs`, shared with RingEdge) with `TaitoTypeXGeneric` settings: Taito stick mode (features `01 02 10 00 02 02 00 ...`: 2 players,
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
second byte btn3-8 0x80..0x04, third byte (generic `20 01 03` reply) btn9-16 0x80..0x01,
system byte test 0x80; coins counted on release, `30`/`31` decrease/increase. Native names for
`native_map`: `start service test coin up down left right btn1..btn16`.
Other `20` layouts (Gaia Attack 4 asks `20 01 03`: 1 player x 3 bytes) get the requested size,
as a generic reply; `67 xx` (unknown, polled by Gaia Attack 4) is acknowledged so
the commands after it are answered (otherwise I/O ERROR).

Lightgun games (`guns.rs`, `WAL_TYPEX_GUNS=gaia-attack-4|music-gungun-2|haunted-museum|haunted-museum-2|block-king-ball-shooter|elevator-action-death-parade`):
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

Music GunGun! 2 has the same 20-byte gun record (received `+0x2B8100`, copied to the game's
`+0x2B80E4` only when a gun board packet is parsed): both are written, axes 0..0x3FF0. A
player starts by shooting the screen: the start keys pull that player's trigger.

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

Medal games (New Super Mario Bros. Wii Coin World, Capcom on Type X2 hardware, 4 satellites):
* Backup SRAM board (`sram.rs`, always on when the game imports it): the game reads its
  battery-backed SRAM through `TxedLap.dll` (`TXE001_Open/Close/GetSRAMSize/Read/Write`, C++
  names, cdecl) over the board driver `txedctl.dll`. Without the board `Open` fails, its backup
  thread never starts and the boot stays on バックアップＲＡＭ確認中. The imports are answered
  with a 32 KB file, `WindowsLoader/txe001-sram.bin` (settings, bookkeeping, credits; delete it
  to reset the cabinet). `Read`/`Write(offset, count, buffer, width)`: `count` bytes, or with
  `count` 1 one item of `width` 0/2/4 (1/2/4 bytes).
* JVS on COM2 is only a watchdog: Taito commands `01 01` (status), `05 0B B8` (3000 ms),
  `04` (enable), then `01 01` + `08` (kick) every poll; `08` is answered with one data byte.
* Medal I/O board on COM1 (`medal.rs`, `WAL_TYPEX_MEDAL_PORT`): text frames at 115200 baud.
  Game: `:<date> <time>` (first frame), `/S`, a line per unit `<unit>:<satellite>,<command>,
  <value>,0` (`H` hopper, `C` medal selector, `O` lamps with 3 values, `L`, `M`
  electromechanical counters, commands `R` run / `S` stop / `C` clear error), `/E`, ETX. Board:
  `/S`, a line per unit `<unit>:<n>,<state>,<d1>,<d2>,<error>,<detail>`, the switches of each
  satellite `S:<n>,S,0,0,3,<state>,<released>,<pressed>` (hex words: state active low, then the
  edges since the last frame), `/E`, ETX (the game checks `/S`...`/E` and the ETX). No reply:
  "I/O board communication timeout" (7201) after ~7 s.
  * Selector `C`: d2 = medals accepted since the last frame (the game adds it to the
    satellite's credits and echoes it in its value); reported only while the selector runs.
    A cumulative count over-credited, hit the credit cap and started refund payouts.
  * Hopper `H`: the request value is the number of medals to pay; the board answers state `R`,
    d1 = the requested count, d2 = paid so far, then `S` once paid (the game then subtracts
    the paid medals from its request). Paid one per poll (~15/s, the satellite hopper test
    pays its batches of 100). Other units: the commanded state echoed, zeros.
  * Switches (the game's input test, satellite test mode): bits 3/2/0/1 joystick, 4 START,
    5 BET, 6 payout, 8 medal accepted, 7 fake medal, 11 test/enter, 12 select, 13
    cancel/error reset, 10 satellite key (active high, "adjusting" 98xx when on), 9
    maintenance door (low = closed, 99xx "door open" otherwise), 14 hopper count, 15 hopper
    over-current. Satellite N = player N: `coin` inserts a medal, b1 BET, start START, b2
    payout, b3 select, test (F2) the satellite menu, service (F1) cancel/error reset
    (`native_map` names: up down left right bet start payout medal test select cancel key
    door). The game takes the medals as bets by itself (3 per play).

Driving games (Valve Limit R, Chase H.Q. 2, Battle Gear 4 Tuned, Wacky Races, D1GP Arcade):
* Pedals on JVS analog channels: `WAL_TYPEX_JVS_ANALOG_INPUTS=<axis>,...` gives player 1's
  virtual axis read on each channel (`-axis` inverted, `+axis` its positive half only: 0 at the
  center and below, a clutch on a stick axis; empty: the fixed `WAL_TYPEX_JVS_ANALOG` value),
  left-justified 16 bits. The features then report 8 analog channels of 10 bits: the
  game's JVS library polls `22 n` only for the channels the board declares (`n` = the count).
* Steering board (`wheel.rs`, `WAL_TYPEX_WHEEL_PORT=COM1`): the wheel's motor driver, which
  also reports the wheel position. 2-byte commands at 38400 baud, a 2-byte reply to each:
  `20` reset -> `A0 00`; `1F` motor stop -> `1F 00` (bit 7 clear: already calibrated, the game
  skips its wheel calibration); `11` starts the position reports, then every command (motor
  forces) is answered `0x400 | pos` big endian (10 bits, 0 = full right; full left for Battle
  Gear 4: `WAL_TYPEX_WHEEL_AXIS=-lx`); before that
  `8C A0`. Wheel = player 1's `lx` (`WAL_TYPEX_WHEEL_AXIS`). Forces are not forwarded yet.
* Valve Limit R is a 4-cabinet link game: a race takes the cabinets of our link group, and
  without a network the lookup failed alike for every NETWORK-ID, so the three absent cabinets
  joined ("WAIT! CHALLENGER(S) STILL SELECTING" for minutes). Its profile patches the group
  lookup to return the NETWORK-ID itself (standalone cabinet).
* Battle Gear 4 (`WAL_TYPEX_JVS_LAYOUT=battle-gear` / `battle-gear-pro`, I/O mapping found with
  its I/O test and its input table: label, type, mask): key reader on the main board, Taito
  commands `6F` (read), `6D` (status), `70` (UID, last byte non-zero or the game errors), `6A`,
  `6B` (tag data: a space, the 7-character key id, `W_OK` at 41; id kept in `bg4-key.txt` of the
  data folder, random: 2 letters, `T`, 4 digits); these replies carry their own report byte.
  Professional cabinet (`-pro`): a second board answers as node 2 (the sense line reports "all
  addressed" after its address only). The game runs its wide monitor & clutch mode when it is
  there (no test menu setting: FACTORY SETTING only resets): 1360x768, 6-speed H shifter, clutch
  on analog channel 6, the transmission chosen per race. Its shift up / shift down bits (button
  2, button 3) are inverted and give the H shifter rows (top = shift down, bottom = shift up);
  node 2's `26` byte gives the lanes (left 0x80, right 0x40) and the shifter mechanism state
  (0x20 / 0x10 set: not in its 6-speed / sequential configuration, starts in 6-speed), toggled
  once per start of its `32` output (the mechanism motor: the I/O test's MOTOR TEST, VIEW and
  HAZARD; toggled on every packet it flipped 60 times a second). Node 2 also reports the coin
  counters (no credit otherwise) and answers `66` with a report. Gears 1-6 = player 2's virtual
  b1-b6 (one source maps to one target: a gear needs a lane and a row).
* Force feedback from the game's memory (`ffb.rs`, `WAL_TYPEX_FFB=battle-gear-4`): every 16 ms
  the race flag (+0x4A9508), wall / car contact flags (+0x42EBB2 / +0x42EBB3) and speed (float
  +0x3F3000): in a race a centering spring (`WAL_TYPEX_FFB_SPRING` %), a left hit (wall 0x10,
  car 0x08) pushes right and a right hit (0x20, 0x02) left with speed / 180 of full force, a
  car contact (0x01) vibrates. Addresses from FFBArcadePlugin's BG4JP.cpp, not confirmed in a
  race yet (they stay 0 in the attract demo); `WAL_TYPEX_FFB_TRACE=1` logs them. The steering
  board's own motor commands (COM1) are not used.
  `WAL_TYPEX_FFB=battle-gear-4-original` (Battle Gear 4, 2005): the same effects, race flag
  +0x40F2C0 (its code matches Tuned's), contact flags +0x3CF862 / +0x3CF863 (the player's car
  record +0x3CF860, Tuned's +0x42EBB0: same code using it) and speed +0x3CF97C (that record +
  0x11C, km/h): found in races on Batocera from /proc/pid/mem snapshots (race flag 1 in a race,
  contacts changing on hits, the speed following the throttle).
  `WAL_TYPEX_FFB=chase-hq-2`: the wheel motor command in the game's I/O output word (32 bits at
  [+0x130B558] + 0x45, read with `ReadProcessMemory`: the pointer is not set at boot), lamp bits
  cleared, 15 codes each way (as FFBArcadePlugin's ChaseHQ2.cpp): levels 16-30 push right with
  (31 - level) / 15 of full force, 1-15 left with (16 - level) / 15; no spring. Seen in a race:
  levels 24, 17, 16, 2, 1, idle code 0x4000 in between.
  `WAL_TYPEX_FFB=wacky-races`: the same scheme ([+0x7E00590] + 0x45, lamp bits 0x10, 0x400, 0x200,
  0x80, 0x08, 0x100, 0x4000, FFBArcadePlugin's WackyRaces.cpp codes), levels 16-30 pushing left
  and 1-15 right. Seen in a race: levels 9, 24.
  `WAL_TYPEX_FFB=d1gp`: D1GP Arcade drives its TouchSense wheel through the Immersion library
  (`IFC23.dll`, C++ classes): `immersion.rs` answers the game's 23 imports of it (device,
  project, spring; every call succeeds) and keeps the spring (on/off, center, saturation) and
  the named effects of `data/game.ifr` playing. Every frame the game moves the spring's center
  with the car (self-aligning torque, saturation 0.6-0.7): played as a constant force toward
  it, 2 x its distance from the wheel up to the saturation (`WAL_TYPEX_FFB_SPRING` scales it).
  Effects as vibration: `crash*` 70% for 300 ms, `tire1` 50%, `dirt0` 40%, `engine*` 12%, menu
  `select` / `click*` 30% for 150 ms; `tire0` (grip, a slope) and `steer*` (friction) not
  played. Seen in a race: spring on at the start, `engine0`, `tire0`, `crash1` on wall hits,
  forces up to about 2000 with the wheel centered.

Mahjong games (Taisen Hot Gimmick 5, `mahjong.rs`, `WAL_TYPEX_MAHJONG=hot-gimmick-5`): the
game reads its mahjong panel as a keyboard (DirectInput `GetDeviceState`, one call per frame),
the JVS board only gives the coins and service/test. The call is patched: the key states are
rebuilt from the PC keyboard's `A`-`N` (tiles, MAME's layout) and player 1's virtual stick
(start and b1-b5 = kan, pon, chi, reach, ron: keyboard `1`, left Ctrl, left Alt, Space, left
Shift, `Z`), every other key cleared. The game's own keys: tiles `1`-`0` `-` `Q` `W` `E`, start
`O`, calls `Y` `T` `R` `U` `I`.

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

Elevator Action Death Parade (`WAL_TYPEX_GUNS=elevator-action-death-parade`): the gun board
record at +0x201BE8 (10 bytes a gun: sensor error flags +0/+1 cleared, offscreen +2/+3, x +4,
y +6 in 0..0x3FF0, trigger +8), board state +0x201C10 = 2 (connected), each gun's button
(`b2`, right click) a byte of its player's state (+0x212235, +0x21228D) and a JVS bit; no reload
button (aiming off the picture reloads). Its JVS board is polled with `7D 00` (unknown,
acknowledged like `67 xx`: unanswered, the analog read after it was lost and the game started
in its gunsight calibration screen) and reads a single 4-byte switch block for the whole
cabinet (`WAL_TYPEX_JVS_LAYOUT=elevator-action`, from its switch test: system 0x80 test; byte 0
service 0x40, elevator panel up 0x20 / down 0x10 / action 0x08, 1P gun button 0x02, 2P gun
button 0x01; byte 1 SELECT 0x08, ENTER 0x04): both players' switches merged, each player's
button 1 its own gun button. Master volume = JVS analog input (the cabinet's volume knob, 0:
silent).

Attraction doors (`doors.rs`, `WAL_TYPEX_DOORS=<folder>`): the cabinet is an elevator whose two
physical doors close in front of the screen between stages and shake during some scenes. The
game drives them through `doors(left, right, speed)` (+0xC4B10; 0.0 open .. 1.0 closed, travel
time 4000 - 3000 x speed ms) and `vibration(level, power, time ms)` (+0xC4C40), which return at
once without the door board: their entries are hooked (5-byte jump, first instructions in a
trampoline; prologue bytes checked) and the doors are drawn over the back buffer before each
`Present` (`screenshot::set_overlay`): the dump's `DoorLeft.png` / `DoorRight.png` of
`Attraction Doors Pack/1280x720 alt` (each half of the 720x1280 portrait picture) in system
memory surfaces of the back buffer's format, copied with `UpdateSurface` (kept across resets);
a door moves toward its target at its travel speed, the vibration alternates it by power/200
every 33 ms. Known patches make the door checks of the test menu (+0xC4E20) and of the game
(+0x3EBA0) return true, so the game drives its doors.

Common payload options used by Type X2 games: `WAL_PIN_CWD=1` (`drive.rs`): the game's
`SetCurrentDirectory` stays in its own directory (`.\sh`, `.\data\sh` -> those folders), for Type X2 games (Gaia Attack 4 steps up with `..\`: SOUND ERROR).
`WAL_VFW_CODECS=vidc.wmv3=WMV9VCM.dll` (`vfw.rs`): registers Video for Windows codecs in
`HKLM\...\Drivers32` at startup, the DLL being in the game directory (Gaia Attack 4's WMV9 AVIs,
codec extracted from the dump's `wmv9VCMsetup.exe`).

### 6.3b Raw Thrills (`crates/payload-rawthrills`)

Raw Thrills PC cabinets read their JAMMA USB I/O board (HID) themselves: no driver DLL to
replace, the payload is started by `wal-loader`. `D:\` (logs, personal bests) goes to
`WindowsLoader` (`WAL_RAWTHRILLS_DDRIVE`).

The `sdaemon.exe` games, one engine (`sdaemon.rs`, `WAL_RAWTHRILLS_GAME`: `fnf` The Fast and
the Furious 2006 build, `drift` Drift 2010 build, `superbikes` Super Bikes 2010 build,
`h2overdrive` H2O Overdrive 2010 build; an address table per game, each patch site checked
against its expected bytes; the module doc has the details):

* The board is not opened: its init (open + I/O thread) is replaced by a thread building the
  board's 4-byte reports from the virtual sticks and handing them to the game's own parser,
  which keeps the held flags and pressed-once latches of the inputs (`0x1000..0x1010`) and the
  analog bytes; the per-frame board check always finds the board. Reports: `80` switches
  (coins, start, brake / handbrake, service, test, bill) and volume, `A0` gas / brake, `C0`
  wheel or handlebar, keypad key, buttons 1-3, TUNES, gear switches, `E0` DIP switches
  (default 0: networking off, machine 1; `WAL_RAWTHRILLS_DIPS`: before the board answers they
  read 0xFF, networking on, and the game waited for its linked cabinets).
* The base path of its files (the current directory) is left empty: the run directory's long
  path overran its fixed path buffers (a return into a file name's text after "Checking files").
* The dumps' DirectInput keyboard/mouse controls (IO flags `0x700`) are set back to the board
  only and the developer keys always read released (Space ends a race).
* Brake type: the game's (Super Bikes a switch, The Fast and the Furious an analog pedal with a
  separate HANDBRAKE), `WAL_RAWTHRILLS_BRAKE=analog|switch`. Pedals released read 1: the
  calibration skips 0 as the throttle minimum ("Max Must Be Greater Than Min").
* Gears (manual): Super Bikes shifts up on SHIFT (`0x1005`) and has no shift down (the game
  shifts down when braking): its developer key Down is made input `0x1006` (`shiftdown`); in
  the garage SHIFT changes the bike colour, TUNES the character, the neon colour cycles while
  the brake is held (`shiftdown` out of a race, game state not 0xB / 0xC). The Fast and the
  Furious has a 4 position lever (the gear switch held): `shiftup` / `shiftdown` move a lever
  kept by the payload, `gear1`..`gear4` for a real H shifter. Drift: the same, or a 6 speed
  gate from switch pairs with its `-6speed` (column 0x1005 / 0x1006, row 0x1007 / 0x1008).
* Force feedback (The Fast and the Furious, Drift): the game drives its wheel motor through the
  parallel port (PortTalk, absent: the port stays disabled), its force function (`0x46de10`,
  Drift `0x48a830`)
  (float -1..1 -> one byte, under 0x80 pushing left) replaced: the force goes to the launcher
  as `FFB_CONSTANT` (-force x 10000), no port I/O.
* Keypad (12 keys) read from the PC's numeric keypad; player 2's coin is coin 2.

H2O Overdrive (2010 "Triton" build, a later variant of the engine): the JAMMA USB board is a
device object the game allocates and opens. The board's `Open` (SetupAPI/HID, its I/O thread)
is replaced by a fake that keeps the device and marks the board open, and the board's
per-frame poll is replaced in place: it fills the reports from the virtual sticks and hands
each to the game's report parser (device + report in registers). Once the board reads as
present the update reads its state only through `GetStatusBlock`, replaced by that poll then
the original state copy. The game's input device copies the board's 62 byte state block into
named channels: Wheel, Gas (the throttle lever: neutral in the middle, forward / reverse from
the gas / brake), Start (also the boost, its own control too), View1..3 (the `C0` report's
button byte, active low), Gear (its high nibble, unused, sent released), Test / Service and
VolumeUp / VolumeDown (the `80` report, both pairs swapped against the older games), Coin1,
Coin2, Bill, and the keypad: 12 keys in the `C0` report's key nibble (the tracks and boats
selection). The `80` report's brake switch feeds no channel, the `E0` report is not used.
Service and test presses go through the game's press detector, which reads event rings its
gated-off keyboard poller would write: a trampoline at its entry writes the virtual buttons'
edges to those rings; `GetAsyncKeyState` is hooked for the same keys. The boot additionally
needs the board's PortTalk channel: the game opens `\\.\PortTalk` (the Raw Thrills PortTalk
driver) and must get a handle, then fires `DeviceIoControl` calls it does not check; the
driver is absent, so the game's `CreateFileA` / `DeviceIoControl` imports are hooked: the
channel opens as a fake device, every control succeeds (output zeroed). Movies: the game
handed Bink FMOD's sound output handle as a DirectSound object (not one under Wine): the
attract movies crashed at random; the handle is not fetched, Bink opens its own DirectSound.
Force feedback: the game's wheel force float (positive pushing right) and its centering
spring stiffness, read every frame, sent as `FFB_CONSTANT` / `FFB_SPRING` (on change and
every second).

Guitar Hero Arcade EDITION (game 1.0.5 / OS 8.4) is a Guitar Hero III build. Its GHA payload
bridge follows OpenParrot's GHA loader and shared `XInputEmu.cpp`: it emulates two connected
XInput controllers (the executable imports state, capabilities and audio GUIDs by ordinals
2, 4 and 5), disables hardware state writes, feeds the internal controller state from both
virtual sticks, writes `AspyrConfig.xml` and the Local AppData copy,
and selects the unlocked main-menu or attract-mode PAK files. AWL's `HKLM\SOFTWARE\Aspyr\Guitar
Hero III` open/create requests are redirected to a real per-user registry key, avoiding the
invalid-handle/permissions failure under Wine. The payload also grants the Game Explorer
`VerifyAccess` check and preserves OpenParrot's window title, sizing, drag, cursor, and right-click
minimize behavior. Defaults follow the dump's `teknoparrot.ini` (fullscreen, HD resolution patch,
unlocked menu); `WAL_RAWTHRILLS_GHA_WINDOWED`, `WAL_RAWTHRILLS_GHA_HD_PATCH`, and
`WAL_RAWTHRILLS_GHA_UNLOCK_MAIN_MENU` override those options. The launcher disables Escape and
Back + Start exit shortcuts so those inputs remain available to the game. Without the shared
XInput connection emulation, the game ignored the injected state despite the correct memory
writes. Gameplay and controls work (user: 100%).

Frogger (2011 redemption, `WAL_RAWTHRILLS_GAME=frogger`) uses a serial JAMMA board for
cabinet controls and a USBIO watchdog for status. The payload answers the serial probe,
feeds modulo-16 button edge counters in `S` reports for both players' starts and coins,
test, service and volume, and fakes USBIO discovery, version and pipe setup. Watchdog reads
become pending between reports and complete through the game's OVERLAPPED event; returning
an endless stream of synchronous reports trapped its update loop, freezing the animal rows
while water continued animating. The serial `WaitCommEvent` hook uses its three-argument
Win32 ABI, avoiding stack corruption. HASP imports are stubbed, and the rebuilt executable
loads at its preferred base (`exe_fixed_base: true`). Both players complete rounds; holding
Test enters the service menu, volume buttons navigate, and Escape exits. User-confirmed 100%.
See [FROGGER_CRASH_FINDINGS.md](FROGGER_CRASH_FINDINGS.md) for the decoder and report layout.

All Raw Thrills games: the cabinet reboot (`ExitWindowsEx`, H2O Overdrive leaving its service
menu) asks the launcher to start the game again, the process ended at once (its Direct3D
device's teardown crashed in DXVK); `ExitProcess` is logged with the game addresses on the
stack.

The H2O board games (`h2o.rs`, `WAL_RAWTHRILLS_GAME=supercars`: The Fast and the Furious
SuperCars, 2014 build; USB board through libusb): the game keeps its inputs behind
`SetControl(code, value)` (`0x4a9510`): switches `0x10000 + n` (START, buttons 1-2, MUSIC
`0x10003`, gear lever `0x10005..0x10008`, coins, test, service, bill, volume, digital brake
`0x10010`, keypad `0x10080..`), analog `0x10020..0x10022` (wheel, gas, brake, 0..4095). A
thread calls it from the virtual sticks (switches on change, analog every poll). The input
init (DirectInput keyboard/mouse, USB board) is not called, the board counts as present, its
message sender returns at once, the files' base path is left empty. The removed input init
also sets analog full scale (`0x10042`, at `0x477f910`): it is supplied as 4095 every poll.
Without it, the normalized steering and pedals divided by zero even with valid calibration
(locally reproduced: raw left / center / right 0 / 2048 / 4095, full scale 0; fixed: normalized
steering -1 / approximately 0 / +1).
Force feedback: the force
board's `0x402cf0(float)` replaced, the force sent as `FFB_CONSTANT`.

The same `SetControl` engine, later builds (`h2o.rs`, `alpine` Super Alpine Racer 1.50,
`snowboarder` X-Games Snowboarder 1.23, `snocross` Winter X Games SnoCross 1.58): the board's
named inputs (`start1 view1..3 tunes gear1..4 coin1 coin2 test service bill volup voldown
start2 keypad adc1..3`) bound to the same codes. These take the game's own path without a
board ("No I/O Board found. Using keyboard/mouse."; forcing the board present makes the game
wait for it forever): only the input init (DirectInput keyboard/mouse) is removed and the
thread feeds `SetControl`. Ski games: swing `adc1`, edge / tilt `adc3` (calibrated in the test
menu), views `0x10001` / `0x10002`; Super Alpine Racer draws for a vertical monitor (rotated in
its 1360x768 back buffer), X-Games Snowboarder is landscape. Their dumps' `d3d8.dll` (d3d8to9)
crashes assembling shaders with Wine's d3dx9: hidden, DXVK's d3d8. SnoCross: handlebar,
throttle, brake `adc1..3`, START also the nitro, snowballs `0x10001`, view `0x10004`; its dump
carries protection traps, undone (fatal C runtime heap check, a `jmp $` once a counter reaches
2, a feature check made 1), the cabinet mode flag (USB restarts, reboots on board errors) not
set, a graphics crash skipped, the base path left empty. Unlike the ski builds, its no board
path reboots ("Unable to initialize I/O Board!"): the I/O init result is taken as success, the
reboot made its final idle loop, the "I/O BOARD MISSING" displays and the force board init
return at once. It still reads the PC keyboard's developer keys: Space (ends the game) made
Escape. Force feedback: the handlebar force the race computes (`0x99ab6c`, -1.2401..1.2401,
positive pushing left), read every poll, sent as `FFB_CONSTANT`.

The Fast and the Furious Super Bikes 2 (`sb2.rs`, `superbikes2`, 2010 build): the SuperCars
engine, but the dump's input functions read every board code as a keyboard key (`and ecx,
0xff; jmp` in place of the board test): a board code reads the keyboard state byte `code &
0xff` of the game's table (bit 0 held, bit 1 pressed once), its analog codes a dword of the
game's analog table (0..255, turned into floats through the test menu's calibration). The
game's DirectInput keyboard is not created (its input init removed); its input poll is
replaced to write those bytes and dwords from the virtual sticks on the game thread (also
in the test menu's polling loops): START, VIEW, TUNES, SHIFT, the developer key Down
(shift down), coins, test, service, bill, volume, the digital brake (or the brake pedal past
a third), the keypad; lean, throttle, brake. START also drives the developer Space input
used by this dump to leave attract mode. Analog full scale (`0x10042`) is set to 255 (the
removed hardware init left it at zero, causing division by zero). Build fixes: the back
buffer format kept (R5G6B5 crashed), the race-ending Space check uses an unmapped key
(Escape shared its byte with VIEW), no error box on close, no Watson, no
Escape box, files' base path empty.

Dirty Drivin' (`dd.rs`, `dirtydrivin`, 2011 build 1.14): without its cabinet hardware (Raw
Thrills HID board, FTDI coin board, PortTalk) the game drives the truck from its developer
keyboard device: held keys ramp its steering (`0x8ad0fc`, -1..1) and throttle (`0x8ad0f8`)
floats. Those ramps are removed; a thread writes the floats (throttle minus brake), START and
the crank (developer key Space) and VIEW (V) through the game's `GetAsyncKeyState` (hooked in
its imports, every other key up), coins through the coin board's callback (`0x41eef0(type)`:
coin 1, coin 2, bill; audits and credits), TEST / SERVICE / volume through the switch latches
the game copies into its switch table every frame (`0x98c490..`). Its effects are HLSL
compiled at load: Wine's d3dx9_42 / d3dcompiler_42 fail one and the race crashes on the
missing effect (`tricks: [d3dx9_42, d3dcompiler_42]`); its race needs a 4 GB address space
(`WINE_LARGE_ADDRESS_AWARE=1`). Display (2026-10-13): native fullscreen, user confirmed
booting and fullscreen with `args: [-WinPC, -Windowed, -r, 1360x768]` and the rebuilt
payload. This arcade build still creates a 1368x768 fullscreen device with those arguments.
Batocera Xorg at 1920x1080 selected 1280x800 instead, clipping the right-hand crank prompt.
Its machine user profile now enables Gamescope (`width: 1368`, `height: 768`,
`args: [-S, stretch, -f]`): a captured 1920x1080 output shows the whole prompt, and the
device log confirms 1368x768. Local Wayland keeps the native fullscreen configuration.
Removed the forced windowed back buffer and screen-sized window overrides: the experimental
resize path failed a D3D reset (`0x8876086c`), recreated a 680x384 device and rendered black.
Force feedback: the race's wheel force (`0x886ec4`), mirrored
on tracks 8-14 (`0x96f770`). Build fixes: the hardware init result ignored, no reboot or
watchdog, no hang on a missing device.

Cars (`cars.rs`, `cars`, 2012 build 1.42, the dump's 4 GB patched `sdaemon.exe`): the game
reads its I/O board, the RIO (USB `VID_0C70&PID_0780`), through libusb0 (a reader thread of
its own, 64-byte interrupt packets parsed by type). Its libusb0 imports are answered by the
payload: one bus with the RIO, every call succeeding, writes accepted, each read a type 2
packet: switches as 4-bit transition counters (odd: pressed), two a byte, high nibble first
(coin 1 / 2, bill, SERVICE, TEST, volume up / down, START, VIEW, flip, jump, chatter), and the
12-bit analog channels (steering, gas, brake: high nibbles in bytes 11-12, low bytes in
13-16; calibrated in the test menu). Its HASP dongle (the API statically linked: login,
logout, read, write, encrypt, decrypt) is faked: the game encrypts "Flynn Lives" at start and
checks it decrypts back (identity), the serial number read at 0x28 (`WAL_CARS_SERIAL`);
without it the game reboots. The calibrated steering / gas clamp made unsigned (an infinite
scale while calibrating crashed). Its effects need native `d3dx9_43` / `d3dcompiler_43`
(cars not drawn). Display: native 1368x768 back buffer in a borderless screen-sized window
(`WAL_D3D9_WINDOWED_SIZE`, `WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE=screen`), scaled over the
whole screen (the game's own window left a black frame on larger Xorg modes; user confirmed
the fix). Force feedback: the wheel force the game sets (`[[0x18ae0d8]+0x1b4]+0x258`).

Nicktoons Nitro Racing (`sdaemon.rs`, `nicktoons`): the JAMMA USB board engine of The Fast and
the Furious (same parser, board init, status check, keyboard input functions, motor force
function), a kart: analog gas and brake, no gearbox, view; its IO flags already the board only.
The board device object is supplied for its status paths, lamp/watchdog writes accepted,
and its cabinet feature check emulated. Normal gameplay contains 105 inline AMD 3DNow
math blocks: modern CPUs faulted at `FEMMS` (`0x4b4c10`), leaving a white startup screen.
`nicktoons_3dnow.rs` verifies the complete blocks against fingerprints before replacing
them with SSE2 trampolines, preserving XMM registers, flags, stack operands and 64-bit
memory access widths. Boot, calibration and gameplay are user-confirmed working (100%).
The dump's opening `movies/bump640.ogg` (Theora), `rtlogo.m1v` and `bump2.m2v` contain
video streams only; no embedded audio track was found with ffprobe.

Batman (`batman.rs`, `batman`): Direct3D 11, RIO over libusb bulk transfers through the
shared `rio.rs` transport (Cars continues to use interrupt transfers). Gameplay, calibration
and controls are user-confirmed working. Batman's switch assignments differ from Cars: switch 7
fires, 8 is the digital brake and 9 starts. Default keyboard: 1 start, Left Ctrl fire,
Left Alt view, Space brake button, arrows steer / gas / brake. The brake pedal also presses switch 8 past a
third of its travel. Its type 2 report additionally carries keypad nibbles at payload byte
17 (numeric keypad with NumLock on; decimal/multiply for *, add/Enter for #).
Controls, including the Space brake button and keypad, are user-confirmed. Level loading
then hung in the deliberate spin at `0x12fb710`: the HASP cabinet check (`0x1384dc0`)
returned failure. Both the check and its caller are verified before the check is replaced
with success and cabinet mode `0x1a70c44` set to 3 (the primary cabinet dongle mode).
Mode 2 is a service dongle and draws a green SERVICE overlay; it is not used. Level loading
now starts (user-confirmed); the green overlay is gone with mode 3 (screenshot verified).
Force feedback reads the game's float at image +`0x7ef524` (`0x1abf524` in this build),
normalized by 1.2 and clamped to full scale; positive pushes left. The address and scale
come from [FFB Arcade Plugin's Batman implementation](https://github.com/Boomslangnz/FFBArcadePlugin/blob/master/Game%20Files/Batman.cpp),
verified against this executable's own FFB debug display. Its read instruction is checked
before enabling output. The RIO loop sends constant force through the launcher (gamepad
rumble fallback), including zero when the game releases the force. Runtime output verified
(`ffb: first motor force -499`); physical wheel testing pending.
Batocera at 1920x1080 uses a machine user override at
`userprofiles/rawthrills/batman.yaml`: Gamescope enabled, width 1368, height 768,
`args: [-S, stretch, -f]`. Fullscreen gameplay is screenshot-verified and user-confirmed;
the shared profile keeps the local display settings.

Injustice Arcade (`injustice.rs`, `injustice_cards.rs`, `injustice_overlay.rs`, `injustice`,
2022 UDK build 1.74): the dump's `sdaemon.exe` is the game unwrapped from its HASP envelope
(image base 0x3c0000, a rebuilt import table). The cabinet runs it in UE3's mobile simulation:
`-simmobile` selects its arcade configuration (`Mobile-UDK*.ini`); without it the PC defaults
are generated with the engine's plain `GameViewportClient` where the game's native code
expects its larger `RawGameViewportClient`, writes past it and the garbage collector stops on
an "Unknown token" ~40 s after boot. WinSCard (the NTI smart card reader, absent: free play)
answers a context and no reader; its hooks are stdcall (cdecl ones left the caller's stack 16
bytes off: a crash at the logo). The HASP API linked into the game (login, logout, read,
write, encrypt, decrypt) is redirected to a dongle memory in `WindowsLoader\dongle.bin`: after
6 failed logins the frontend spins on purpose (`jmp $`, the window stops responding); its
dword at 0x30 is the number of cards the operator may still dispense (none: "authorize more
cards", no game starts), refilled at each start. The RIO board is `rio.rs`'s bulk transport
with a second player's map and type 1 status reports (byte 7 0x40: a power-on reset), its 19
switches from the game's button table (coins, bill, test, service, volume, starts, the
players' normal / heavy / block / special, the card dispensers' vend buttons). The card
dispensers' sensor reads answer ready, not empty / low / jammed. Character cards: each player
shows a Code 128 card to a webcam (OpenCV `VideoCapture`, decoded by ZBar). The per-player
device lookup returns the player's camera, the capture calls give blank 640x480 frames, and
while a player's virtual card is on the reader its barcode is drawn into the gray frame handed
to the real `zbar_scan_image`: the game's own decoding runs. The card (130, from
[injustice-arcade-cards](https://github.com/CB9001/injustice-arcade-cards)) is chosen on an
overlay drawn before `SwapBuffers` (fixed-function OpenGL, GDI bitmap font) with the
directions, put on / off the reader with the card toggle; selections in
`WindowsLoader\cards.txt`. The first boot checks all its files (9 GB, minutes) once.
Full screen: a borderless window of the screen's size (the game renders at its window's size).

### 6.3c Namco ES3 (`crates/payload-namcoes3x`)

The 32-bit ES3 payload is installed as the game's Banapass reader DLL (`bngrw.dll`); Mario
Kart Arcade GP DX is its game. Main reference for ES3 games:
[bananatools](https://gitea.tendokyu.moe/TeamTofuShop/bananatools) (public domain,
segatools-style hooks for Bandai Namco PC hardware). Its `mkachook` targets MKDX and its
`dist/mkac/bananatools.ini` gives the game's identity (game id `SBZB`, game cd `MK31`,
ALL.Net version `1.10`, CA config version `18.16`, network `192.168.112.0`). Other useful
parts: `amcus/` (AMAuthd replacement), `board/` (NA-JV JVS board, Banapass serial reader
`bpreader`), `platform/` (dongle, network environment, DNS redirects).

- **AMCUS auth (`amcus.rs`).** The game creates the cabinet auth daemon's COM object
  (`AMCUS/AMAuthd.exe`, interface `IAuth`, CLSID {4603BB03-058D-43D9-B96F-639BE908C1ED},
  local server). Under Wine the call fails: error 0x1a at boot, network treated as down,
  Banapass cards refused ("you can't use the Banapassport card at this time"). The
  `CoCreateInstance` hook returns an in-process object ported from bananatools'
  `amcus/iauth.c`. The game's own wrappers sit at 0x401000-0x401c30 (v1.18.16): slot 9
  `GetUpdaterState` (200-byte state, read by about twenty wrappers), 10 cabinet config, 11
  version info, 14 ALL.Net answer, 18 MUCHA board auth, 19 updater state (must report 1),
  20, 23. Every slot takes one pointer or one dword after `this` in this 32-bit build, so
  bananatools' 64-bit `int64` parameters must not be copied.
- **Card server.** The card-server client talks HTTPS (WinHttp, default port, flag
  `WINHTTP_FLAG_SECURE`) to `192.168.112.254`, paths `/<region>/<api>/<function>`. All
  WinHttp strings are UTF-16. `WAL_NAMCOES3_CARD_SERVER` redirects that host to another
  `host:port` as plain HTTP (`netlog.rs`); `WAL_NAMCOES3_PROFILE_SERVER=1` serves a fully
  unlocked profile there (`profile.rs`, data from
  [MKDX-Unlocked-Profile](https://github.com/Harmmmm/MKDX-Unlocked-Profile):
  `server/getStatus`, `amid/getAmidInfo_bf`, `player/getData`, `judgement/getData`).
- **Test menu.** Two IC card options: the reader, and the card *vendor* (the Banapass
  vending unit behind the STR board). Enabling the vendor without one hangs the boot on
  E23-01 "IC CARD VENDOR DISCONNECT ERROR" (error code 0x62 from 0x6032ea): keep it off.
- **Auto accelerate.** With the accelerator released the race code (0x663400) sets a kart
  flag (`[car+0x9a]`) that forces full throttle ("auto accel is on!"). The payload turns
  the flag's `je` at 0x66389f into a `jmp` unless `WAL_NAMCOES3_AUTO_ACCEL=1`. Found with a
  temporary hardware write watchpoint on the kart's throttle input; pedal values themselves
  were correct.

### 6.3d Sega RingEdge (`crates/payload-ringedge`)

RingEdge games (Virtua Tennis 4; RingEdge 2: Guilty Gear XX Accent Core Plus R) link Sega's amLib statically and import no driver DLL: the
payload is loaded by `wal-loader`. amLib talks to the cabinet's kernel drivers, to the system
services (`mxmaster`, `mxkeychip`) over TCP, and to the JVS board. Each piece found from the
game's amLib (its own error strings name every function):

- **amLib's log (`amlog.rs`).** The release build keeps amLib's log calls, `log(function,
  line, format, ...)`, each gated by its module's level variable. `WAL_RINGEDGE_AMLOG=<rva>`
  replaces the function with a stub writing to the payload log and
  `WAL_RINGEDGE_AMLOG_LEVELS` sets the levels: every remaining failure shows by name
  ("amSramInit(177): CreateFileA Error!!"). Virtua Tennis 4's addresses are in its profile.
- **Devices (`devices.rs`).** `\\.\mxsram` is the save folder's `SRAM.bin` (2 MB; version
  IOCTL `0x9C406000`, size by `IOCTL_DISK_GET_DRIVE_GEOMETRY`, then plain reads and writes).
  The SMBus driver (device interface class `5c49e1fe-...`, SetupDi faked) carries the EEPROM
  (I2C 0x57, `E2PROM.bin`, 32-byte block transfers `0x9C40200C`) and the PCA9535 I/O expander
  (0x20, byte transfers `0x9C402004`): its port 1 holds the main board's TEST (0x04) and
  SERVICE (0x08) push switches, active low, which the test menu reads (the game reads the
  JVS board's); port 0 the DIP switches, also active low, where games read their monitor
  resolution (`WAL_RINGEDGE_DIPSW`, default `FF` all on: 1920x1080 for Virtua Tennis 4, Koihime
  Enbu, Under Night; Ge-Sen Love Plus Pengo! refuses it, "ERROR 0910 Wrong Resolution
  Setting": `8F`, 1280x720). `\\.\columba` reads physical memory: amPlatform scans 0xF0000 for the
  `_DMI_` entry point, reads 64 KB of SMBIOS structures (the table must start at or below
  0xF0000), and takes the system manufacturer (`NEC`) and the third OEM string as the
  platform id (`AAL` RingEdge). `\\.\mxsuperio` (hardware monitor, Winbond-like): chip id
  registers 0x20/0x21 = `A0 20` (`0x9C402004`), the monitor register file read with
  `0x9C40200C` (0x48 = 0x2D) and written with `0x9C40A010`; most games go on without it
  (amHm "not found"), Transformers: Human Alliance stops on a missing status object. Blank SRAM and EEPROM are formatted by amLib itself.
- **PCP services (`pcp.rs`).** mxmaster on TCP 40100, mxkeychip on 40106 (its binary channel
  40107 unused so far); for test menus also mxinstaller 40102, mxgdeliver 40108, mxgcatcher
  40110 and mxstorage 40115, answering `request=<name>` with `response=<name>&result=success`
  (with no service, amLib waits for the connection forever). Text protocol: prompt `>`, request `key=?&cache=1\r\n`, answer
  `key=value\r\n`. Keychip values are text (game id `SBWX`, billing ids) or hex bytes
  (region, flags). `mxmaster.foreground.next=<n>`: the game hands over to another
  application and exits (test button: 3, the test menu = the same executable with `-t`): the
  payload asks the launcher to relaunch with that application's arguments
  (`WAL_RINGEDGE_FOREGROUND`, default `3=-t`; `3=exe=<file> -t`: a separate test menu
  program, which the launcher starts instead of the game).
- **Keychip MAC (DS28CN01).** amDongle asks `keychip.ds.compute=<7-byte challenge>&page=<n>`
  every 100 ms and compares the 20-byte answer with MACs it already knows, else error 0949
  "Keychip Not Found": 100 records loaded at random from the game's `SBWX_Table.dat` (10000
  records of 87 bytes: the challenge, then 4 page MACs whose bytes are shuffled, order in
  `MAC_ORDER`; then 10000 32-byte SSD records), and a fixed challenge with its 4 MACs in the
  executable (found by its bytes, else `WAL_RINGEDGE_DS_TABLE`). The payload looks the
  challenge up in both (`WAL_RINGEDGE_DS_FILE`): no keychip secret needed. Newer amDongle
  (1.14) also asks `keychip.ssd.proof=<16-byte challenge>`: answered from the SSD records
  (answer bytes shuffled, `SSD_ORDER`). Dumped executables can hold amDongle's 100 records as
  loaded at dump time and use them as they are (Transformers' test menu): the challenge is
  then found in the executable's image, its MACs or SSD answer right after it.
- **RingEdge 2** (`WAL_RINGEDGE_BOARD=ringedge2`, newer amLib 1.11). amPlatform's board type
  picks amBackup's SRAM layout: with platform `AAL` and maker `NEC` it reads a fifth OEM
  string, `AAL2` for a RingEdge 2 (4-byte aligned backup records; the RingEdge 1 layout's
  512-byte alignment fails with "invalid address"), and amSram's access unit (`0x9C406004`)
  is 4. amAuth (ALL.Net / NESiCA authentication, `nxAuth.exe` on the cabinet) is one more PCP
  service, TCP 40190: replies `<command>=ok` (`app.property=set&id=<sub game id>`,
  `accounting=start&code=100&player=<n>`), `current.status=ok&error=0&icon=normal&exit=false`,
  `system.time=ok&time=YYYYMMDDhhmmss`; without it the game waits forever before its window.
  The fixed DS challenge and its MACs are the same in all these games (an amLib constant).
  amAuth 1.40 (Under Night In-Birth Exe:Late) asks more: `serial.id=ok&keychip=<id>&main=<id>`,
  `close.time=ok&time=alltime`, `place.info=ok&size=206` with the 206-byte shop record on the
  auth data channel (TCP 40191, sent zeroed), `title.server.address=ok&ipaddr=0` (a number;
  no online server, no Aime card play), `card.info` (Aime reader) acknowledged. Its attract
  mode then shows an Aime banner ("offline, no Aime play" after 2 s offline, else the access
  code hint): a profile patch skips the banner choice (`0x32a502:EB31`; Late[st]:
  `0x2baac6:EB31`). Late's amJvs uses the serial port (`WAL_TYPEX_JVS_PORT: COM4`), Late[st]'s
  the `\\.\mxjvs` driver.
- **JVS.** amJvs opens `\\.\mxjvs` (driver mode), configures it with the serial calls and
  exchanges each packet with `DeviceIoControl` `0x9C402000`; the Type X board (Sega
  837-14572, moved to payload-common `jvs_board.rs`) answers through `serial::transact`. The
  sense line is DSR: modem status 0x10 before the address, 0x30 after (with the Type X values
  amJvs assigns 31 boards: error 6401 "JVS I/O board is not connected").

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
| `mario-kart-arcade-gp-dx` | Mario Kart Arcade GP DX (Namco ES3) | namcoes3 | works (user: 100%); Namcam and Banapass card reader not implemented | version-aware dongle skip (v1.10.22 and v1.18.16), offline router/network bypass, 61-slot IPC compatibility object, Namco ES3 JVS steering and pedals, test-menu settings saved on F:/G: game drives, fullscreen DXVK; force feedback from the game's steering-force float (1.18 offsets implemented; physical wheel output unverified) |
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | not playable (user: in-game bug) | - |
| `aliens-extermination` | Aliens Extermination | globalvr | works (user: 100%), 2 guns (trigger also starts), movies | wal-loader; payload `aliens.rs` (`WAL_GLOBALVR_ALIENS`): gun reader replaced (trigger, special, flamethrower, X/Y; reload = shot off screen), panel keys' `GetAsyncKeyState` answered from the sticks (start, test F1/F2, select, volume), coins into the board's coin queue; Creative OpenAL from the dump's DATA (`files: {dump}/...`); DivX 5 movies not decoded, their error boxes patched out, 1920x1080 game in a gamescope screen scaled to the real one (`-S fit`: smaller screens showed it cut off) |
| `the-swarm` | The Swarm | globalvr | works (user: 100%), 2 guns (fire, grenade, shield); stage changes: the game started again with the next map (Relaunch message), works (user: 100%); new high scores not saved | PC port dump: `the_swarm_controls.exe` run directly with its adapter's arguments; the payload is its .NET input adapter (`WAL_GLOBALVR_SWARM`: shared memory `Local\SwarmPcInputV1`, console and front end commands; `pc_start_mode.lua` a profile file rewritten before each run, players' state from memory, end of stage from `game.log`; imports the dump left unresolved go to msvcr90, a Lua error no longer crashes the game) |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | `tricks: [d3dx9_37]` (Wine fails its .cfx effects, crash) |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | hide dinput8 (loader hack) |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | - |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | D: data in WindowsLoader |
| `battle-fantasia-nesica` | Battle Fantasia (NESiCAxLive) | nesica | works (user: 100%) | Type X JVS payload through wal-loader, 1280x800, A/B/C/D/G layout; no executable patches needed |
| `spica-adventure-nesica` | Spica Adventure (NESiCAxLive) | nesica | works (user: 100%) | Type X JVS payload through wal-loader, standard attack / jump buttons; no executable patches needed |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | key bbcf, shop hours patch |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | - |
| `blazblue-continuum-shift-2` | BlazBlue Continuum Shift II | nesica | works (user: 100%) | hide MS dinput8 |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | d3d8 1280x800, DirectMusic tricks (native dsound) |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user; 1280x800: pillarboxed on a 16:9 screen) | native dsound (wine dsound caps made DxLib compute a 5-million-pixel window / overrun its mixer), ranking NULL-check patches, `WAL_D3D9_FULLSCREEN` (9Ex display mode), `WAL_FONT_SCALE: 0.28` |
| `daemon-bride` | Daemon Bride: Additional Gain | nesica | in fight | key bbcp |
| `dariusburst-another-chronicle` | Dariusburst Another Chronicle (1.05) | nesica | works (user: 100%) | as EX (previous version): NESiCA I/O despite the typex2 folder; key darius, `WAL_FASTIO_COIN: counter`, `WAL_FASTIO_BOARDS: 2`, `WAL_FASTIO_ANALOG1: FF`, `WAL_ANSI_CODEPAGE: 932`, EX init.ini with JVS on (`files:`), right-screen un-flip patch (1.05 addresses) |
| `dariusburst-another-chronicle-ex` | Dariusburst Another Chronicle EX | nesica | works (user: 100%), 4 players, sound, test menu in Japanese | NESiCA I/O despite the typex2 folder; key darius, `WAL_FASTIO_COIN: counter`, `WAL_FASTIO_BOARDS: 2`, master volume knob at maximum (`WAL_FASTIO_ANALOG1: FF`, silent at 0), `WAL_ANSI_CODEPAGE: 932`, init.ini with JVS on (`files:`), 1.16 right-screen un-flip patch (same addresses); 2720x768 back buffer, fine with GE-Proton without gamescope |
| `dark-awake` | Dark Awake: The King Has No Name | nesica | in fight | same as Chaos Breaker (same engine) |
| `do-not-fall` | Do Not Fall: Run for Your Drink | nesica | works (user) | D: data in WindowsLoader |
| `dragon-dance` | Dragon Dance | nesica | works (user), smoke and sparkle effects correct with d7vk; crash on applying saved display settings fixed by a code patch | run game.exe (NxL stand-in), native DirectMusic/dsound, d7vk (wined3d GL drew the particle effects with an opaque black background), code patch (frame drawn while applying the saved display settings, NULL effect object) |
| `elevator-action` | Elevator Action Death Parade | nesica | works (user: 100%), 4:3 | 4:3 picture stretched over its 1280x768 back buffer: windowed 1280x768 (`WAL_D3D9_WINDOWED_SIZE`) in a screen-sized popup, presented to a centered 4:3 rectangle (`WAL_D3D9_ASPECT: "4:3"`), 60 fps (`WAL_D3D9_MAX_FPS`: no vsync in a window) |
| `en-eins-perfektewelt` | EN-Eins Perfektewelt | nesica | works (user) | 1280x800, native dsound (nothing on screen with wine's dsound) |
| `exception` | Exception | nesica | works fullscreen (user) | native dsound (no picture with wine's dsound), `WAL_SDL_FULLSCREEN` |
| `gouketsuji-ichizoku` | Gouketsuji Ichizoku: Matsuri Senzo Kuyou | nesica | works (user) | hide dgVoodoo D3D8/D3D9 |
| `homura` | Homura | nesica | works (user), sound effects and music in game; attract demo silent, as under Windows (original or dump, not the loader) | native dsound (stuck on NOW LOADING with wine's dsound) |
| `hyper-street-fighter-2` | Hyper Street Fighter II: The Anniversary Edition | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `ikaruga` | Ikaruga | nesica | in game (user) | CRT D: redirection (storage error), `fakejapanese` |
| `kof-2002-um` | The King of Fighters 2002 Unlimited Match | nesica | works (user) | `WAL_D3D9_QUERY_FIX` (event query polled into a 1-byte variable: DXVK writes 4 bytes over the saved EBP) |
| `kof-98-umfe` | The King of Fighters '98 Ultimate Match Final Edition | nesica | works (user) | `WAL_D3D9_QUERY_FIX` (event query polled into a 1-byte variable: DXVK writes 4 bytes over the saved EBP) |
| `kof-xiii-climax` | The King of Fighters XIII Climax | nesica | in game, movies | key file 303002.key, crypto plaintext reply, dshow find-filter, xact, remuxed opening.wmv |
| `magical-beat` | Magical Beat | nesica | works (user) | key magicalbeat, init wait patch |
| `nitroplus-blasterz` | Nitroplus Blasterz: Heroines Infinite Duel | nesica | works (user) | - |
| `persona-4-arena` | Persona 4 The Ultimate in Mayonaka Arena | nesica | works (user: perfect) | key persona4arena, plaintext reply, NESYS on, shop hours patch 0x6C9D0 |
| `persona-4-ultimax` | Persona 4 The Ultimax Ultra Suplex Hold | nesica | works (user) | shop hours patch |
| `psychic-force-2012` | Psychic Force 2012 | nesica | works (user) | run game.exe (NxL stand-in), native dsound prefix, patch 1280x768 preset -> 1280x720 |
| `puzzle-bobble` | Puzzle Bobble | nesica | works (user) | - |
| `raiden-3` | Raiden III | nesica | in game (user); intro movie black | hide dinput8; TODO: movies are uncompressed BGR24 240x320 AVIs played through amstream (`IAMMultiMediaStream`, MediaStreamFilter): AVI Decompressor is added but most connections to the media stream are refused (`VFW_E_TYPE_NOT_ACCEPTED`) |
| `raiden-4` | Raiden IV | nesica | in game (user), intro movie (with GE-Proton's winedmo MPEG sequence header fix, after 11-7) | `tricks: [d3dx9_31]` (MMShader.fx), hide ReShade, `WAL_ANSI_CODEPAGE: 932` (Shift-JIS movie name); intro: MPEG-1 elementary stream with its only sequence header at the start of the file, consumed by the demuxer while probing: Wine's DirectShow MPEG decoder dropped the one of the media type ("Invalid frame dimensions 0x0" on every picture) — GE-Proton ge-video-rework/0077 passes it |
| `rastan-saga` | Rastan Saga | nesica | works (user) | 1280x800, hide ReShade |
| `senko-no-ronde-duo` | Senko no Ronde DUO: Dis-United Order | nesica | works, sound effects (user) | hide XAudio2_6.dll + manifests (wine's xaudio2) |
| `skullgirls-2nd-encore` | Skullgirls 2nd Encore | nesica | works (user) | - |
| `space-invaders` | Space Invaders | nesica | works (user) | - |
| `strania` | Strania: The Stella Machina | nesica | works (user), music and sound effects | wine's XAudio2 2.6 / XACT 3.6 (`dll_overrides`: silent with the native ones); the sound effects' XAudio2 comes from the dump's manifests (registration-free COM, as Senko no Ronde DUO): manifests and `XAudio2_6.dll` hidden |
| `street-fighter-3-3rd-strike` | Street Fighter III 3rd Strike: Fight for the Future | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `street-fighter-zero-3` | Street Fighter Zero 3 | nesica | works fullscreen (user) | dump lacks config.ini: Vampire Savior's installed with `files` |
| `the-rumble-fish-2` | The Rumble Fish 2 | nesica | works (user) | `.windowsloader`: `game\Game.exe` (loads `..\data`) |
| `tottemo-e-mahjong` | Tottemo E Mahjong | nesica | works (user); test menu (TestMode.exe) crashes, TODO | run game.exe (NxL stand-in), patch 1280x768 -> 1280x800 limited to game2.exe (`WAL_PATCHES_EXE`) |
| `trouble-witches-ac` | Trouble Witches AC: Amalgam no Joutachi | nesica | works (user) | - |
| `ultra-street-fighter-4` | Ultra Street Fighter 4 | nesica | works (user: 100%), opening video | key usf4, hide Microsoft dinput8 (blocks on raw HID devices), 6-button map |
| `vampire-savior` | Vampire Savior: The Lord of Vampire | nesica | works (user) | NESYS on ("server not connected" when disabled) |
| `yatagarasu` | Yatagarasu: Attack on Cataclysm | nesica | works (user: 100%), fullscreen with its side portraits, Japanese text | hide ReShade, `WAL_ANSI_CODEPAGE: 932`, language files with "MS Gothic" instead of "ＭＳ ゴシック" (`files`, fakejapanese: squares otherwise), picture drawn 854x480 in a 1280x720 back buffer and presented to 0,0 1280x720: `WAL_D3D9_WINDOWED_SIZE: 854x480` (also turns its fullscreen Reset windowed, drops the Present rectangles) in a screen-sized popup (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE: screen`) |
| `million-arthur-arcana-blood` | Million Arthur Arcana Blood | nesica2 | works (user: 100%: training and story modes), NESiCA network connected | NxL2Core.dll replaced by the NESiCA payload (`nxl2.rs`: inputs, coins, settings, card), NESYS on with `ms`/`ss` in the certificate's server list (`WAL_NESICA_NESYS_HOST`), its HTTP card-service reads answered (`/root/match_conf`), native d3dx9_42 |
| `kof-xiv` | The King of Fighters XIV | nesica2 | works (user: 100%), NESiCA network connected | 64-bit NxL2Core.dll replacement (`wal_nesica64.dll`), NESYS on with `ms`/`ss` in the certificate's server list (`WAL_NESICA_NESYS_HOST`), `Resolution=1` (1080p: 1920x1080 window; 0 = 1280x720) |
| `snk-heroines` | SNK Heroines Tag Team Frenzy | nesica2 | works (user: 100%), NESiCA network connected | as KOF XIV (64-bit NxL2Core.dll replacement, NESYS `ms`/`ss`, `Resolution=1`), dump's 32-bit dinput8 hidden; runner fix `winedmo-seek-flush` (hung after START: movie seek deadlock) |
| `samurai-shodown-130` | Samurai Shodown (1.30.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | Unreal Engine 4: `Onion-Win64-Shipping.exe` started directly, payload as the plugin's `NxL2Core.dll` (`payload_install`), NESYS offline (`WAL_NESICA_NESYS_OFFLINE`: online it waits for its card object storage before each fight) |
| `samurai-shodown-180` | Samurai Shodown (1.80.00) | nesica2 | works (user: 100%), NESYS offline; a red banner ただいまネットワークを使ったプレイができません。 (network play unavailable) stays on screen: the game's own offline notice | as 1.30.00 |
| `fighting-ex-layer` | Fighting EX Layer | nesica2 | in game (user: card accepted, game starts), NESiCA network connected; card saved (user: name shown, no freeze after a match) | Unreal Engine 4: `EXF-Win64-Shipping.exe` started directly, its `NxL2Core.dll` replaced by the 64-bit payload; NESYS `ms`/`ss`, card reissue replies and session (§6.2b), card inserted automatically (`WAL_NESICA_CARD_AUTO`), card writes saved as their reads, ASCII case folding (`WAL_FAST_WCASE`: Wine's towlower stalled the game ~5 s twice after a match) |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | wal-loader, JVS, 1280x800, `tricks: [d3dx9_33]` |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | wal-loader, JVS, 1280x800, game patches, runner hotfix (winedmo) |
| `cyber-diver` | Cyber Diver | typex | works (user: 100%) | Source client hl2.exe, -io 1, serial hooks in bin/xioman.dll, complete JVS 0x25 reply, cabinet start and active-low switches, fallback axis calibration; offline; some Japanese labels garbled; testmode.exe absent |
| `block-king-ball-shooter` | Block King Ball Shooter | typex | works (user: touch, coins, start, test menu; 4-player co-op as DemulShooter) | wal-loader, JVS (`block-king` layout), touch sensor driver answered (`WAL_TYPEX_LSDRV`), shared touch for 4 guns (`WAL_TYPEX_GUNS`), patch of its touch position writes, `WAL_DINPUT_DISABLE` (it also read the PC mouse: a second shot at the cursor for each click) |
| `blazblue-calamity-trigger` | BlazBlue Calamity Trigger | typex | in fight (user) | wal-loader, JVS, 1280x800, patch 0xECFD0 |
| `blazblue-continuum-shift` | BlazBlue Continuum Shift | typex | works (user: 100%) | wal-loader, JVS, no patch (generic Type X2 build) |
| `chase-hq-2` | Chase H.Q. 2 | typex | works (user: 100%, wheel/pedals, switches, Nancy videos) | wal-loader, JVS (wheel, gas, brake on analog channels 2-4; START on btn7, PATO-NITRO btn8, SHIFT up, SHIFT-NITRO down), native DirectPlay 8 (`tricks: [directplay]`: wine's IDirectPlay8Peer::Host is a stub, NETWORK CHECKING forever), its own 800x600 mode (no `WAL_D3D9_FULLSCREEN_SIZE`: it drew an 800x600 viewport in the corner), WMV9 codec (`WAL_VFW_CODECS`), known patch skipping the pedal calibration, force feedback from its motor command (`WAL_TYPEX_FFB`, untested on a wheel) |
| `gigawing-generations` | GigaWing Generations | typex | works (user), Landscape/Bezel dump rotated by its ReShade | wal-loader, JVS, native DirectMusic prefix (exits at start with wine's), `reshade_files` dgVoodoo D3D8 + ReShade dxgi, `tricks: [d3dcompiler_47]`, dgVoodoo.conf with Direct3D 11 output (`files`: the dump asks D3D12, NULL device crash) |
| `chaos-breaker-typex` | Chaos Breaker | typex | works (user: perfect) | wal-loader, JVS, native DirectMusic prefix, its window mode (`args: [-window]`) in a screen-sized popup (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE: screen`): Wine drew its fullscreen 640x480 unscaled in the top-left corner |
| `gaia-attack-4` | Gaia Attack 4 | typex | works (user: 100%, 4 guns, coins, sound, videos) | wal-loader, JVS, guns in the gun board record (4 x 10 bytes, `COM1,!COM3`; no patch of its per-frame gun update, which copies it), volume knob on analog 0 (its volume write not patched out), `WAL_PIN_CWD`, WMV9 + Indeo 5 codecs (`WAL_VFW_CODECS`; ir50_32.dll copied into the dump), `dxvk: false` (first attract video crashed), windowed 1280x720 device in a screen-sized popup window (any screen mode; a 1280x720 window went past an 800x600 screen) |
| `gouketsuji-ichizoku-typex2` | Gouketsuji Ichizoku - Matsuri Senzo Kuyou | typex | works (user: 100%) | wal-loader, JVS (native 640x480, no override); movie blocked: VMR second wined3d GL context fails — see docs/GOUKETSUJI-INTRO-VIDEO-DEBUG.md |
| `kof-98-um-typex` | The King of Fighters '98 Ultimate Match | typex | works (user: perfect) | wal-loader, JVS, `.windowsloader`: `launcher.exe`, `WAL_D3D9_QUERY_FIX`, A/B/C/D map, hide MS dinput8 |
| `kof-sky-stage` | The King of Fighters Sky Stage | typex | works (user), rotated by the dump's ReShade | wal-loader, JVS, hide MS dinput8, `reshade_files` ReShade d3d9, `tricks: [d3dcompiler_47]` |
| `haunted-museum` | Haunted Museum | typex | works (user: 100%, guns, service/test, sound) | wal-loader, JVS (`haunted-museum` layout, volume knob on analog 0), guns in the gun board record (`WAL_TYPEX_GUNS`, `COM1,!COM3`), `WAL_PIN_CWD`, MUSEUM.ini with WindowsLoader paths and a 1280x768 picture (`files`; the dump's 1280x720 left the health bars below the screen), window created 1286x5434821 at CW_USEDEFAULT: windowed 1280x768 device (no such mode) in a screen-sized popup window (`WAL_D3D9_WINDOWED_SIZE`, `WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE`) |
| `gundam-spirits-of-zeon` | Mobile Suit Gundam: Spirits of Zeon | typex | works (user: guns, inputs, coins; intro video fixed with `WAL_D3D9_POW2`) | wal-loader, JVS, guns in its JVS data (`WAL_TYPEX_GUNS`), known patches (`WAL_PATCHES`), popup 640x480 window, work area fix, power-of-two texture caps |
| `gundam-spirits-of-zeon-2p` | Mobile Suit Gundam: Spirits of Zeon (2 players) | typex | works (user: perfect, in gamescope, guns) | as `gundam-spirits-of-zeon`, 2 player patch, 1280x480 in `gamescope` |
| `haunted-museum-2` | Haunted Museum II | typex | works (user: guns, inputs, sound, video) | wal-loader, JVS (`haunted-museum` layout, volume knob on analog 0), guns in the gun board record (`COM1,!COM3`), `exe_fixed_base` (rebuilt exe: absolute addresses not relocated, imports without lookup table), known patch restoring its JVSENABLE/GUNENABLE reads (hex-edited to run without boards), both guns pre-calibrated (`initial_files` HM20/HM21.CFG + CRC32), `dxvk: false` (Indeo videos: RADV GPU fault with DXVK), popup 1280x720 window |
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | BLOCKED: error 0002 DISPENSER_ERROR (card dispenser) | wal-loader, JVS (re-init after bus reset), window mode in a screen-sized popup, `WAL_ANSI_CODEPAGE: 932` (d3dx9 DrawTextA); TODO: dispenser (reference "Skip Boot Check") |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works (user: 100%), intro movie | wal-loader, JVS, `dxvk: false`, hide the dump's Wine DLLs, patch 0x447C, runner ddraw overlay emulation (docs/KOF-MIRA-INTRO-VIDEO-DEBUG.md) |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | wal-loader, JVS, 1280x800, A/B/C/D button map, runner quartz fix (#823) |
| `elevator-action-death-parade` | Elevator Action Death Parade | typex | works (user: 100%, perfect), 2 guns, attraction doors and bezel, crosshairs option | wal-loader, JVS (`7D` acknowledged, cabinet switch layout `elevator-action`, volume knob on analog), guns in the gun board record (`WAL_TYPEX_GUNS`, aim relative to the picture), attraction doors and bezel drawn from the dump's pictures in a screen composed over the game's window (`WAL_TYPEX_DOORS`, `doors.rs`; crosshairs `WAL_TYPEX_CROSSHAIRS`), PhysX 2.8.1 system runtime (`tricks: [physx]`), portrait 720x1280 windowed device, 60 fps, known patches (door board error, door checks) |
| `king-of-fighters-xiii` | The King of Fighters XIII | typex | works (user: 100%) | wal-loader, JVS (native 1280x720 in a fullscreen window, no override) |
| `kof-xiii-climax-typex2` | The King of Fighters XIII Climax (Type X2) | typex | works (user: 100%) | wal-loader, JVS, Taito crypto service (`WAL_NESICA_KEY: kofxiiiclimax`, plaintext reply: media.kof is encrypted, black screen without it), dshow find-filter, xact, as the NESiCA build |
| `music-gungun-2` | Music GunGun! 2 | typex | works (user: perfect), attract movie, sound, 2 guns | wal-loader, JVS, guns in both gun records (`WAL_TYPEX_GUNS`), `WAL_PIN_CWD`, patches: 0x137C70, 60 Hz-only mode enumeration (screens at 120 Hz), 1920x1080 instead of 1360x768 (no such Xwayland mode), volume knob at maximum (muted otherwise); start keys = trigger; GE-master video DLLs (0080: attract movie hung) |
| `raiden-3-typex` | Raiden III | typex | works (user); intro movie black (as NESiCA) | wal-loader, JVS; Notice screen ~65 s |
| `raiden-4-typex` | Raiden IV | typex | works (user) | wal-loader, JVS, `tricks: [d3dx9_31]` (MMShader.fx), `WAL_DINPUT_DISABLE` (keyboard 2 opened the test menu) |
| `shikigami-no-shiro-3` | Shikigami no Shiro III | typex | works (user), Landscape/Bezel dump rotated by its ReShade | wal-loader, JVS, `WAL_WINDOW_POPUP` (overlapped window: empty frame), `reshade_files` d3d8to9 + ReShade d3d9, `tricks: [d3dcompiler_47]` |
| `samurai-spirits-sen` | Samurai Spirits Sen | typex | works (user: 100%) | wal-loader, JVS, the dump's original `launcher.exe` with known patches in memory (`WAL_PATCHES`, `WAL_PATCHES_EXE`; its `game.exe` is a hex-edited copy), saves on D: |
| `senko-no-ronde-duo-typex2` | Senko no Ronde DUO: Dis-United Order | typex | works (user: perfect) | wal-loader, JVS, native 1280x720, hide xinput1_3 + XAudio2_4 and its manifests (wine's xaudio2), as the NESiCA build |
| `spica-adventure` | Spica Adventure | typex | works (user: 100%) | wal-loader, JVS |
| `new-super-mario-bros-wii-coin-world` | New Super Mario Bros. Wii Coin World | typex | works (user: 100%), 4 satellites, medals, hoppers, satellite test menu | wal-loader, backup SRAM (`TxedLap.dll` answered), JVS watchdog, medal I/O board on COM1 (`WAL_TYPEX_MEDAL_PORT`); XAudio2 2.7 32-bit registration (shared prefix; crashed after the device creation without it), windowed 1920x1080 device in a screen-sized popup window, 16:9 kept (its fullscreen 1920x1080 was refused on a 1600x1200 screen) |
| `taisen-hot-gimmick-5` | Taisen Hot Gimmick 5: Mirai Eigou | typex | works (user: 100%) | wal-loader, JVS (coins, service/test), mahjong panel read as a keyboard (`WAL_TYPEX_MAHJONG`: tiles A-N on keys A-N, start and calls from player 1), hide the dump's dinput8.dll (another loader's input hack: window not responding) |
| `tetris-the-grand-master-3` | Tetris The Grand Master 3 Terror-Instinct | typex | works (user: perfect) | wal-loader, JVS, OpenGL, `WAL_WINDOW_POPUP` (overlapped window: empty frame), save folder patch, picture height 448 -> 480 (white bars) |
| `street-fighter-iv` | Street Fighter IV | typex | works (user: perfect), intro video plays | wal-loader, JVS, native 1920x1080 (no back buffer override), hide MS dinput8 |
| `super-street-fighter-iv-ae` | Super Street Fighter IV Arcade Edition (export, standalone) | typex | works (user: 100%), intro video plays | wal-loader, JVS, 1280x720 game: windowed 1280x720 device scaled to the screen, 16:9 |
| `super-street-fighter-iv-ae-cabinet` | Super Street Fighter IV Arcade Edition | typex | works (user: 100%) | cabinet build (`launcher.exe`), as the export build: 1280x720 windowed device scaled to the screen, 16:9 |
| `super-street-fighter-iv-ae-2012` | Super Street Fighter IV Arcade Edition Ver. 2012 | typex | works (user: 100%) | cabinet build (`launcher.exe`), asks for 1920x1080 (1280x720 was zoomed): 1920x1080 windowed device, 16:9 |
| `valve-limit-r` | Valve Limit R | typex | works (user), wheel, pedals, races start at once; TODO: option to hide the passenger girl's cut-ins | wal-loader, JVS (gas/brake on analog channels 1/2), steering board on COM1 (`WAL_TYPEX_WHEEL_PORT`), standalone link patch, `WAL_DINPUT_DISABLE`; `.windowsloader`: `launcher.exe` |
| `d1gp-arcade` | D1GP Arcade | typex | works (user: wheel, pedals, buttons, test menu); TODO: force feedback check on a wheel, magnetic card reader (saves) | wal-loader, JVS (pedals on analog channels 3/4, START on btn7, SHIFT up/down btn1/btn2, side brake right, view btn8), fake DirectInput wheel "Immersion TouchSense Steering Wheel" (`WAL_DINPUT_WHEEL`: steering, and the game merges the JVS switches only with such a device), native DirectPlay 8 (`tricks: [directplay]`), standalone patches (boot network check passes, the link task never hosts nor draws), "Handle Initialize" stage goes on without the TouchSense effect (it stopped forever), no card: no in-game hardware error checks nor "CARD VENDOR / ERROR" (its magnetic card reader/vendor on COM1 is not emulated; the game backs card data up in `d:\card_backup\`), force feedback from its Immersion calls (`WAL_TYPEX_FFB`, untested on a wheel), its 1368x768 layout as a windowed device in a screen-sized popup window, 16:9 kept (it picks its fullscreen mode from the monitor's: 1280x800 at 1920x1080, right side cut) |
| `wacky-races` | Wacky Races | typex | works (user: controls, start, view) | wal-loader, JVS (wheel/gas/brake on analog channels 2/3/4, START/VIEW on buttons 8/7, LEVER on down), the dump's root Launcher.exe (set up to play at once), `WAL_KEYBOARD_STATE_DISABLE` (it read START/VIEW on the PC keyboard's Enter/Right Shift), force feedback from its motor command (`WAL_TYPEX_FFB`, untested on a wheel) |
| `battle-gear-4` | Battle Gear 4 | typex | works (user: 100%) | original Japanese release (2005): as `battle-gear-4-tuned` (same cabinet, inputs, key reader, `WAL_GAME_DRIVE: E`), patches of this executable (clean: no window menu, intro fix), 800x600 windowed in a screen-sized popup, force feedback from its memory (`WAL_TYPEX_FFB=battle-gear-4-original`), attract movie through DirectShow on its game drive (it crashed ~20 s after boot when not found) |
| `battle-gear-4-tuned` | Battle Gear 4 Tuned | typex | works (user: wheel, pedals, buttons, intro, music, coins), test menu | wal-loader, JVS (gas/brake on analog channels 3/4, shift up/down on buttons 2/3, key reader: `battle-gear` layout), steering board on COM1 (inverted, `WAL_TYPEX_WHEEL_AXIS=-lx`), `WAL_GAME_DRIVE: E` (data found from the current directory's drive; music through `mmioOpenA`), patches: hex-edited entry point and transmission lookup restored, no window menu, intro fix, wide monitor without the clutch type (the cabinet type check sets both flags: patched to set only the wide monitor one); 1360x768 windowed in a screen-sized popup (`WAL_D3D9_WINDOWED_SIZE`), `WAL_HIDE_CURSOR`, `WAL_DINPUT_DISABLE` (it shifted gears on the PC arrows and keypad), force feedback from its memory (`WAL_TYPEX_FFB`, untested on a wheel) |
| `battle-gear-4-tuned-pro` | Battle Gear 4 Tuned (professional) | typex | works (user: H shifter, clutch, coins in the I/O test) | as `battle-gear-4-tuned` (a copy of its dump with this `.windowsloader`), professional cabinet: second JVS board, H shifter (player 2's b1-b6), clutch on analog channel 6 (`+ry`), 1360x768, force feedback as the normal cabinet |
| `fast-and-furious` | The Fast and the Furious | rawthrills | works (user: 100%) | wal-loader, JAMMA USB board reports to the game's parser (§6.3b), files' base path left empty, IO flags back to the board, analog brake + HANDBRAKE, 4 speed lever from shift up/down, keyboard ramp 0.5 s, force feedback from its wheel motor force (untested on a wheel); The Fast and the Furious Drift: dump missing |
| `fast-and-furious-drift` | The Fast and the Furious Drift | rawthrills | works (user: 100%) | wal-loader, as `fast-and-furious` (§6.3b), switch brake, 4 or 6 speed lever, screen size back to 640x480 (`WAL_PATCHES`: the dump's data said 1280x720, menus in the corner), force feedback from its wheel motor force (untested on a wheel) |
| `fast-and-furious-supercars` | The Fast and the Furious SuperCars | rawthrills | works (user: 100%) | wal-loader, H2O board inputs through the game's `SetControl` (§6.3b), analog full scale 4095 (normalized steering / pedals otherwise divided by zero), 4 speed lever from shift up/down, files' base path left empty, its 1360x768 picture in a windowed device over the screen (Direct3D 8 interval fix), force feedback from its force board's force (untested on a wheel) |
| `fast-and-furious-super-bikes` | The Fast and the Furious Super Bikes | rawthrills | works (user: 100%) | wal-loader, JAMMA USB board reports to the game's parser (§6.3b), files' base path left empty, keyboard ramp 1 s (lean, throttle), switch brake, shift down = developer key made a board input |
| `fast-and-furious-super-bikes-2` | The Fast and the Furious Super Bikes 2 | rawthrills | works (user: 100%) | wal-loader, inputs sampled in the game's input poll (§6.3b), Start also drives the developer Space input, analog full scale 255, 4 coins per credit, lean / throttle / brake calibrated in the test menu, back buffer format fix, race-ending Space check disabled |
| `dirty-drivin` | Dirty Drivin' | rawthrills | works (user: 100%) | wal-loader, developer keyboard floats and keys (`GetAsyncKeyState` hooked), coins through the coin board callback, switch latches, `tricks: [d3dx9_42, d3dcompiler_42]` (race crashed on a missing effect), `WINE_LARGE_ADDRESS_AWARE=1`, force feedback |
| `frogger` | Frogger (2011 redemption) | rawthrills | works (user: 100%), two-player controls, service menu, volume | wal-loader, serial JAMMA button edge counters, bounded asynchronous USBIO watchdog reports, HASP stub, fixed-base executable; attract freeze and WaitCommEvent stack corruption fixed |
| `cars` | Cars | rawthrills | in game (user: accel, TEST, calibration) | wal-loader, RIO board emulated behind libusb0, HASP dongle faked, calibration clamp fix, `tricks: [d3dx9_43, d3dcompiler_43]`, force feedback |
| `nicktoons-nitro-racing` | Nicktoons Nitro Racing | rawthrills | works (user: 100%) | wal-loader, JAMMA board and cabinet feature check emulated, 105 verified 3DNow math blocks replaced by SSE2, calibration and gameplay confirmed, opening movie has no embedded audio track, force feedback from its wheel motor force (untested on a wheel) |
| `batman` | Batman | rawthrills | works (user-confirmed gameplay and controls; FFB wheel test pending) | wal-loader, RIO board over libusb bulk transfers, wheel and pedals, fire / start / brake switches corrected, numeric keypad, cabinet check emulated to avoid level-loading spin, force feedback from game float (wheel test pending) |
| `injustice-arcade` | Injustice Arcade | rawthrills | works (user: 100%, virtual card readers) | wal-loader, `-simmobile`, HASP dongle emulated (cards authorized refilled), RIO board over libusb bulk transfers with status reports, card dispensers ready, webcams emulated: virtual barcode cards chosen on an OpenGL overlay, full screen; open: an intermittent crash in the attract movie (Bink) |
| `teenage-mutant-ninja-turtles` | Teenage Mutant Ninja Turtles | rawthrills | works (user: 100%), four-player controls, test menu, variable-height jump, videos | TMNT 1.59 (`TMNT.exe`); run `tools/build-tmnt-direct-input.sh <game directory>` to patch `IOManager.Update` with direct launcher input before `UpdateCombinedInputs`, including per-frame counter snapshots and hold timers; WALInput disabled and TMNTTPPlugin input writer disabled; menu developer keyboard shortcuts disabled to avoid player overlap; Page Up / Page Down for volume; `Pawn.UpdateJump` uses a local height/gravity helper instead of the TeknoParrot host call, restoring landing and hold-dependent jump height; bundled Windows `OrenVid.dll` playback works; no winhttp override |
| `super-alpine-racer` | Super Alpine Racer | rawthrills | in progress: boots, a race played from scripted inputs (swing, start) | wal-loader, inputs through the game's `SetControl` without a board (§6.3b), DXVK's d3d8 (dump's d3d8to9 hidden), picture rotated for a vertical monitor (no rotation yet) |
| `x-games-snowboarder` | X-Games Snowboarder | rawthrills | in progress: boots, a race played from scripted inputs | as Super Alpine Racer, landscape |
| `winter-x-games-snocross` | Winter X Games SnoCross | rawthrills | works (user: 100%) | wal-loader, inputs through the game's `SetControl` (§6.3b), no board: I/O init taken as success, no reboot, board missing displays off; the dump's protection traps undone, keyboard developer key Space (ends the game) made Escape, base path empty, force feedback from its handlebar force (untested on a wheel) |
| `h2o-overdrive` | H2O Overdrive | rawthrills | works (user: 100%) | wal-loader, JAMMA USB board reports to the game's parser (§6.3b), the board's `Open` faked, its poll and `GetStatusBlock` replaced, service / test edges written to the press detector's rings, throttle lever centered (gas forward, brake reverse), boost its own control, Bink's sound not taken from FMOD (attract movie crash), service menu exit reboot = relaunch, keypad selects the tracks and boats, force feedback from the game's wheel force float (untested on a wheel) |
| `guitar-hero-arcade` | Guitar Hero Arcade EDITION | rawthrills | works (user: 100%) | Guitar Hero III game 1.0.5 / OS 8.4; two connected virtual guitars, OpenParrot GHA input, per-user Aspyr registry redirect, generated config and menu PAK selection, Game Explorer and window behavior |
| `virtua-tennis-4` | Virtua Tennis 4 | ringedge | works (user: 100%), test menu | wal-loader, amLib devices (SRAM, SMBus EEPROM and push switches, SMBIOS platform id), keychip and master PCP services with the DS28CN01 MACs from the game's table, JVS on `\\.\mxjvs` (§6.3d), test button relaunches into the test menu (`-t`) |
| `guilty-gear-xx-accent-core-plus-r` | Guilty Gear XX Accent Core Plus R | ringedge | works (user: 100%) | RingEdge 2 (§6.3d): platform `AAL2`, amAuth PCP service (TCP 40190), 4-byte SRAM access unit, keychip MACs from `SBYG_Table.dat` |
| `under-night-in-birth-exe-late` | Under Night In-Birth Exe:Late | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), `RingGame.exe.orig` (the dump's `RingGame.exe` is patched), amAuth 1.40 commands, JVS on COM4 |
| `under-night-in-birth-exe-late-st` | Under Night In-Birth Exe:Late[st] | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), newer version of the same game: `RingGame.exe.orig`, amAuth 1.40 commands, JVS on `\\.\mxjvs` |
| `koihime-enbu` | Koihime Enbu | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), amLib 1.21 (2015 build), JVS on `\\.\mxjvs`, APM network client offline |
| `blade-arcus-from-shining` | Blade Arcus from Shining (2.10.00) | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), `crus.exe` (OpenGL), amAime, JVS on `\\.\mxjvs`, profile patch: the attract banner's scene test always hides it (`0x1749b6:EB`) |
| `blade-strangers` | Blade Strangers (1.02.00, 1.3.2) | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `himekaku.exe` (OpenGL), amLib 1.21 (2015 build), JVS on `\\.\mxjvs`, APM network client offline |
| `caladrius-ac` | Caladrius AC | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `SILVER_AMP_R_ROM.exe`, JVS on `\\.\mxjvs`; its own init (no amLibInit) gives up on an invalid credit settings record (backup record 7, made by the system menu) and amCredit stays uninitialized (main loop spins): profile patch `0x13fc1:9090` goes on, amCreditInit takes its defaults |
| `melty-blood-actress-again-current-code-re2` | Melty Blood Actress Again Current Code (RE2) | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `MBAA_RWMasterBuild.exe.orig` (the dump's `.exe` is patched), amLib 1.21, JVS on COM4 |
| `phantom-breaker-another-code` | Phantom Breaker: Another Code | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `pbac_ringedge2.exe`, JVS on `\\.\mxjvs`; a windowed 800x600 device with its 16:9 picture in the middle 800x450: borderless screen-sized window, `WAL_D3D9_SOURCE` presents that band stretched |
| `ge-sen-love-plus-pengo` | Ge-Sen Love Plus Pengo! | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `glpp.exe`, JVS on COM4, DIP switches `8F` (resolution bits valid: native 1280x720) |
| `transformers-human-alliance` | Transformers: Human Alliance | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), 2 lightguns on the JVS analog channels (`p2:` inputs), 20 general purpose outputs (else Error 6402), super I/O hardware monitor, keychip SSD proof, Wine large address aware; dumped executables with `.rdata` made read-only (R6002); test menu `TestMode_dumped.exe` (JVS on COM4, item fields zeroed by a patch, keychip records from its own image) |
| `guilty-gear-xrd-rev2` | Guilty Gear Xrd Rev 2 | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), Unreal Engine 3, `Binaries/Win32/GuiltyGearXrd.exe` with `game.bat.orig`'s arguments (`-AUTH`), cabinet drive M: = the dump's SAVE folder, JVS on `\\.\mxjvs`; Aime banner (`AIME_OFFLINE`) skipped by a patch |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user-confirmed): `-launchGame`, coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | wal-loader, `TsuInput` + `TsuMotion` (idle motion seat) COM objects emulated in the payload, Wine DirectInput with host joysticks hidden, dump's `tsunet.dll` registered in-proc + adapter-walk patched, d7vk; TODO: cabinet env (`C:\Tsunami\`, `launch.reg`) reproducible from the profile — see docs/REVOLT-DEBUG.md |

Wine's builtin DirectSound breaks several games in ways that do not look like sound bugs
(Crimzon Clover's window/mixer sizes, Dragon Dance's crash, Homura stuck loading, Exception and
EN-Eins showing nothing, Psychic Force never opening its I/O): they need native dsound and
DirectMusic (winetricks, their `tricks`). The shared prefix has them; the games that do not
list dsound get wine's.

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
* More systems (Type X JVS, ...).
