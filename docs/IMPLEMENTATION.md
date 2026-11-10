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
  payload-segashell/       Sega Amusements Shell payload (the Shell's TCP service, the I/O board), loaded by wal-loader
  payload-alls/            Sega ALLS payload (amdaemon's keychip, USB I/O board, network; the LED board), loaded by wal-loader in amdaemon and the game
  payload-seganu/          Sega Nu payload (amLib's AMEX board devices, keychip driver, system registry values; JVS), loaded by wal-loader
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
* Wine runner: the default runner is `GE-Proton11-12-x86_64` (`wine-runners/`); **all game
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
| card keypad | `kp0`..`kp9` (e-Amusement readers' keypad: the card's PIN; Konami profiles map the numpad) | bits 17-26 |
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
  * Players (`players.rs`): the devices connected at start are added in player order, the ones
    plugged later take the first free player. The order: the GUI's priority list
    (`userprofiles/players.yaml`, SDL GUIDs, P1 first), then the frontend's order
    (`WAL_PLAYER_PADS`: evdev nodes, P1 first, `;` separated, set by the Batocera generator from
    ES's players), then SDL's. Devices sharing a GUID take its entries in the frontend's order.
    A wheel (calibrated, SDL's wheel type, or a raw `input.devices` joystick) is always P1, the
    first gamepad joins it there; the others (and a second wheel) take the first free player.
    An `input.devices` entry's `player` wins.
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
  `PREFIX_SETTINGS` (`reg add`, applied once, recorded in `<prefix>/.wal-settings`): winebus'
  SDL, evdev and hidraw backends off, so no game of any system sees a game controller
  (winmm, DirectInput, XInput, HID): games read the I/O board the payloads emulate, a pad
  read next to it mixed the players (Dariusburst: P1's stick moved P1 and P2).
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
for the games listing it only, and Microsoft's Windows Script Host 5.7 (`wsh57`: VBScript,
JScript, scrrun, UltraPin's tables need its `ExecuteGlobal` / `GetRef`): the other games keep
wine's (its DLLs are `trick_dlls` in `wine.rs`, the isolated verbs `ISOLATED_TRICKS`). `physx` (PhysX system software, legacy 2.x runtimes) serves the
PhysX 2.8.1 games whose `PhysXLoader.dll` looks the runtime up in the registry (Elevator Action
Death Parade: no PhysX scene without it, crash at the first stage). `d3dx9_42`: Million Arthur
Arcana Blood imports it. `dll_overrides` (`<dll>: n | b | ...`) has the last word. The overrides
are written `*<dll>`: wine looks up `*dsound` before `dsound`, in the environment then the
registry, and winetricks writes `*dsound` keys, which a plain `dsound=b` would not beat. A
game that cannot share the prefix sets its own `prefix` (and `prefix_tricks`).

Runner hotfixes: `tools/runner-hotfixes.py [runner...]` patches known bugs of runner builds in
place (exact byte match, `<file>.orig` backup, idempotent), until the fix ships in a release.

GE-Proton11-12 (default runner since 2026-11-08) is the GitHub release (`GE-Proton11-12-x86_64.tar.gz`,
sha512 checked) plus `bin/winetricks`, with the PE DLLs `winedmo`, `quartz`, `mfreadwrite`,
`mfsrcsnk`, `mfasfsrcsnk`, `mfmp4srcsnk` (i386 and x86_64) rebuilt from the GE-Proton11-12 tree
(`~/code/wal-wine-build-1112`: the wine section of the tag's `protonprep-valve-staging.sh`, the
same `configure` as below, `make dlls/<dll>/all`, `strip --strip-debug`; originals in
`hotfix-backups/<arch>-windows/`). 11-12 ships every earlier fix up to GE patch 0109; added:
0110/0111 (Pokkén Tournament movies), 0112 (Wasteland Racers 2071 attract movie), and from
`~/code/protonge` branch `winedmo-video-decoder-system-memory-fallback`:
* 0113: the MPEG-4 Part 2 decoder's YUY2 D3D11 samples, which DXVK cannot create, replaced by
  system memory samples (`init_allocator` failed with `E_INVALIDARG`, every frame lost).
* 0114: the video processor converts system memory input on the CPU into its D3D output sample
  (its re-enabled D3D11 path failed and fell back to a memory sample: Unity's VideoPlayer wants
  `IMFDXGIBuffer`).
* 0115: a video decoder takes input again once its drain completed (the source reader drained it
  a second time when a sample request raced the seek back to 0: a looped movie stopped on its last
  frame after one to a few loops, 11-10 too).
Big Foot Crush's attract video needs all three. The release's unix `winedmo.so` is kept (same
24-entry unix call table as the tree's build).

GE-Proton11-10 (default runner 2026-10-19 to 2026-11-08, still in rgs for the winebat games pinned
to it) ships every fix of the GE-Proton11-7 list below
(GE patches 0072-0075, 0080, 0091, the upstream demuxer-destroy fix, no mountmgr serial probe);
it only carries `winedmo-sws-padded-output`:
* `winedmo-sws-padded-output` (GE-Proton11-10 `winedmo.so`, x86_64 and i386, rebuilt from the
  GE-Proton11-10 tree: the wine section of `patches/protonprep-valve-staging.sh` with the fix as
  `patches/ge-video-rework/0095`, `configure` per architecture, `make dlls/winedmo/winedmo.so`,
  `strip --strip-debug`; originals in `hotfix-backups/<arch>-unix/winedmo.so.orig`): the video
  conversion ran `sws_scale` straight into the caller's exactly-sized output buffer; swscale's
  SIMD output stores past the last row (24 bytes on a 216x120 RGB24 frame), over the next heap
  block's header: Target Bravo: Operation G.H.O.S.T. crashed in `RtlFreeHeap` (WM reader demux
  thread) entering its stage select. Now scaled into a padded `AVFrame`, then `av_image_copy`.
  Sent to GE-Proton (branch `winedmo-swscale-padded-output`). WoW64 runners load the
  `x86_64-unix` side for 32-bit games: patching only `i386-unix` changes nothing.

GE-Proton11-7 list (previous default runner):
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
* `winedmo-parser-rewind-after-eos` (2026-11-02, GE-Proton11-10 `winedmo.dll`, i386 only,
  previous file `hotfix-backups/i386-windows/winedmo.dll.pre-0109`): GE patch 0009 (Arcania)
  made `dmo_seeking_SetPositions` take an absolute position equal to `seek.llCurrent` as
  unchanged (recorded, no flush, no seek); but that is the position the streams were last
  sought to, not where they are. Frenzy Express loops its MP3 music with `SetPositions(0)` on
  `EC_COMPLETE`: the parser had sent its EndOfStream, skipped the seek, never delivered again,
  and the graph's next state change waited forever for the DirectSound renderer (the game's
  window thread hung in `xshow.dll`'s window procedure, ~10 s after START). The position is now
  unchanged only while no active pin delivered a sample or EndOfStream since the last seek or
  start. GE patch `0109` on `~/code/protonge` branch `winedmo-parser-rewind-after-eos` (worktree
  `~/code/protonge-wt-parser-rewind`, pushed to the fork for a PR). Built from the
  `~/code/wal-wine-build/build` objects with the parser of the upstream chain (0080, 0091, 0100
  applied: that tree's parser predates them) plus 0109, linked out of tree, stripped. Found with
  the hang watchdog (`crash::heartbeat` called from the payload's `XIO::update`).
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
Ctrl-C/SIGTERM → `wineserver -k` (the game still running 5 s later: its process group killed;
the shared prefix extracted again during the run gave the game's server another folder, the
server folder being named after the prefix directory's inode), child exit → break } →
`wineserver -k` for leftovers.

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
  then crashed ~20 s after boot stopping its never created movie (NULL `IMediaControl`). The
  DirectShow hook is also installed for a redirected drive without a game drive (Winning
  Eleven 2008's attract movies, `D:/HCK/data/movie`); a game DLL's own file functions are
  redirected with `redirect_module`, its `mmioOpen` too (Winning Eleven 2008's sound.dll banks).
* `wasapi`: `WAL_WASAPI_SHARED=1` plays a game's exclusive-mode WASAPI stream in shared mode
  (winepulse.drv has none: Initialize fails, silence): exclusive IsFormatSupported answered for
  PCM / float, Initialize shared with `AUTOCONVERTPCM`, GetBufferSize one device period, the
  game's GetBuffer / ReleaseBuffer through a FIFO given to the device as it has room (spice2x's
  `-wasapishared`). Wine's IAudioClient / IAudioRenderClient vtables patched on the first device
  (Bishi Bashi Channel).
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
  presenting after its Reset. A 0x0 back buffer (the window's size, SEGA Golden Gun) takes the
  window's client size first. `WAL_D3D9_WINDOWED_SIZE=WxH` creates/resets every device windowed
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
* `WAL_D3D9_UPDATE_TEXTURE_FIX=1`: an `UpdateTexture` the device refuses between two 2D
  textures is redone level by level with `UpdateSurface`, from the destination level of the
  source's top level size. Koihime Enbu uploads its 1024x1024 L8 character sprites from 1-level
  system memory textures into 11-level default pool ones: DXVK returns D3DERR_INVALIDCALL
  (source with fewer levels) and the characters stayed invisible. The first refusal is logged
  with both textures' descriptions.
* `d3dx`: the game's `D3DXCompileShader` calls without an include handler get one reading
  `#include` files from the current directory, as native d3dx9 does (Wine's compiler fails
  them: SEGA Golden Gun's blur shaders, a NULL effect, crash at boot). Always on.
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
  `WAL_DINPUT_WHEEL_Y_AXIS` / `_Z_` / `_RX_` / `_RY_` / `_RZ_AXIS` map more axes (Rx-Rz listed
  only when mapped), `WAL_DINPUT_WHEEL_BUTTONS=<virtual button>,...` the buttons in order
  (`none`: unused; default b1-b8, start, coin, service, test). The wheel follows the game's
  `SetDataFormat` (each object at the offset the game gives it) and answers
  `DIPROP_LOGICALRANGE` / `DIPROP_PHYSICALRANGE` like `DIPROP_RANGE` (GRID scales its axes by
  the logical range). `WAL_DINPUT_WHEEL_FFB=0`: no force feedback capability,
  `CreateEffect` unsupported (Showdown).
* `mouse_stick`: `WAL_DINPUT_MOUSE_STICK=<speed>` keeps the real DirectInput 8 and wraps the
  system mouse device's `GetDeviceState` (vtable slots patched from `DirectInput8Create` /
  `CreateDevice`): player 1's left stick is added to its relative X / Y, `<speed>` counts per
  poll at full tilt, sub-count remainders kept (Super Monkey Ball Ticket Blitz's trackball).
* `window`: `WAL_WINDOW_SIZE=WxH` (or `screen`: the primary monitor's size) forces the size of the game's top-level windows
  (`SetWindowPos`/`MoveWindow` IAT hooks, at 0,0); in window mode Direct3D stretches the back
  buffer to it (Crimzon Clover's DxLib computed a 5-million-pixel high window: X BadAlloc).
  `WAL_WINDOW_POPUP=1` creates the game's top-level windows as borderless popups
  (`CreateWindowExA/W` IAT hooks: `WS_POPUP`, caption/frame/system menu removed). Shikigami no
  Shiro III makes its fullscreen Direct3D 8 device on a plain overlapped window (style 0, so a
  caption and border): the window manager showed an empty frame with the desktop behind. Their
  menu bars are removed too (`SetMenu` after creation, the game's `SetMenu` dropped): Thrill
  Drive 4's window showed the DirectX sample framework's File menu above the picture.
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
  (`/root/match_conf`). 64-bit games too (Tekken 7 Round 2:
  `.?AVConnectionRefusedException@Net@Poco@@`, a server on the wrong port).
* An unanswered request logs `nesys: unhandled command 0x..` and the game waits: the command
  ids (requests 0x1-0x2B, replies and notices 0x101-0x12A) are listed in `nesys.rs` (`cmd`).
* HTTP posts: dump them (`WAL_NESYS_HTTP_DUMP=1`), the form's `type` says what is read; reply
  formats differ per game (Million Arthur skips 2 bytes and wants XML; the Groove Coaster card
  server format is `1\n1,1\n<xml>`). Only the replies the game parses matter: find the parse
  code from the reply handler (0x122: the client copies the body to its session object) and
  its XPath strings.
* NxL2 settings the game reads are logged (`nxl2: setting <name> = ...`): e.g. KOF XIV's
  `Resolution` (0 = 720p, 1 = 1080p).

**Theatrhythm Final Fantasy All-Star Carnival (Unity 5.3 Mono, 64-bit, `theatrhythm.rs`).** No
`NxL2Core.dll`: the C# code DllImports three plugins of `game_Data/Plugins`, all three replaced by
`wal_nesica64.dll` (profile `payload_install`):
* `ismACIO.dll` (I/O board): `GetInputD` bits right lever 0x1-0x8, left lamp switch 0x20, Enter
  0x80, left lever 0x100-0x800, Select 0x1000, Test 0x2000, Service 0x4000, coin 0x8000, right
  lamp switch 0x10000; `GetCoin` / `DecreaseCoin` a coin counter; `GetStatus` 2 (OK). Natives
  for `native_map`: `lup ldown lleft lright llamp rup rdown rleft rright rlamp select enter test
  service coin`; the right lever also follows the right stick (`rx`/`ry` past half way).
* `NESiCAReader.dll`: XIO status idle (1), `Read(timeout)` waits for the `card` input's card
  (the game wants the reader empty when it starts reading: insert the card at the title, not
  with `WAL_NESICA_CARD_AUTO`), `GetID` returns non-zero with the 16 characters.
* `SimpleNesys.dll` (HTTPS REST client, not the `nesys_games` pipe), `theatrhythm_nesys.rs`:
  ONLINE_MODE after `NESYS_START_ONLINE_MODE`, maintenance time -2 (-1 turns the reader off),
  the certificate's `card_server` the payload's own (`127.0.0.1:33702`), `game_server` the
  Photon address (must not be empty); card data `error_number` 0 (2020 would start the NESYS
  registration); requests answered at once (handle > 0, status OK); `NESYS_PARSE_JSON` calls
  back for each wanted key (names ANSI, strings UTF-16, booleans 4-byte). `WAL_THEATRHYTHM_SERVER=0`:
  STANDALONE_MODE, every request -4, guest play only.
* Card server (`theatrhythm_server.rs` HTTP, `theatrhythm_player.rs` answers, records
  `theatrhythm-server/<card>.json`): Unity `JsonUtility`, so every field is sent (no null).
  Errors are HTTP 200 + `result` `NG` + `code` (a non-2xx body is never read); `service/echo`
  must be non-JSON (`OK`; JSON counts as an error); `service/version`'s `serviceVersion` must be
  the game's (`GameVersion` header) or error 31001; `event/musicscoreranking` 404; one
  `info/banner` info URL at least (the Information screen divides by their count); first play =
  `user/profile` NG `#004` (no name entry), the record made at `user/accountlogin`; optional
  services NG (play bonus U011, score ranking U013, domination U006/U004 with `eventInfo:{}`,
  raid boss U019/U018); 3 parties of 4 characters; the song list 0 or >= 11 songs and not 20
  (the client asks for details 10 at a time and divides by zero otherwise): the 171 default
  medleys + the unlocked and played ones; 6 levels per song detail. Saved: `playupdate` (gains
  summed, scores, parties, settings, characters' absolute exp), `playreward` (unlocks, absolute
  medals / crystals, title), `playresult` (titles), `infoupdate`, `shopbuy`. Collection cards'
  rarity is not kept (their table is in an asset bundle).
* Managed code (`theatrhythm_mono.rs`, payload-common `mono.rs` `Mono::replace`: a jump at the
  start of the method's compiled code, `mono_compile_method`; small methods may be inlined, so
  only larger ones): `ServerManager.SetProtcol` does nothing (the card server stays `http://`),
  `PhotonManager.Connect` sets `Online` for the boot / attract checks (no lobby option) and
  `Standalone` for a credit (`GameStart` passes an option, even empty: `Online` would wait for a
  room forever, black screen), `Disconnect` does nothing. The matching server is checked every
  30 s in attract and gates the NESiCA reader.
* Mono loads each plugin by full path: the first copy loaded hooks game.exe's and mono.dll's
  `LoadLibraryW/ExW` so the other names return this module (one state).
* Touch screen = Unity's mouse (Windows player): pointer `accel`/`brake`, touch `b7`, messages
  posted and real ones filtered as Chase Chase Jokers; raw input registration and Unity's
  XInput (`GetProcAddress`) hidden. The pointer is a red dot drawn over the picture
  (payload-common `pointer_d3d11.rs`: DXGI `Present`/`Present1` hooked through a swap chain of
  the payload's own, drawn in a device context state of its own);
  `WAL_THEATRHYTHM_POINTER=arrow` keeps the Windows cursor shown and warped instead, `none` hides it.
* `NanoTS_PTool_win8.exe` (touch panel setup / calibration / `connection:state`, polled) answers
  "Not connected" without the USB controller (error 15001): mono.dll's `CreateProcessW` runs
  `cmd.exe /c echo Connected` instead.
* D:\ (settings, bookkeeping, logs) through mono.dll's file functions (`drive::redirect_module`).
* The dump's `Assembly-CSharp.dll` is another loader's modified build (free play, checks
  removed, data in `.\`): the profile installs `Assembly-CSharp_org.dll` (the original) instead.

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
* Other boards: `WAL_TYPEX_JVS_IDENTIFIER` (the `10` identification string) and
  `WAL_TYPEX_JVS_SWITCHES=<players>x<switches>` (the features' switch layout, default `2x16`):
  Let's Go Safari checks for `837-14895` with a 6th field `FE MODE`, 1 player with 22 (or 14)
  switches and 16 outputs (else error 6402). Its wheel is a rotary encoder on 2 switches (a
  step per pulse): `WAL_TYPEX_JVS_PULSE=<button>,...@<ms>` pulses those buttons while held,
  `ms` on then off (toggled on each `20` read, it does not turn).
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

Harikiri Professional Baseball (2005, `launcher.exe` = the cabinet's `game.exe`, `-s`), a
touch screen baseball card game (offline here: the 12 teams, no cards nor saves; 2 coins):
* Touch screen on the JVS screen position input (`25 01`, `WAL_TYPEX_JVS_TOUCH`): x, y big
  endian raw values (reply parser +0x87BBA: `0x8EE006` / `0x8EE008`), the touch switch is switch
  byte 3 bit 0x10 (`native_map` `b4: btn12`). The touch object (`0x8EDE90`) maps the raw point
  through a 4-corner calibration (d:/game.ini `[Caliblation]`, defaults when missing: the test
  menu's inset marks at 280,320 .. 6580,5060, center 3460,2720) onto the display mode
  (`IDirect3DDevice9::GetDisplayMode`, +0x23F3) minus `ClientToScreen` of its window. The
  profile's corners (the picture's corners) were fitted from the positions the game computed for
  a 5x5 grid of raw points (within 5 pixels). The pointer is player 1's `rx`/`ry` (mouse or gun)
  mapped into the picture (`screenshot::to_picture`), drawn as a red dot (common crosshair).
* A 1024x768 game making its back buffer the display mode: `WAL_DESKTOP_SIZE=1024x768` with
  `WAL_DESKTOP_CLIENT=1` (display mode, window and client rectangles, `ClientToScreen` in desktop
  units: the test menu sizes its back buffer and window from them) and `WAL_D3D9_WINDOWED_SIZE`.
* In its window fitted on the screen two mouse loops froze it (endless `WM_MOUSEMOVE`): its window
  procedure moves the Direct3D cursor to the mouse on each mouse message
  (`SetCursorPosition`, call at +0x99E07 removed) and its exclusive DirectInput mouse (Wine
  confines and warps the cursor; `WAL_DINPUT_DISABLE=1`).
* Test menu (`switcher.rs`, `WAL_TYPEX_SWITCHER=switch.exe`): the test switch runs
  `switch.exe test.exe -s`, which closes the game window, starts `test.exe -s` and exits (back:
  `switch.exe game.exe -s`). The program asking asks the launcher to start the next one when it
  exits (`exe=test.exe -s`) and starts the switcher with the payload, whose own start of the next
  program is answered as done. The test menu's EXIT restarts the cabinet (`ExitWindowsEx`): the
  game started again. Its on-screen buttons (↑ ↓ MENU ENTER) are touched; SWITCH TEST: ENTER =
  switch byte 2 0x08 (button 7), SELECT 0x04 (button 8).


Usagi - Yasei no Toupai Online (2005, `mjc.exe`; the cabinet's `game.exe` starts it from a
`launch.conf` missing in the dump), an online mahjong game on a touch screen (offline here:
in-store battles and story mode against the computer, no cards; 2 coins):
* Boot: the game reads `etc/mjcconf.txt` (missing: defaults) and `d:/config/gameconf.txt`
  (missing: written with the defaults; its write thread waits forever when `d:/config` does not
  exist, a black screen). The profile installs it once (`initial_files`) with `FACTORYSETTING
  FALSE`: otherwise the first boot starts with the clock setting screen (years up to 2020).
* Network check (CABINET NO.01 (MASTER): ETHERNET, MASTER, LLB, NESYS, WMR, WLB, PDB, CDB, RKM,
  EVM): the game connects on TCP 52001 to the local message relay `bin/lmrd.exe`, which the lobby
  `bin/llb.exe` also joins (the master flag from `d:/config/gameconf.txt`); `game.exe` starts
  both with `--no_daemon`. Without them it retries forever (MASTER/LLB ERROR). The payload
  starts them (`servers.rs`, `WAL_TYPEX_SERVERS`, in their folder, in a job ending them with the
  game); they run without the payload, D: is a Wine drive (`drives`). The relay's `MRN0 = NESYS`
  asks `nesys.taito.co.jp` (`/server/certify.php?gid=400`) for its world relay host: the
  profile's `etc/lmrd.conf` names a local one (nothing answers, no Internet request; NESYS stays
  ERROR -1029 on the screen). The check's state 9 then waits 10800 frames (+0xD2866, 3 minutes)
  for the world relay before marking WMR..EVM ERROR and going on offline: 60 frames.
* Switches (`20 01 03`, packed big endian with the system byte at +0x9EB5C8): test system 0x80,
  service byte 1 0x40, ENTER / SELECT byte 2 0x08 / 0x04 (buttons 7 / 8), touch byte 3: 0x08 the
  panel's line, 0x04 touching (+0x45A580: the position read while both are on, a release when
  0x08 stays on without 0x04: `WAL_TYPEX_JVS_HELD=btn13`, the touch `b4: btn14`).
* Touch position: the JVS screen position input (`25 01`), raw x, y mapped by +0x45A6A0 with
  `gameconf.txt` CENTERX/Y (3332/2748), CHANGEX/Y (1923/1939) and the TX1/TY1 skew (stored - 5000,
  0 by default): x = (raw - CENTERX) x 100 x CHANGEX / 10^6 + 640, y likewise + 512, on the
  1280x1024 picture; the profile's corners are that mapping's raw values at the picture's
  corners. The game makes a mouse of it (+0x41DC80: position, held, click on the first frame)
  for its 128 touch areas.
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

Sonic Blast Heroes (`punch.rs`, `WAL_TYPEX_PUNCH=sonic-blast-heroes`), a punching machine:
the dump's `game.exe` is encrypted (the cabinet decrypts it), `game_release.exe` is the
decrypted v1.04. The cabinet's pad is a custom XIO node (`GWXioNode_Hero`: pad sensor switches,
the time the pad takes to pass its speed sensors, force ~ C / time² then a calibration table),
and a light curtain (3 HUMAN-SENSOR switches) guards it: none of them on for 0.2 s is "safety
sensor error, call an attendant", then the test menu. The game also has keyboard versions
(`CPunchingPadEmu`: a punch is F4 held, `CHumanSensorEmu`: broken while F9 is held), used when
XIO is absent: the two adapters' `je` are made `jmp` (both forced), the pad's F4 queries (held,
pressed, released call sites) answered from the virtual sticks (held: `accel` past ~18% or
`b1`), F9 never, and the pad's punch end (its vtable slot, which turned F4's hold time into the
raw value) replaced: the force is the trigger's peak during the punch (b1: full force) times
`WAL_TYPEX_PUNCH_MAX_KGF` (default 200), turned into the raw value by a binary search over the
game's own `GetKgf` (+0x31890, thiscall on a scratch pad; its float conversion overflows to
0x80000000 for the shortest times: counted as beyond any force). Its two buttons are XIO
system bits 0x100000 (Confirm, JVS down) and 0x200000 (Selection, JVS up).

K-On! After School Rhythm Selection sells collectible cards (a card dispenser) and scans up
to 3 of them before a game (a barcode reader). The dispenser is wired to the JVS board
(`WAL_TYPEX_JVS_LAYOUT=k-on`, `jvs_board.rs`): the card-ready line is player 1's switch byte 2
bit 0x80 (jam 0x40, empty 0x20; the game reads the `20 01 03` reply as one dword: system, then
the 3 switch bytes), the dispense pulse general purpose output byte 1 bit 0x08 (`32 03`). At
boot the game wants one of the 3 lines on (else error 0002 DISPENSER_ERROR), the empty and jam
lines off; after a dispense pulse the ready line must drop within 5 s (the card leaving) and
come back after 2 s to 60 s (the next card, else DISPENSER_JAM): it drops for 1 s. The
emulated dispenser is never empty. The game's card reader "connected" check returns true in this
build. The scanner is a keyboard (`kon_cards.rs`, `WAL_TYPEX_KON_CARDS=1`): the window
procedure feeds each `WM_KEYDOWN` to the reader (+0xCE70), which takes 13 digit keys less than
a second apart, then Enter, Ctrl, J; the 13-digit JAN code is looked up in the card table
(+0xBCC4C, 304 entries of 24 bytes; 0x62, 0x67 and 0x6A-0x99 refused: 254 cards). The payload
reads the codes from the table, draws a picker over the game (the game's own
`data\common\card_<n>.tga` pictures: the selected card, its neighbours smaller; drawn with the
common Direct3D 9 quads, `d3d9_quads.rs`, shared with the crosshair), browsed with player 1's
b1/b2 (b3/b4 by 10; `native_map` keeps them off the JVS board) and scans with `card`: the
keys posted to the game window, through the common key filter (`input_filter.rs`, the
executable's `PeekMessageW`: the real keyboard hidden). Selection kept in
`WindowsLoader\kon-card.txt`.

Won!Tertainment Music Channel (Capcom, 2009, version 4) is a kids' rhythm game on a touch
screen with a robot dog toy, a card dispenser and a card scanner (`WAL_TYPEX_WONTERTAINMENT=1`,
`wontertainment.rs`). Its JVS switches (`WAL_TYPEX_JVS_LAYOUT=wontertainment`, `20 02 02`, found
with its I/O test): system byte SELECT 0x80; player 1's first byte ENTER 0x40, door 0x20, the
dispenser's gate sensor 0x10 (labelled ENABLE: a card waits at the gate; off at boot: error
100), JAM 0x08, EMPTY 0x04; its second byte the LEFT / RIGHT buttons 0x80 / 0x40, SERVICE 0x08,
CARD OUT 0x04 (the card in the exit tray), TEST 0x01. A dispense (a phase machine polled every
100 ms, 0x43F480): motor on (output byte 1 bit 0x10) for 0.5 s, then the card must be gone from
the gate, which comes back on with the next card; CARD OUT's release is the card taken. The game
polls its switches irregularly (an input poll for ~10 output packets, gaps up to a second), so
the emulated gate's drop can arrive after the 0.5 s check: that check's jump is patched out
(0x3F552), the later phases still read the gate. Its two I/O threads sometimes write the input
poll's header over the output packet (`06 20 02 a b 33 06 ...`, a bad checksum): a checksum error
reply made it resend the same bytes then stop (error 903), so the packet is answered as it
parses, and the motor bit is read from the outputs before `33 06`.
COM1 is the infrared link to the Idol Puppy toy: frames `FD 02 ~(code+seq) code seq [data]`, a
reply's size from its code; the presence query `7E` wants the toy's `7E` frame (two equal state
bytes, then 0xAA), error 700 otherwise; its other frames get the toy's `AA` ack. COM3 is a
barcode scanner sending 12 digits + CR LF, read as a 48-bit hex value whose low byte of the high
word is the card's family; error 600 without the port. The cards come from the game's tables
(`wontertainment_cards.rs`): Puppy 0x11 (110), Outfit 0x21 (315), Accessory 0x22 (190),
Miracle Power 0x31 (30), the code at +8, the Shift-JIS name at +0x18 (the operators' version
cards 0x23 left out); a picker overlay (the names drawn with GDI into a Direct3D 9 texture,
`d3d9_quads.rs`): up/down browse, left/right the family, `card` scans (pushed on COM3), kept in
`WindowsLoader\won-cards.txt`. The touch screen is the mouse on the cabinet (a touch panel
driver): the window procedure reads `WM_MOUSEMOVE` (`GetCursorPos` over the client size) and the
left button; the gun's place in the letterboxed picture is posted as the mouse
(`input_filter::post_mouse`, `own_mouse`), the touch on b3.

Common payload options used by Type X2 games: `WAL_PIN_CWD=1` (`drive.rs`): the game's
`SetCurrentDirectory` stays in its own directory (`.\sh`, `.\data\sh` -> those folders), for Type X2 games (Gaia Attack 4 steps up with `..\`: SOUND ERROR).
`WAL_VFW_CODECS=vidc.wmv3=WMV9VCM.dll` (`vfw.rs`): registers Video for Windows codecs in
`HKLM\...\Drivers32` at startup, the DLL being in the game directory (Gaia Attack 4's WMV9 AVIs,
codec extracted from the dump's `wmv9VCMsetup.exe`). `WAL_VFW_READ_FIX=1` hooks the game's
`AVIStreamRead` around two Wine avifil32 bugs (`IAVIStream_fnRead`, upstream Wine, not
GE-Proton's) on fixed sample size streams (PCM audio): a size query
(`AVISTREAMREAD_CONVENIENT`, no buffer) answers 0 bytes (asked again with the stream's suggested
buffer size worth of samples), and a convenient read with no `bytes`/`samples` output is refused
(`AVIERR_BADPARAM`; given an output). Storm Racer G's movie audio.

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

#### Point Blank X

`PBX100-2-NA-MPR0-A63.exe` (Unity 5.1.3, Mono, 32-bit). The cabinet's I/O is the Mono
`DllImport`s of `BNUsio` (`Assembly-CSharp-firstpass.dll`) into
`PBX100-2-NA-MPR0-A63_Data/Plugins/bnusio.dll` (cdecl, plain names): the USIO board (switches,
coin and service counters, outputs) and the gun board on a COM port (`GUNPORT`). The game uses it
when `Setting.ini` says `CONTROLLER 0`; `1` is a PC keyboard/mouse mode (dumps ship with `1`
and a BepInEx input plugin).

- **Install.** `payload_install` puts `wal_bngrw.dll` in place of `Plugins/bnusio.dll` and
  `payload_loader` loads it at the executable's entry point (wal-loader): the filters are in
  place before Unity's first frame, and Mono's later load of that path gets the same module.
  The payload's `DllMain` takes the PBX path from its own file name (`bnusio.rs`).
- **Board (`bnusio.rs`).** `BNUsioController.update` calls `usio_Communication` once per
  frame, then `usio_GetSwIn` (test 0x80, service 0x4000, up 0x2000, down 0x1000, enter 0x200,
  P1 start `SW_PUSH17` 0x2000000, P2 start `SW_PUSH18` 0x1000000), `usio_GetCoin` /
  `usio_GetService` (counters the game decrements with `usio_DecCoin` / `usio_DecService`) and
  the guns. The virtual sticks are sampled in `usio_Communication` (edges for coin, service,
  test toggle). Boot checks: `usio_GetSystemError` 0, `usio_GetFirmwareVersion` 120..=126,
  `gun_OpenEx` 0. Gun positions are 10-bit values shifted by 6 (the original's `<< 6`),
  divided by 65535 and passed through `EightPointsGunCalibration`, whose default (no saved
  calibration) is the identity with y up: `lx` as is, `ly` flipped, 0 avoided (no position
  for the game). `gun_IsInRange` false while the `offscreen` native is held (b6, the gun's
  off-screen source). Natives for `native_map`: `trigger offscreen start coin service test up
  down enter`.
- **Test switch.** It latches on the cabinet: `test` toggles it (`WAL_NAMCOES3_TEST_TOGGLE=0`:
  held). On: the operator menu (`OperatorScene`); off: back to the game.
- **Crosshairs.** The game draws one per player when `GlobalData.isCrosshairVisible` (false by
  default, the cabinet has none). `WAL_CROSSHAIR=1` (the common option, the profile's default) sets it from
  `usio_Communication` (the game's main thread, Mono attached): the `Singleton<GlobalData>`
  instance is found with `mono_reflection_type_from_name`
  (``Artoncode.Core.Singleton`1[[GlobalData, Assembly-CSharp]]``, `mono.rs`'
  `class_of_type_name`).
- **Serial number.** The boot check shows `HKLM\SOFTWARE\NBGI\SerialNumber` (ERROR when
  missing, not fatal), written at load (`WAL_NAMCOES3_SERIAL`, default `WAL000000001`).
- **Input filter.** Unity reads the keyboard (Escape quits) and, in the simulated mode, the
  mouse: `payload-common`'s `input_filter` (shared with arcade PC, the executable's
  `PeekMessage`/`GetMessage` since Unity 5 has no `UnityPlayer.dll`).
- **Display.** `BootSceneController.Start` sets 1280x720 full screen; the picture is scaled
  to the screen.
- **Not done.** Outputs (gun recoil `gun_StrikeOnce`, start lamps `usio_SetGout` 15/16, player
  lamps 9/10), the ticket dispenser (`TICKET 0`).

#### Mach Storm

`src\game\ACE7_WIN\ACE7_WIN_10.exe` (2013, Direct3D 9, 32-bit, `machstorm.rs`), a dome
cabinet flight game; its data is `..\..\..\boot\WIN_GAME\USRDIR`. It imports no board
DLL: `wal_bngrw.dll` is loaded at the entry point (the system's loader) and the executable's
build (PE timestamp 0x52cfbf33, image size 0xf3a000) selects the Mach Storm code.

- **I/O board.** NA-JV on COM3 (`jvs.rs`, `init_flight`, the Battle Pod layout): analog 1
  throttle lever (0x80 idle), 2 / 3 control lever x / y; byte 0 service 0x40, up 0x20, down
  0x10, enter 0x02; byte 2 machine gun 0x02, missile 0x01, a third button 0x04 (the game's
  input object, 0x470e70, reads one byte per switch from the board state at +0x188).
  `WAL_NAMCOES3_REVERSE_Y` / `_THROTTLE` invert the axes.
- **Patches.** The device error list's push (0x7071e, `add [esi+4],0x10`) is removed: the
  cabinet camera, USB dongle and network errors are never shown. Two boot checks of an extra
  serial device (`RSTALL`, `vST` commands, 0x76430 / 0x76870) wait minutes without it: they
  return 1. The save path `%c:\%s\%s.dat` (drives F and G) becomes `.\%c%s.dat`, the
  `F*.dat` / `G*.dat` files dumps carry next to the executable.
- **Flat screen.** The picture goes through a 1920x1080 RG16 lookup texture made for the dome
  projector. `WAL_NAMCOES3_FLATSCREEN=1` (default): a 9-byte stub in the padding at 0x3d3637
  replaces the data argument of the texture creation called at 0x3d2fed with a flat screen
  map (each half bent toward the center, rows scaled with it; v below 0 wraps).
- **Window.** A 1920x1080 windowed device: the profile's `WAL_WINDOW_POPUP` /
  `WAL_WINDOW_SIZE: screen` make it a borderless full-screen window.
- **Input filter.** XInput (#2-#4) reports no pads; keyboard and mouse messages filtered.
- **Not done.** Outputs (lamps, fans, seat), the cabinet camera (photo).

#### Lost Land Adventure

64-bit (`crates/payload-namcoes3`, `lla.rs`): Unreal Engine 3, 2014, a dome cabinet for two
guns. The dump's `RSLauncher.exe` (cabinet launcher: I/O board, card reader, credits, test
menu) starts `LLA\Binaries\Win64\DomeShooterGame-Win64-Shipping.exe`; they share state through
`RSSharedData`. Both import `hasp_windows_x64_106993.dll`: the profile's `payload_install` puts
`wal_namcoes3.dll` in both places, and that library name selects the Lost Land code.

- **Dongle.** The game's dongle thread (0xa30950) checks, every 3 s, the USB key (the same
  enumeration at 0x20b0 as Battle Pod's, id 0x0c10) or the HASP: 0x40 bytes at 0xd00,
  decrypted, byte 0x3f the complement of 0x3e, then 13 bytes against the run-time serial
  template 0x16b3248 (`27282*15****`, `*` any character). The launcher's own check (0x68ad0,
  else **06-01 GUN CONTROLLER ERROR**, fatal) wants `2728`, characters 8 and 9 `0`, 10 `0`-`4`,
  11 not `0`: both get `272820150001`.
- **Launcher errors.** Its error manager (`[0x40e898]`, codes 1-67 registered at 0x337f0 with
  their texts) keeps a log of the raised codes (+0x1c0: code, tick), saved in
  `ErrorLogData.bin`. A screen "FATAL ERROR" without a code: dump that log.
- **I/O board.** `wajvio.dll` on COM3 (NA-JV, `jvs.rs`). `WAJVGetInput` gives one byte per
  switch (+0x188 player 1, bit 7 of each byte first; +0x187 test): byte 0 service 0x40, up
  0x20, down 0x10, enter 0x02, operator panel 0x01; byte 1 start 1 0x80, start 2 0x40 (names
  from the launcher's test menu variables `varTestStart1`...). The test switch only counts with
  the panel open, so the panel bit is on with the latched test switch.
- **Card reader.** The international ROM (`Data\romVersion.txt` `LLA100-1`; mode
  `[0x40e8b0]` from its 8th character) wants a Banapass reader on COM4 (else 07-12 / 07-11):
  NFC frames `00 00 FF len lcs D4 cmd ... dcs 00` (PN532 style), answered by `bpreader.rs`,
  ported from bananatools' `board/bpreader.c`. No card is ever presented.
- **Projector.** An NEC projector on COM1 (`projector.rs`): the launcher polls error status
  (`00 88`) and running status (`00 BF ... 02`) every 3 s; absent, it reopened the port every
  5 s, showed 09-08 "SERIAL COMMUNICATION WARNING", and GNOME reported it "not responding".
  Answered: no error, running status data byte 2 = 0x04, byte 3 = 0 (1: 09-10).
- **Language.** `-Language=` from the operator setting `[0x341084]` (1 JPN ... 8 THA, else
  none = INT). `WAL_NAMCOES3_LANGUAGE` patches the read at 0x2c28e. The profile uses JPN: the
  community English patch puts its English texts in the JPN files (the launcher adds
  `-voicelanguage=English(US)` for JPN), and the `*_LOC_JPN.upk` packages are the only
  localized ones. Menu captions baked in textures stay Japanese.
- **Flat screen.** The game renders a 3520x1980 back buffer of two 90 degree views yawed 45
  degrees left and right, then draws the screen through a lookup texture of `Startup.upk`
  made for the dome projector. Dumps carry a known patch (`CreateDomeMap`, on the
  uncompressed package) replacing it with a rectilinear view (about 127 degrees wide); the
  payload relies on it (the crosshair mapping below was measured with it). The attract demo's
  fisheye is a separate render, left as is.
- **Guns.** The game reads its gun camera itself on COM2 (115200 bauds, 0xa2eec0): 12-byte
  packets `5F player x y buttons F5` (x, y: f32 camera coordinates, about 0..1; buttons bit 0
  player 1's trigger, bit 2 player 2's: `GunPeripheral1PTriggerMask` 1, `...2PTriggerMask` 4;
  bit 6 camera error), a streaming device in the payload. The game maps them through its dome
  model (0xa2e570: a point on the dome; 0xa2e820: the fisheye projector image position, passed
  to the script's `SetMousePosition`); the crosshair (a 3D actor placed by yaw / pitch) is
  then drawn elsewhere on the flat picture. Measured on a 5x5 grid: a yaw / pitch of the dome
  point seen from (0, 0.375, -0.134) (the config's `DomeEyePositionY` 0.375), yaw x1.0985,
  pitch x1.5159 + 0.4233 rad, then `0.5 + 0.2812 tan(yaw)`, `0.5 - 0.3921 tan(pitch) /
  cos(yaw)`, measured with the patched texture: within about 1% of the screen (2% on the
  edges), in menus and in game. The payload inverts it (Newton steps through the game's own 0xa2e570): each gun aims where it
  points. The game writes 2 bytes back per poll (gun outputs, not used).
- **Test menu.** The launcher's window (created with style 0 at the default size) is a small
  topmost status overlay (network icon) it moves about 30 times a second; in test mode it goes
  full screen (GDI menu) and the game window shrinks to 256x60 (0x62ee0), the reverse when it
  ends (0x62fd0). The overlay took the game's focus and GNOME pinged it ("not responding");
  over the game as a popup it took the game out of full screen. The payload creates it as a
  popup and, while small, moves it off the screen without activation; when the launcher puts
  a window full screen on top (the menu, then the game back), it is activated. The window
  manager still keeps the full-screen game above the menu (a black screen on Batocera): alt-tab
  to see the test menu. Its first show must stay
  (hidden, or shown without activation: the test menu pages were never built, a crash at
  0x48210 on the test switch). The test switch counts only with the operator panel bit
  (byte 0, 0x01) on. Inside gamescope (headless test runs) the menu is not seen.
- **Not done.** Gun outputs (recoil), Banapass cards.

#### Time Crisis 5

64-bit (`tc5.rs`, `usio.rs`): Unreal Engine 3, 2015, one player per PC (a cabinet side).
`RSLauncher.exe` (Lost Land Adventure's launcher: card reader, projector, credits, test menu)
starts `TC5\Binaries\Win64\TimeCrisisGame-Win64-Shipping.exe` (`-NOINI -Language=JPN
-voicelanguage=English(US)`, English texts). Both import `hasp_windows_x64_105293.dll`: the
profile's `payload_install` puts `wal_namcoes3.dll` in both places, that name selects the
Time Crisis 5 code. Unlike Lost Land Adventure the game, not the launcher, owns the I/O.

- **Dongle.** The dongle type (`[0x1861958]`) selects the HASP (0xd00 read as Lost Land
  Adventure's, template 0x16f4360 `2737212499**`: serial `273721249900`) or the USB key (the
  enumeration at 0x20b0, id 0x0c10, template 0x16f4350 `27371*2400**` or the script's at
  0x16f4330). Both are answered.
- **Launcher.** Banapass reader on COM4 (`bpreader.rs`; absent: 07-01 / 07-02) and NEC
  projector on COM1 (`projector.rs`). Its error page is a full-screen topmost window; behind it
  the game window was minimized, and UE3 saved that size (160x31, clamped to 160x120) as
  `ResX` / `ResY` in `My Games\UnrealEngine3\TimeCrisisGame\Config\TimeCrisisSystemSettings.ini`,
  read back at every boot (`-NOINI` does not apply to it): a 160x120 game. The status overlay
  is handled as Lost Land Adventure's. Network errors 05-20 / 05-34 (ALL.Net auth) stay on
  the test menu; the game plays.
- **Window.** The game's own window is 960x540 with a caption (`Fullscreen=false`). The
  launcher payload sets `WAL_WINDOW_POPUP=1` and `WAL_WINDOW_SIZE=screen` in its environment
  after its own window setup: the game it starts inherits them (a borderless window the size of
  the screen, minimize requests dropped), the launcher's windows keep their sizes.
- **I/O board: USIO on USB (`usio.rs`).** The game links the USIO library statically
  (0x140001c00-0x140003800): SetupDi enumeration of the interface
  {E54BF304-66A3-476F-A456-C6DDAB8C7078} (bananatools' `usio_guid`), `CreateFileW`, WinUSB (one
  bulk IN, one bulk OUT pipe). The board's memory is read with `10 00 addr16 len16` then `len`
  bytes, written with `90 xx addr16 len16`, the data, a 1-byte answer. The library's thread
  reads 0x0000 (0x100 bytes: +0x02 status, 0 = no error, else backup / firmware `IRM1` /
  memory errors, the last two hang; +0x28 the game's `81 73` marker), 0x1080 (switches dword,
  0x1090 / 0x1092 coin and service counters, 14 bits), 0x1800 once, and writes the outputs at
  0x1420, `99 99` at 0x0008 (watchdog), 0x220 bytes at 0x2000 at boot. The payload answers
  these calls on the executable's imports. Switches (the operator menu's switch test): test
  0x80, enter 0x200, right pedal 0x400, left pedal 0x800, down 0x1000, up 0x2000, service
  0x4000, gun button 0x800000. No start button: a pedal starts.
- **Gun board.** A serial device the game looks for on COM1, COM2, COM15..COM3 (38400 bauds,
  0x140a54c30), one command byte, a fixed-size answer whose last byte is the complement of the
  sum of the others (table 0x16f2f80): `08` 10 bytes (x, y, two more 16-bit values big endian,
  flags: 0x20 trigger, 0x80 out of screen, 0x10), polled every frame; `1C` 10-bit x / y; `14`,
  `18`, `0C`, `04` one byte; `10` a 31-byte version string starting with `R`. The payload
  answers on COM1. With the default calibration (`testmode.xml`'s `varGunAdjust...`) the gun
  test puts raw (x, y) / 1023 (y up) on the screen at `1.148 x - 0.080`, `1.163 - 1.268 y`
  (measured, residuals under 2%); the payload inverts it.
- **Input filter.** XInput / DirectInput pads, keyboard and mouse hidden from the game (the PC
  build's mouse binds `Fire`).
- **Saved resolution.** UE3 keeps the window's last size as `ResX` / `ResY` in the prefix's
  `My Games\UnrealEngine3\TimeCrisisGame\Config\TimeCrisisSystemSettings.ini`: the game
  payload sets them to the screen size at each start (a minimized window saved 160x31).
- **Banapass (`WAL_NAMCOES3_BANAPASS=1`, the Options tab, off by default: not finished).** The
  launcher's AMCUS `IAuth` is answered in-process (`amcus.rs`, the older layout: slots 6,
  4, 5, 14, 9, 13; identity SBVE / TC51 1.00, the HASP serial). The launcher puts `https://`
  before the ALL.Net uri itself (the answer has none). Its game server is `spp.net` (protocol
  buffers schemas in the launcher, `namcoes3srv/src/spp.rs`, run in the launcher process): `POST`
  of a raw `spp.net.Request`, the answer's body by its `Content-Length`; any certificate is
  accepted. REGISTER_PCB must answer the status INITIALIZED (0) or REPAIR: with NORMAL the
  launcher ends in its "PCB invalid" state (22-02, 05-20, 05-56). Online, the card manager
  polls the reader (game card mode `+0x4f0` 1, the touch sets 2) and reads the card (access
  code, type 0x101). The game's script then never calls `TCArcadeLoadTouchedPlayerData` (mode
  5, the launcher's LOAD_USER): the touch only shows an empty message. To find in the script
  (LZO-compressed `TimeCrisisGame.u`). Off: offline, 05-20 / 05-34 on the test menu.
- **Not done.** Outputs (gun recoil, LEDs at 0x1420), Banapass cards (above), the cabinet link
  (2-player co-op: two PCs).

#### Tekken 7

64-bit (`tekken7.rs`): Unreal Engine 4, 2015, v1.12 (`TekkenGame\Binaries\Win64\
TekkenGame-Win64-Shipping_612.exe`, PE time stamp 0x566C3FBD, the dump's `.windowsloader`).
One player per cabinet (versus is two linked cabinets; the switch test shows P1 only). No
launcher, no dongle: the game imports no cabinet library, so the payload is installed as its
`xinput1_3.dll` (`payload_install`, `xinput1_3=n,b`). The game imports ordinals 2 / 3 of it,
which the loader binds to the payload's HASP exports of the same ordinals; `game_pads_hidden`
rewrites those two entries before the game starts (neutral pads).

- **Boot patches** (applied when the bytes match): the online check at 0x147380 returns 1
  (offline); the pushes to the boot error list at 0x16bc41 and 0x179050 (`add qword [rbx+8],
  4`) are removed. Without them the boot stops on 2-1 FIRST POWER ON, 21-4 FIRST CHECK ERROR,
  22-2 NETWORK ERROR 2, 7-1 CARD R/W DISCONNECT, 19-1 SYSTEM ERROR, 3-1 I/O ERROR 1.
- **I/O board.** The Namco JVS board on COM3 (`jvs.rs`; its Namco command `70 18` must be
  answered, otherwise the game reopens the port and the boot hangs at 16%). Poll `20 02 02
  21 01 22 06 32 02 00 00`. Switches of a player (the operator menu's switch test): byte 0
  start 0x80, service 0x40, up 0x20, down 0x10, left 0x08, right 0x04, left punch 0x02, right
  punch 0x01; byte 1 left kick 0x40, right kick 0x20 (0x80 is none). The operator menu: up /
  down, enter with button 1.
- **Online, Banapass cards (`WAL_NAMCOES3_BANAPASS=1`, default; the Options tab).** The
  "offline" function answers 0 (it is `authtype == "OFFLINE"`: Fated Retribution's dump sets
  it in `tekken.ini`). The game then creates the AMCUS auth daemon (`IAuth`, `amcus.rs`
  in-process, ALL.Net identity SDBS, `TE76` 1.00 / `TER1` 1.50, the dongle serial
  282814450001): Tekken 7's own layout (`OBJECT_TEKKEN`: bananatools' slots minus 2, state 184
  bytes waited on until +176 is 14, cabinet config 152 bytes without the link status), Fated
  Retribution's the newer one (`OBJECT_NEW`). The ALL.Net answer's uri is the game server
  (`https://127.0.0.1:9002`), the Banapass reader is on COM4 (`bpreader.rs`, the `card`
  button, F4; the card made once in the dump's `Saved\banapass.txt`).
  `WAL_NAMCOES3_AUTO_CARD=1` (default): the card is on the reader a second after start, for
  8 s (`bpreader::card_on_start`).
- **Game server ("graphene", `namcoes3srv/src/graphene.rs`).** HTTPS (the game accepts the
  self-signed certificate), `POST /` of a `graphene.protobuf.Request` (1 the `MessageType`, the
  request in the field of the type's name in lower case), answered with a `Response` (1 the
  type, 2 the result: 1 success, anything else is retried every 3 s). The messages are proto2,
  all fields required, numbered differently in each build: the server reads the game's own
  schema (the `graphene.proto` file descriptor in the executable, found in its image by the
  payload) and encodes the answers by field name, the fields not set at their defaults. Boot:
  RegisterPcb, SaveAds, LoadClientSettings, CheckDiagnosis (v1.12), Ping every minute (Fated
  Retribution reads the card only after a Ping answering `acid_status` / `bnid_status` 1). The
  card: LoadUser (a player made at the card's first use, `new_card`), LoadChar per fighter;
  after each fight SaveCResult (CPU) / SaveMResult (versus) and SaveChar (rank, rank points,
  wins / losses, fight money as a signed delta, treasure gauge). The players in the dump's
  `Saved\graphene\users\<bp_amid>`.
- **Game log (`WAL_NAMCOES3_GAMELOG=1`).** The game's printf-like debug log (0x223aa0 /
  0x3319f0, an empty function in these builds) to the payload log: the boot sequence's
  `get...SeqError()=<n>` results (1: done), menu texts.
- **Language.** Tekken 7's menus are Japanese whatever its `tekken.ini` `Language` (`us` read,
  the texts unchanged); Fated Retribution's are English.
- **Saves.** Settings, credits and bookkeeping in the developers' path `C:\perforce\tk7\master\
  ue4\TekkenGame\Saved\Savedatas`, mapped to the dump's `Saved` folder (`c_folders`, made in
  the dump at the first start).
- **Resolution.** The dump's `DefaultGameUserSettings.ini` asks 1280x720: the profile passes
  `-ResX=1920 -ResY=1080`.
- **Input filter.** Keyboard, raw input and the game's `PeekMessageW` filtered (developer
  controls). Unreal Engine's usage reports (`wininet`, etsource.epicgames.com) get no
  connection.

Tekken 7 Fated Retribution (2016, `TekkenGame-Win64-Shipping.exe`, PE time stamp 0x573CBE0E):
the same code (`tekken7.rs`'s build table: online check 0x204af0, error list 0x25e791 /
0x268e00), the same board and switches, 1920x1080 from its own settings. It saves in Tekken
7's folder (`C:\perforce\tk7\master\ue4\TekkenGame\Saved\Savedatas`, `*_75.bin`): the same
`c_folders` entry, to its own dump's `Saved`.

Tekken 7 Fated Retribution Round 2 (2019, PE time stamp 0x5CDB8E5A): the dump's
`TekkenGame-Win64-Shipping.exe` is a patched copy (a jump at the end of `.text` loading another
I/O emulator), its `.windowsloader` runs the untouched `TekkenGame-Win64-Shipping.exe.orig`.
The same code again (online check 0x1ed080, error list 0x244e31 / 0x24d8c0, game log
0x32d470), ALL.Net game code `TR21` 1.00, the newer IAuth. Its game client keeps only the
ALL.Net uri's host (HTTPS on port 443, Poco `ConnectionRefusedException` every 3 s
otherwise): the payload's `connect` (ws2_32 #4) sends the game server host's port 443 to the
server's port. Saves `*_V2.bin` in the same folder, to its dump's `Saved`.
Insignias (Fated Retribution and Round 2, "kenkyu"): the mode select's INSIGNIA RATE BOOST
is offered only to a member, LoadUser's `user_status` non-zero (the menu's set-up copies it
from the answer, 0: the panel greyed out); the server sends it to the builds whose protocol has
SetKenkyuBoost. LoadClientSettings sets the insignia event (`kenkyu_*`, its end a year ahead),
LoadUser the boost's end and the day's free boost (`kenkyu_boost_free_count`, "First try is
free"), SetKenkyuBoost / OsBncoinConsume start a one-hour boost, GetKenkyu adds the insignias
won.

#### Pokkén Tournament

64-bit (`pokken.rs`): Direct3D 11, 2015, `ferrum_app.exe`: update 24.05 (PE time stamp
0x58DDBEF3, online) and 26.02 (0x5C18DFCF, the last one, made offline when the network
service ended); per build the RVAs of the patches below. The cabinet's `init.ps1` starts it
with `fullscreen serverenv=production verboseLevel=0` (the profile's `args`; without
`fullscreen` a window). No launcher, no dongle library: as Tekken 7, the payload is the
game's `xinput1_3.dll` (`payload_install`, `xinput1_3=n,b`), whose ordinals 2 / 3 / 4 the
game imports (bound to HASP exports, rewritten in the import table at attach). The dump's
`dinput8.dll` is another loader's force feedback plugin: hidden.

- **Controller.** The cabinet's Pokkén controller is an XInput pad (USB 0f0d:0073): the game
  reads XInput pad 0 (buttons, triggers, both sticks, subtype from the capabilities) and lists
  the USB controllers (`driverUtil.dll` `driverUtilGetControllerUsbIdPairs`), stopping on any
  other id ("usb controller error"): the payload answers that pair, and the pad from player
  1's virtual stick (b1-b4 X A B Y, b5 / b6 L / R, b7 / b8 ZL / ZR on the triggers, start,
  the d-pad, both sticks). DirectInput: no device (`WAL_DINPUT_DISABLE`).
- **I/O board.** The Namco JVS board on COM3 through `wajvio.dll` (`jvs.rs`, `Game::Pokken`):
  test, service, coins, start; poll `20 01 03 21 01 22 06 32 03 00 00 00` plus `70 18`. The
  test menu (English): up / down (0x20 / 0x10) or service (0x40) choose, enter (0x02) enters:
  start while the test switch is on (the pad's A does nothing there). FREE PLAY is in COIN
  OPTIONS.
- **Boot checks.** The Namco USB dongle enumeration at 0x30b1d0 (the Battle Pod library: a
  0x628-byte block) answers the "USB DONGLE" 0b9a:0c10 (id at +0x22) with the serial
  274713190001 at +0x428 (2747, region 1, role 3, cabinet 19: parsed by the boot). The boot's
  serial step compares its first 8 digits with `F:\WritableConfig.ini` `[RuntimeConfig]`
  `serialID` (the auth daemon's file; written by the payload with `netID` ABGN3190001, as
  ARTEMiS derives it): "19-1 SYSTEM ERROR 1" otherwise. The system check wants Windows
  Embedded Standard 7 SP1 (`GetVersionExA` / `RtlGetVersion` through `GetProcAddress`: 6.1,
  workstation, `VER_SUITE_EMBEDDEDNT`) and the Windows account `arcade` (`GetUserNameA`):
  "19-12 SYSTEM ERROR 12" otherwise. The link check (`GetIfTable2`) wants a hardware interface
  with its cable connected: Wine marks none, the up interfaces but the loopback are ("4-1 LAN
  ERROR 1" otherwise). The gateway ping (0x5f0d30) and the store subnet check (192.168.123.x,
  0x5f1980) succeed; `BlockInput` does nothing.
- **Language.** The game picks its language index from `GetUserDefaultLCID` (Japanese 0,
  English 1, ... Spanish 5: the file suffix of table 0x984940, and the column of its
  multi-language texts) and its region from `GetSystemDefaultUILanguage` / `GetUserGeoID`. It
  carries only the Japanese files; on an English locale the navigator texts are Spanish: the
  three answer Japan (ja-JP 0x0411, GEOID 122). The test menu is English anyway.
- **Saves.** The operator settings and bookkeeping in the dump's `Win32\save_data`
  (`nbamsavdat.dll`); F: / G: / J: (logs) are drives in the dump. Without network the screen
  corner shows 4-1 LAN ERROR 1 (the in-store link), 2-1 FIRST POWER ON at the first start.
- **Movies.** The attract and stage movies (`Win32\movie\master\*.mp4`, H.264 + AAC) go
  through the Media Foundation source reader on the game's own byte stream, read
  synchronously (`ReadSample` of any stream). Two GE-Proton bugs froze them, fixed by patches
  of the runner (§ wine runner): mfreadwrite handed out the stream that was ahead, so the
  game's small video frame pool filled while the audio it plays them on was still to be read
  (patch 0110: the stream that is behind, as Windows); mfsrcsnk read the byte stream holding
  the source lock, while the game's byte stream completes its reads through
  `MFInvokeCallback` on the one-thread standard queue, whose thread waited for that lock
  (patch 0111: demuxing without the source lock): a still title screen, stopping a movie hung.
- **Banapassport cards (24.05).** In process, as the cabinet's AMAuthd: the AMCUS object
  (`amcus.rs`, `OBJECT_NEW` through a `CoCreateInstance` hook; game SDAK / PKF1, the ALL.Net
  uri path `/pokken/`, the reinit's state 0 at `Finalize`), the card reader on COM4
  (`bpreader.rs`; with `WAL_NAMCOES3_AUTO_CARD` the card goes on the reader at the game's
  polls after start, once per game: start also dismisses its messages), the game server
  `namcoes3srv/src/jackal.rs` on 127.1.1.1:9002 (the game `strlen`s the `gethostbyname`
  address: no 0 byte; ARTEMiS's `titles/pokken`: jackal.protobuf through the graphene
  dispatch, the matching JSON and the admission websocket for the online battles against the
  CPU). The game's server library speaks TLS 1.1 (axTLS): its `https://` checks answer "not
  https" (plain HTTP, the server tells by the first byte, `Connection: close`). A new card is
  registered by the server with the profile's name (`WAL_NAMCOES3_PROFILE_NAME`): the game's
  registration asks for the name on a touch keyboard. The card is saved once per credit
  (SaveUser after the last battle), players in the dump's `F\jackal`. The guest's name
  (hard-coded Japanese) is patched to `Player (No Card)`. 26.02 starts no network.
- **Text.** The game loads its texts from `Win32\text\master\resident_ntx_ja.drp` (and the
  navigator's `NAVI_nnn_ja.drp`): DRP archives (encrypted big-endian, zlib), NTXL string
  tables, labels looked up from the code and the layouts (`resident_tlt.drp`). The dumps
  carry a Spanish fan translation; 24.05's resident archive is Pokkén Tournament DX's tables
  (move lists, ...), without the arcade's Card / System / Tips and many labels: empty
  message boxes. `wal-pokken-text` (`crates/pokken-text`) exports the strings and rebuilds
  the archives from a translation memory; the English one is installed in the dumps (the
  originals as `*.orig`), 24.05 with the arcade resident archive of 26.02. The game takes 32
  tables at most in that archive (more: nothing drawn).
- **Closing time.** The test menu's CLOSE TIME SETTING (the shop's): the game warns
  ("N min. until coins can no longer be inserted") and stops taking coins.

#### JoJo's Bizarre Adventure: Last Survivor

64-bit (`jojo.rs`): Unreal Engine 4.20, Direct3D 11, 2019, `AJB-Win64-Shipping.exe`, update
28.02 (`JJL128-1-NA-MPR0-F02`, the dump `JJL10JPN-28`; the PC port of github.com/Aeyth8/AJB
runs the same exe). The payload is the game's `xinput1_3.dll` (`payload_install`,
`xinput1_3=n,b`), as Tekken 7. The game logs to stdout (the launcher's output). Its system
check (serial, I/O board, card reader, backup memory, touch panel, ALL.Net, store network)
passes in full; the game server is not there: national and store battles are off ("network
offline", the test menu's 5-24 / 5-1), the game plays as a guest.

- **Cabinet serial.** `nbamUsbFinder_GetInformation` answers the USB key (`patches::usb_key`)
  with `282713030001`: the game wants model 2827, region 1, type 3-5 and code 03 (found by
  logging its wide string compares). AMCUS (in-process IAuth, `amcus.rs`) gives the matching
  NetID `ABLN3030001` (`ABLN` and the serial's last 7 digits), game id `SDEX` / `JJL1`.
- **AMActivator.** `amactivator.dll`'s imports answered in-process (bananatools'
  `amactivator` stubs, public domain): not busy, statuses ok, generation 1.
- **I/O board.** `winusio.dll` (`CWinUsio`, ANSI SetupDi / `CreateFileA`) on the emulated
  USIO board (`usio.rs`): firmware 126 at board memory 0x12 (the game wants 120-126), the
  board name `NBGI.REAL_USIO` at 0x1800, the switches (test 0x80, service 0x4000, start,
  directions, 11 push buttons) and the analog inputs at 0x10a0 (both sticks; the d-pad drives
  the left one).
- **Card reader.** Sega's `libamw.dll`: its NVRAM `D:\sysfile.dat` (the dump's `D` drive),
  the Aime NFC reader 837-15396 on COM1 (`sgreader.rs`, from bananatools' `sg-*.c`: frames,
  firmware, LED board; no card) and the payment display (VFD) on COM2 (`vfd.rs`, the version
  answer only): the card reader row is NG until the display answers.
- **Touch panel.** The game asks `GetSystemMetrics(SM_TABLETPC)`: 1. The touches are the
  mouse's (or a gun's) left button where the cursor is (`kp0`), posted as mouse messages, a
  red dot where it touches (`WAL_CROSSHAIR`).
- **Store network.** The cabinet sets itself to 192.168.171.2 with `netsh`: replaced by
  `cmd /c exit 0`; from then on `GetAdaptersInfo` / `GetAdaptersAddresses` report that
  address and `IcmpSendEcho` to it replies (before, silence: an early reply sends the game to
  the store update server's mode). IAuth reports the cabinet links ok (the statuses of
  `GetCabinetConfig`) and settles in its idle state 15 (`settle_idle`), which the game waits
  for.
- **English.** The game's texts are keyed `FText`s and string table entries, looked up in
  the current culture's `Content/Localization/Game/<culture>/Game.locres`: the US locale
  (en-US) loads the dump's `en` one, no asset is changed. `wal-jojo-text`
  (`crates/jojo-text`) exports the Japanese texts to a translation memory and writes the
  resource (version 1, `FCrc::StrCrc32` source hashes); titles spelled one text block per
  character are found in the widget trees (a panel's slots, in order) and the English shared
  out over the blocks. The PC port's fan patch (`AJB-ENG-PATCH-ALEXIOR.pak`, 39 replaced
  assets) was adopted into the memory (`adopt`): a pak can't sit beside the loose files
  (Shipping refuses loose `.uasset`s once a pak is mounted), and its English widgets clash
  with the Japanese copies of the same keys. The memory and the fan patch are kept in the
  dump's `translation`. Still Japanese: the catchphrases and pose names (plain strings of the
  `m_words` / `m_pose` data tables) and the text drawn in textures.
- **Game server (unfinished, off: `WAL_JOJO_ONLINE=1`).** `wal_namcoes3srv::jojo` on the
  ALL.Net uri: HTTPS POST `<api>` (`sys/serveralive`, `user/login`, ...), a message is the MD5
  of its MessagePack and a hash key (`AJB `) then the MessagePack: a request `[header map,
  parameters]`, an answer `[{result, session}, [result, values...]]` read by position (the
  game's `Response_<Api>`). libcurl checks the certificate: its SAN `127.0.0.1`
  (`tls_config`), `n.VerifyPeer=0` in the saved `Engine.ini`. The player's session comes from
  the answer's header map; a guest (`user/create`) gets none and plays offline (the game's
  `IsOfflineMode`). The Aime reader then holds a card (after a coin, until read: MIFARE
  blocks, `F:\aime.txt`): "play with this card?", the online mode select (national solo with
  `sys/national_match_info`). Not done: the sync server the login names (`UNetSocket`: a
  54-byte header, AES-128-CBC MessagePack, key `0123456789012345`): the game's first message
  (protocol 20000) gets no answer it takes, and it leaves every ~26 s ("disconnected from the
  game server"), even in training. National and store battles also need dedicated game
  servers (`UAJBInGameServerInfo`).

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
  Only the variables compared with 1 (`cmp level, 1; jl`) are levels: amLib's init flags are
  compared with 0 before some log calls, and setting them skips the init (Let's Go Island:
  no SRAM, a division by zero in amSramRead).
- **Devices (`devices.rs`).** `\\.\mxsram` is the save folder's `SRAM.bin` (2 MB; version
  IOCTL `0x9C406000`, size by `IOCTL_DISK_GET_DRIVE_GEOMETRY`, then plain reads and writes).
  The SMBus driver (device interface class `5c49e1fe-...`, SetupDi faked) carries the EEPROM
  (I2C 0x57, `E2PROM.bin`, 32-byte block transfers `0x9C40200C`) and the PCA9535 I/O expander
  (0x20, byte transfers `0x9C402004`): its port 1 holds the main board's TEST (0x04) and
  SERVICE (0x08) push switches, active low, which the test menu reads (the game reads the
  JVS board's); port 0 the DIP switches, also active low, where games read their monitor
  resolution (`WAL_RINGEDGE_DIPSW`, default `FF` all on: 1920x1080 for Virtua Tennis 4, Koihime
  Enbu, Under Night; Ge-Sen Love Plus Pengo! refuses it, "ERROR 0910 Wrong Resolution
  Setting": `8F`, 1280x720). Bit n-1 = switch n; switch 4 = vertical monitor, switches 5-7 the
  resolution (Dream Raiders manual: 5 + 2*6 + 4*7 on = 1 640x480, 2 1024x600, 3 1024x768,
  4 1280x720, 5 1280x1024, 6 1360x768, 7 1920x1080, 0 the game's default). Records the system menu makes, absent from new `E2PROM.bin` /
  `SRAM.bin` files (written by the payload when the profile asks; CRC-32 of bytes 4 on, then
  the data, a copy at +0x200 / +0x1000): the EEPROM's static record (record 0, 32 bytes, the
  main board's region at +12: `WAL_RINGEDGE_REGION=<hex>`; Dream Raiders wants 02, 04 or 08
  matching the keychip's region, else error 0903 "wrong region") and the SRAM's credit
  settings (record 7, 512 bytes: `WAL_RINGEDGE_CREDIT_RECORD=1`, all zero; Dream Raiders
  without it: error 0025 "main board malfunctioning", whose screen then spins on the
  uninitialized amCredit, a black "not responding" window). `\\.\columba` reads physical memory: amPlatform scans 0xF0000 for the
  `_DMI_` entry point, reads 64 KB of SMBIOS structures (the table must start at or below
  0xF0000), and takes the system manufacturer (`NEC`) and the third OEM string as the
  platform id (`AAL` RingEdge). `\\.\mxsuperio` (hardware monitor, Winbond-like): chip id
  registers 0x20/0x21 = `A0 20` (`0x9C402004`), the monitor register file read with
  `0x9C40200C` (0x48 = 0x2D) and written with `0x9C40A010`; most games go on without it
  (amHm "not found"), Transformers: Human Alliance stops on a missing status object. Blank SRAM and EEPROM are formatted by amLib itself.
- **PCP services (`pcp.rs`).** mxmaster on TCP 40100, mxkeychip on 40106 (its binary channel
  40107 unused so far); for test menus also mxinstaller 40102, mxgdeliver 40108, mxgcatcher
  40110 and mxstorage 40114 (its binary channel 40115; Let's Go Safari's amStorage waits a
  minute on a missing one, then error 1001 "Invalid number of storages for oneself"), answering `request=<name>` with `response=<name>&result=success`
  (with no service, amLib waits for the connection forever); the installer's
  `query_appdata_status` also gives `status=available&id=<game id>` (without them amInstall
  fails: Operation G.H.O.S.T.'s error 3005 on each boot but its first). Text protocol: prompt `>`, request `key=?&cache=1\r\n`, answer
  `key=value\r\n`. Keychip values are text (game id `SBWX`, billing ids) or hex bytes
  (region, flags). `mxmaster.foreground.next=<n>`: the game hands over to another
  application and exits (test button: 3, the test menu = the same executable with `-t`): the
  payload asks the launcher to relaunch with that application's arguments
  (`WAL_RINGEDGE_FOREGROUND`, default `3=-t`; `3=exe=<file> -t`: a separate test menu
  program, which the launcher starts instead of the game). `WAL_RINGEDGE_END_ON_NEXT=1`: a game
  still running 2 s after the request is ended (Teratoma; not by default: Medal no Gunman asks
  for application 0, itself, at boot).
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
  then found in the executable's image, its MACs or SSD answer right after it. A dump without
  its challenge file (SEGA Golden Gun: empty `fs\table`; amDongleSetAuthConfig fails, error
  0949) gets a generated one (random records, same size) in the save folder, opened in place
  of the missing path (`CreateFileA/W` hooks): amDongle only compares the keychip's answers
  with the file's records, and the keychip answers from the same file.
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

### 6.3e Sega Amusements Shell (`crates/payload-segashell`)

Sega Amusements' Windows PC cabinets (Transformers: Shadows Rising, 2018): the cabinet runs
Sega's `Shell.exe` (coins, credits, settings, bookkeeping, the test menu) and the game beside
it. The dump keeps the cabinet's `C:\sega` tree (`Sega/Transformers2/Transformers2.exe`,
`Sega/ShellData/*.ini` the Shell's settings the game reads, `../ShellData/` from its folder);
the game ships its `.map`/`.pdb`, so every address below has a name. System `segashell`,
payload `wal_segashell.dll` loaded by `wal-loader`.

- **Shell service (`shell.rs`).** The game connects to 127.0.0.1:18888 (`CShellMgr`). Packets:
  `TFM`, command id and data length (2 bytes each, big endian), 4 zero bytes, the data. Game
  commands 0x00-0x1c (`GAME_MSG_KEEPALIVE`, `LAUNCHED_OK`, `ASSETS_LOADED`, `ATTRACT_*`,
  `START_PRESSED` (player), `GAME_START`, `PLAYERS_IN_GAME`, `STAGE_*`, `TRIGGER_RECOIL`,
  `GAME_END`..., `SECURITY_CHALLENGE`) plus 0x1d (lamps, several times a second); Shell
  commands 0x40 `SET_SKU`, 0x41 `SETCREDIT` (data: the credit count), 0x42
  `SECURITY_AUTHENTICATE`, 0x43 `RESET`, 0x44 `EXIT`. The payload logs the game's events and
  owns the credits (a coin press adds one, `SETCREDIT`); the dump's settings are free play.
  Security: the game sends a random 4-byte challenge every few seconds and quits after 5
  unanswered; the answer is a byte-scrambled value the game checks with
  `CShellMgr::AuthenticateData`. A profile patch clears the constructor's "security enabled"
  flag instead.
- **I/O board (`board.rs`).** The "SAE board" is a USB game controller the game reads through
  DirectInput (the first joystick, `DIJOYSTATE`, axes set to -1000..1000) into a record:
  6 buttons in two alternating sets (current / previous, for presses: P1 start 0, P2 start 1,
  P1 trigger 2, P2 trigger 3, lever 4 front / 5 back), the guns 0-255 (`(1000 - axis) * 255 /
  1000`: reversed). `WAL_SEGASHELL_BOARD=<rva>` replaces `CSAEBoard::Update` (thiscall) with
  one filling the record from the virtual sticks (both axes reversed); a patch makes
  `CSAEBoard::IsActive` true (the lever is only read on an active board). Reload is aiming off
  screen. The Shell's gun calibration (`ShellData.ini` `Xmin`/`Xmax`...) is not a range of
  the raw values: the full 0-255 spans the screen.
- **Display.** The resolution comes from `SetInitialResolution(argc, argv)`, whose command line
  names (`-hdtv`...) are overwritten by `DefaultResolution()` (index 15, 1920x1080, when a check
  passes, else 14, 1280x720). `WAL_SEGASHELL_DEFAULT_RESOLUTION=<rva>` makes it return 14
  unless `WAL_SEGASHELL_FULLHD=1` (game option): integrated GPUs drop below 60 frames per
  second in stages at 1080p. The game paces itself (`CFrameRateMgr`: a 300 Hz timer, frames a
  whole number of ticks). Vsync stays on, with the launcher's 60 fps limit: without vsync it
  ran too fast on a Batocera machine at 60 frames per second all the same; at 1080p an
  integrated GPU missed refreshes with vsync (~22 frames per second), 720p holds 60.
  At start it builds its 594 standard shaders a batch per frame ("SYSTEM INITIALIZING...
  LOADING", about a minute).
- **Test menu (`testmenu.rs`).** The game has none (its test mode entry is never called): the
  cabinet's test menu is the Shell's (`Sega/shell/Shell.exe`, 32-bit, Direct3D 9, runs under
  Wine). TEST (F2) in the game asks the launcher to start `WAL_SEGASHELL_TEST_MENU`
  (`../shell/Shell.exe`, relaunch `exe=`) and ends the game; the payload, loaded by
  `wal-loader` in the Shell too, recognizes it by its file name and then only:
  sets the working directory to the Shell's folder, applies `WAL_SEGASHELL_SHELL_PATCHES`
  (`WAL_PATCHES_EXE=Transformers2.exe` keeps the game's patches out), writes the inputs and
  sends the test menu's exit back to the game. The dump's Shell is already patched past its
  security check (state 2) and I/O board wait (state 3). The Shell's main state machine
  (state at 0x5f034c, 34-entry jump table at 0x40d190): state 1 (init) returns 2; the
  profile makes it return 0x21, the test menu (0x428ff0), so the Shell skips its updates,
  Dinkey dongle (DK2WIN32, absent: "NO SECURITY KEY" in its error log, harmless) and game
  launch. Leaving the test menu goes to state 28 (launch the game, call at 0x40d0fd) or 2
  (restart, call at 0x40cfad): `WAL_SEGASHELL_SHELL_EXIT` retargets both calls to the
  payload's return to the game (relaunch with the profile's arguments).
  Inputs: the Shell reads its HID I/O board only once it is found (never here), into a
  15-byte report at 0x13fd030 (byte 1/2 P1 gun Y/X, 3/4 P2 gun Y/X, 0 = left / top; 6/7
  buttons 1-16, bit `n - 1`), turned into its button bitmap at 0x1404980 (button `n` = bit
  `n`; also set by its keyboard: 1-8, Q-I). `WAL_SEGASHELL_SHELL_INPUT=<report rva>,<bitmap
  rva>`: a payload thread writes both every 4 ms. Buttons: 1/2 starts, 3/4 triggers, 5/6
  lever down/up, 13 TEST, 14 SERVICE (`Config.ini` `TEST=13`, `SERVICE=14`). SERVICE moves
  the cursor, TEST selects; a TEST still held from the game's press is ignored until
  released. Its data folders are `game.ini` `[ShellData]` settings whose defaults
  (`c:\Sega\ShellData`, `c:\Sega\GameData`) are patched to `..\ShellData` / `..\GameData`:
  the settings and the gun calibration go to the dump's `ShellData/*.ini`, read by the game
  at start (`CConfigMgr::LoadShellDataAdjust`; `GetAdjstGunPos` clamps each 0-255 axis into
  [min, max] and stretches it to the screen: the dump's 100-200 used only the middle of the
  gun's travel). DEVICE CALIBRATION > CALIBRATION START: min/max widen as the guns move,
  TEST saves. The Shell writes ~50 log files a day in its folder: the profile's `files:`
  copies the dump's own `Sega/shell/Game.ini` into the run tree, which makes that folder a
  real directory there (logs in the prefix, not the dump).

**Showdown** (Codemasters / Sega Amusements, 2014; `Showdown/Showdown.exe`, Direct3D 11, the
Ego engine of DiRT Showdown; no symbols). An earlier Shell protocol and a PC game made arcade:

- **Shell paths.** It reads `c:\sega\Shell` (`Game.ini`) and `c:\sega\ShellData\*.ini`: the
  profile patches the four strings to `..\shell`, `..\ShellData\...` (the dump's folders).
  Its config manager validates `ShellData.ini` (`0x5aeff0`, each value against a maximum):
  the dump's `[Audio] AttractMusic=20` (a later Shell's volume; the game takes 0-3) fails it,
  no Shell manager, a crash at once. `settings.rs`: `WAL_SEGASHELL_INI=<section>/<key>=<value>,...`
  answers the game's `GetPrivateProfileIntA` / `StringA` (`Audio/AttractMusic=2`),
  `WAL_SEGASHELL_FREEPLAY` its `Credit/Freeplay`.
- **Shell protocol (`WAL_SEGASHELL_PROTOCOL=dsw`).** The same packets with the magic `DSW` and
  one numbering both ways (names at `0x594920`): game 0x01 ack, 0x03 `LAUNCHED_OK`, 0x04
  `ASSETS_LOADED`, 0x05 `CREDITUSED`, 0x06/0x07/0x08 game start / end / continue, 0x09
  security challenge, 0x0a event, 0x0b state (attract states every 5 seconds), 0x0c shutdown;
  Shell 0x0d `SETCREDIT` (data byte 0: the count), 0x0e `SECURITY_AUTHENTICATE`, 0x0f `EXIT`,
  0x10 `SET_SKU`, 0x11 keypad, bonus codes, 0x18 motion base, 0x19 QR code, 0x1a spoofed
  button. After `LAUNCHED_OK` the game waits 5 seconds for `SET_SKU` (handler `0x594640`, it
  sets the event the boot waits on), else it sends `GAMESHUTDOWN` and quits. `SET_SKU` data
  (16 bytes, `WAL_SEGASHELL_SKU` hex): 0 ?, 1 surround audio, 2 motion base, 3 seat belt, 4 age
  rating shown, 5 external screen, 6 Blue Ripple logo shown, 8-11 screen preset (u32: 0 legacy,
  1 standard, 2 super deluxe), 12-15 default view (u32: 0 bonnet, 1 chase). The payload sends
  it, then the credits, on `LAUNCHED_OK`.
- **Sound (`rapture3d.rs`).** OpenAL through Blue Ripple's Rapture3D (`OpenAL32.dll` router,
  `rapture3d_oal.dll`; MSVC 2010 runtime: `vcrun2010`). The dump's 2.6.5 crashes; the cabinet
  installs 2.7.4 (`Rapture3D audio installer/rapture3dgame_2.7.4_win.exe`, Inno Setup: its
  `sys/` DLLs, extracted with innoextract, go in the dump's `Showdown/`). Its settings
  (`HKCU\Software\BlueRippleSound`), set on the cabinet by `SetupAudioRegistry.bat`, are
  needed too (without them `BadDataException`s, then a crash): `WAL_SEGASHELL_RAPTURE3D=<format
  library>,<speakers .reg>` writes `Formats\UFL` (the game directory's `showdown.pfl`) and
  `Speakers\Configuration` (the layout of its `Stereo.reg`) at start.
- **I/O board.** A DirectInput game controller "SEGA IO Controller"
  (`win/actionmap/sega_controller.xml`: X wheel, Z brake, Rx gas, buttons 0 start, 1 view, 2
  boost, 3/4 music up/down, 5 reset car): the common fake wheel. The game polls `EnumDevices`
  every 2 seconds and keeps a device only when a raw input HID device of its vendor / product
  exists and is not an XInput pad (`0x961df0`): patched to "not an XInput pad". On a new
  device the force feedback processor's handler (`0xc0c8e0`) takes the processor's lock
  while the processor is gone (a freed lock, a crash): patched to its "no force feedback"
  exit; `WAL_DINPUT_WHEEL_FFB=0` also offers no DirectInput force feedback.
- **Force feedback.** `inpout32.dll`'s `Out32` (imported by ordinal 2; the dump's DLL needs its
  driver): the payload takes the import. Port 0x378, Ford Racing's encoding (`0xd2dd80`), the
  common `port_ffb` decoder (`WAL_SEGASHELL_FFB` percent).
- **PC inputs (`input.rs`, `WAL_SEGASHELL_PC_INPUT_HIDDEN=1`).** Keyboard, mouse and raw input
  messages dropped from its `PeekMessage`, raw input not registered, low level keyboard /
  mouse hooks not installed, XInput without pads.
- The game's own log: `OutputDebugString` (Wine `+debugstr`).

### 6.3f Wahlap (`crates/payload-wahlap`)

Wahlap Technology's Windows PC cabinets (Storm Racer G, 2013, with Sega; Storm Rider below): the game
(`bin/racing.exe`, a debug build) imports `usb_io.dll`, the driver of Wahlap's USB I/O board,
statically. System `wahlap`, payload `wal_wahlap.dll` installed as `usb_io.dll`.

- **Board (`usbio.rs`).** `UsbIO_InitSystem(0)` = 1 (board found; -3 would leave the game on
  the keyboard and XInput). `UsbIO_GetInputdata` fills `{u32 a, u32 b, u8 analog[4]}`; the game
  (`0x49f790`) makes its button word `a | (b & 0x18) << 2 | (b & 4) << 6 | (b & 1) << 9` and the
  analogs `value / 256` (wheel, gas, brake, the boost lever: bit 0x100 when under
  `UsbIO_SetBoostThreshold`). Bits from its `CInput::AddButtonPad` tables: 0x04 the coin switch,
  0x02 start (never set by the real board: start is the game's own Enter key, below), menus
  up 0x20 / down 0x40 / back 0x100, operator menu 0x100 + 0x20 held then released. The board's
  coin counter (`UsbIO_ReadSysCoins` / `DecCoin` / `CleanCoin`) is emulated but not credited by
  this build. Its memory calls are stubs in the real driver too (the game keeps files).
- **Inputs from the launcher only (`keys.rs`).** The game reads the PC keyboard from its
  window messages and pads through XInput: its `PeekMessageW` drops the real key messages and
  returns the key messages of the `key_*` natives (start = Enter), `XInputGetState` reports no
  pad.
- **Force feedback (`ffb.rs`).** `UsbIO_SetSteeringStatu(&{mode, 255, strength, a, b, 3})`: in a
  race the steering motor's centering strength, 0 at a standstill to ~750 at full speed (the
  test menu drives the motor with `x * 500 + 500`): the centering spring. `UsbIO_SetRoadType`
  (0 road, 1 grass, 2 rough ground, 3 rumble strip) with the speed of
  `UsbIO_OutputCarStatuData` (`{speed 0..1, gear, ..}` floats): a vibration off the road. No
  left/right force is sent by the game. `WAL_WAHLAP_FFB_SPRING` / `_ROAD`: strengths in percent.
- **Movies.** WMV9 AVIs read through AVIFile: `WMV9VCM.dll` (from `wmv9VCMsetup.exe`) in `bin/`,
  `WAL_VFW_CODECS`; `WAL_VFW_READ_FIX` works around two Wine avifil32 bugs on the audio stream
  (6.3 `vfw.rs`). Without the codec the first frame fails and the game crashes.
- **Debug build.** Its asserts wait for a console key forever under Wine (answered "continue",
  `0xc008b`); one fires at each start (34 strings compiled in, 155 in the assets' table). The
  dump's assets are newer than the exe (its `_crc_list` names a missing `racingGold.exe`): the
  race's checkpoint billboards (HUD objects `checkPoint`, `finishPoint`, `checkPole`,
  `checkPole2`) exist in no resource and their per-frame update (`0x41bb20`) crashed on the
  first race frame: it returns at once.
- **Storm Rider (`motobikeio.rs`).** `all_test/game/Gold/motobike.exe`, the unprotected build
  (`motobikeGold.exe`, the 1.2.6 build its PDB belongs to, is encrypted by its dongle envelope);
  working directory `assets`. Its board driver `motobikeio.dll` (used by the game, by
  `moto_coins.dll`: coins, bookkeeping, settings, and by `FirstMotoLightDLL.dll`: lamps) is the
  same payload under that name (the launcher installs `wal_wahlap.dll` as both). Getters over the
  board's HID state: `GetControlButton` (panel bits 0-6), `GetIoKeyData` / `GetIoKeyState` (the
  switch byte, as Storm Racer G's `b`), `GetADData(0..2)` (10-bit handlebar, throttle, brake),
  `GetCoin` / `DecCoin` / `ClearCoin`; the input wrapper (`0x433720`) builds the same button
  word. Bindings: menus up 0x01, down 0x02, left 0x08, right 0x20, enter 0x10 (`0x45e1f0`), race
  0x04 boost (`0x47efe0`). `MemoryRead` / `MemoryWrite(addr, len, buf, 0)`: the board memory
  (`moto_coins.dll`'s settings record at 0x20, logs), `motobikeio.mem` in `assets`; the
  settings hold each channel's calibration (u16 min, max at +8): a fresh board's 0, 0 made the
  handlebar dead, channels never calibrated read 0..1023. Dongle class `Lock::Initialize` /
  `Lock::test` answered true in the game's imports. Keyboard from `PeekMessageA` polling its own
  window (key messages injected for that window). Same WMV9 movies; its full-screen 1360x768
  device fails under Wine: windowed over the whole screen. No force feedback (lamps only).
- **Display.** Its 1360x768 window (`racing.cfg` `fullscreen = 0`) made a borderless
  screen-sized window, the picture stretched, 16:9 kept.

### 6.3g Sega Europa-R (`crates/payload-europar`)

Sega Amusements Europe's Windows PC cabinets (Ford Racing Full Blown, 2006; SEGA Rally 3, 2008; GRID, 2010): the game
(`FordRacing/fordracing.exe`, Direct3D 9, DirectInput and DirectSound loaded at run time) runs
beside the cabinet's GameShell (`GameShell/GameShell.exe`: coins, credits, operator menu,
updates). It imports `inpout32.dll` (the parallel port: the force feedback board) statically.
System `europar` (also `segaeuropa-r`), payload `wal_europar.dll` installed as `inpout32.dll`
(`Inp32` / `Out32` at ordinals 1 / 2).

- **I/O board (`happ.rs`).** A Happ UGCI (`USB\VID_078B&PID_0010`), a HID device with several
  top-level collections. The game (`0x4de1f0`, the "HappAPI" code the shell shares) enumerates
  the HID class with SetupAPI, opens each interface with `CreateFileA`, keeps vendor 0x078b
  product 0x0010 (`HidD_GetAttributes`) and names each collection from `HidP_GetCaps` (table
  `0x53c6b8`: usage page 1 / usage 4 "JOYSTICK", 0x91 / 2 "COIN DROP", 0x91 / 3 "WATCHDOG",
  1 / 0x80 "SYSTEM", ...). Its thread `0x4de030` reads JOYSTICK with `ReadFile`, 8-byte reports:
  byte 1 wheel, 3 gas, 5 brake (0..255, `data/calibration.ini`), 6 buttons (0x01 shift up,
  0x02 shift down, 0x04 start, 0x08 view change; the shell's table at `0x431628` adds 0x10 TEST
  and 0x20 SERVICE; a report whose byte 6 is 7 is dropped). All of it answered by IAT hooks,
  one report per 8 ms read. COIN DROP (4 bytes, bytes 1-2 a coin counter) is read by the
  shell only.
- **GameShell (`shell.rs`).** The shell starts the game with `/h=<window>`; without it the
  game looks for the window "Game Shell - Windowed" (`0x467ba0`), and without one gives itself
  99 credits and free play. They talk with `WM_COPYDATA` (`dwData` the message): game -> shell
  (`0x467720`, its sender thread) 8 its window, 4 a credit spent, 1 acknowledge, 5/6/7/10/11/12
  game states; shell -> game (`0x467a20`) 3 the credit count (a higher one plays the coin
  sound), 9 quit, 13 a coin sound. The payload's hidden window (created when the game looks
  for it) owns the credits: a coin adds one, a game started takes one. The game reads free
  play itself (`[gamesettings] FreePlay`) but starts only with credits: in free play the shell
  gives 99 and never takes them. `WAL_EUROPAR_FREEPLAY` overrides the setting.
- **Force feedback (payload-common `port_ffb.rs`, shared with Showdown).** `0x467990` scales the steering force (-1..1) by the operator's
  strength (`[ForceFeedback] Strength`, 0..31 / 15) to -15..15 and writes one byte to the data
  port 0x378: 0 none, 1..15 positive (1 strongest), 16..30 negative (16 strongest). In a race it
  writes twice per frame, an opposite force then the one that stays (`0x4a20c0`, `0x4a2020`):
  the port is sampled every 4 ms into a constant force (positive: to the right),
  `WAL_EUROPAR_FFB` percent.
- **Settings (`settings.rs`).** `WAL_EUROPAR_RESOLUTION` answers its `Resolution` read
  (`0x466ac5`: 640x480, 800x600, 1024x768, 1280x1024, else 1600x1200), full screen, scaled
  by Wine.
- **Input.** DirectInput (keyboard, joysticks) is the common fake without devices
  (`WAL_DINPUT_DISABLE`, which also covers `DirectInput8Create` resolved by `GetProcAddress`).
- **Test menu (`testmenu.rs`, not enabled yet).** The operator menu is the GameShell's: TEST
  (`WAL_EUROPAR_TEST_MENU`) relaunches into `GameShell.exe`, the payload injected by
  `wal-loader` (the system's `program_loader`). Its security is patched
  (`WAL_EUROPAR_SHELL_PATCHES`: the launcher program's CRC on its command line, `0x406975`;
  the DS1991 iButton through `IBFS32.DLL`, `0x406ac1` and `GotValidIButton` `0x403b60`). Its
  boot (state machine `0x4066cf`, states 2-0x10: update scan, backup copy to `d:\backup`...)
  does not reach its game launch yet; jumping straight to the test menu state (0x14) shows a
  blank page.
- **SEGA Rally 3 (2008).** `Rally/Rally.exe` loads `./inpout32.dll` at run time
  (`LoadLibraryA`, `Out32` by name) and imports `xinput1_3.dll` (ordinals 2-5): the payload is
  installed as `Rally/xinput1_3.dll` (profile `payload_install`, `dll_overrides: xinput1_3: n`
  over Wine's builtin), its XInput exports report no pad, ordinal 2 (`Out32` in the export
  table) is redirected to `XInputGetState` in the game's import table, and `LoadLibraryA` of
  `inpout32.dll` returns the payload. Its board (`WAL_EUROPAR_BOARD=rally3`, `0x57b2f0`): the
  HID device whose attributes read vendor 0x1043 product 0, 15-byte reports (`0x57b640`):
  byte 1 wheel, 3 brake, 4 gas, 6 buttons 0x01 start, 0x02 handbrake, 0x04 view, 0x08 shift
  up, 0x10 shift down. Force feedback (`WAL_EUROPAR_FFB_FORMAT=rally3`, `0x5c8600`): port
  0x1020, low nibble `16 - |force| * 15` (0 none), bit 0x10 a positive force (to the right).
  Credits: no GameShell window; `WAL_EUROPAR_CREDITS=0x741748` (the credit count of the
  object at `0x741738`, a game starts when it is above 0 unless free play) gets one per coin;
  free play from `config.ini` `[Credit] Freeplay` (`WAL_EUROPAR_FREEPLAY`). Resolution
  `[Graphics] HorizontalResolution` / `VerticalResolution` (`WAL_EUROPAR_RESOLUTION`,
  1280x720 or 1920x1080); at 1920 the in-race timer, "time extended", "final lap" and
  countdown x positions (screen width / 2 at `0x5a6f28`, `0x59ea49`, `0x59e806`, `0x5a48e5`)
  replaced by tuned constants (`WAL_EUROPAR_PATCHES_1920`).
- **GRID (2010).** `Sega/Grid/GRID.exe` imports `inpout32.dll` (payload default) beside the
  Sega Shell. Its board is a DirectInput game controller, "SEGA IO Controller"
  (`ActionMap/actionMapSEGAController.xml`): the common fake wheel (6.1 `dinput.rs`) with X the
  wheel, Z the brake, Rx the gas, buttons 0 start, 1 view, 2 reset car, 3/4 gear up/down,
  9 look back. It scales each axis by `DIPROP_LOGICALRANGE` (unanswered: full throttle read as
  ~3%). Shell: the Ford Racing protocol's later version (`WM_COPYDATA`): its window from the
  command line only (`/h=<hex>`, `0x5f4281`; `WAL_EUROPAR_SHELL_ARG=1` appends it to
  `GetCommandLineW/A`), message 12 its window (`WAL_EUROPAR_SHELL_MSGS=window=12`), 4 a credit
  used, 19/21 lamps; shell -> game 3 credits, 13, 0x27, accepted only from the shell's window
  (`wParam`, `0x5e3f30`). Force feedback (`WAL_EUROPAR_FFB_FORMAT=grid`, `0xb9c900`): port
  0x1020, low nibble `15 - |force| * 15` (at least 1; 15 none), bit 0x10 a negative force. A
  deliberate crash at boot (`0x5ab9e5`, `*(0) = 0` when a cabinet flag is clear) is skipped.
  XInput pads are hidden (its xinput1_3 ordinals 2-4 point at the payload's "no pad").

### 6.3h Konami e-Amusement PC (`crates/payload-konami`)

Konami's Windows XP e-Amusement cabinets on the AVS platform (`libavs-win32.dll`: file systems,
threads, properties). Nobiteke! Byon Byon Daisakusen (2008, game code H24, OpenGL, two
positional guns) starts as `E:\byonbyon\bin\byy.exe D:\byonbyon\ F:\byonbyon\` (game data, then
the writable `dev/`: `nvram`, `raw`); the profile passes the dump root seen from `bin/` (`..\`)
for both. System `konami`, payload `wal_konami.dll` installed as `libacio.dll`, Konami's ACIO
serial bus driver, which gs-system / gamemode / testmode import (cdecl, the open-source spice2x
emulates the same devices).

- **Bus (`acio.rs`).** `ac_io_begin` succeeds, `ac_io_is_active2` reports the node enumeration
  done (status 6), every device present (the game probes 1, the ICCA card reader, and 5, the
  HBHI board), versions zeros.
- **HBHI board (`hbhi.rs`).** `ac_io_hbhi_get_control_status_buffer` fills the 0x40-byte status:
  the 15-byte packet at +4 and what libacio derives from it: +0x14 eight 10-bit analog channels
  (left-aligned; channels 0-3 from packet bytes 0-7, high byte then the next byte's top 2 bits;
  4-7 from bytes 10-13, low bits in byte 9), +0x24/+0x25 the 6-bit coin counters of slots 1/2
  (packet bytes 5/7), +0x28 the packet count, +0x38 the packet time (unchanged for 20 s: an I/O
  error). Switches (test menu INPUT CHECK): byte 1 0x20 test, 0x10 service, 0x04 / 0x02 coin mech
  1 / 2; byte 8 0x01 / 0x02 start 1 / 2; byte 3 0x02 / 0x01 the guns' triggers, 0x20 a further
  switch. Guns (CONTROLLER CHECK): controller 1 x/y/z on channels 0-2, controller 2 on 3-5; x/y
  the aim (y's minimum is the screen's bottom), z the trigger pull (0..100). The pointer is scaled
  onto the default calibration ranges (`gs_setting::ANALOGVOL`, 0x10001190: gun 1 x
  0x4800-0xe800, y 0x0a00-0x6e00, z 0x1800-0x4800; gun 2 x 0x2000-0xc000, y 0x2600-0x8a00, z
  0x5800-0x8800): aimed without `calib.xml`, and a test menu calibration with the pointer keeps
  it. Security plugs: the software plug check (`GLH24JAA`) and the system plug answer found
  (0x12), the ids `0140` + 16 hex digits.
- **Card reader (`icca.rs`).** A slot reader: the `card` control inserts / ejects a card
  (`WAL_KONAMI_CARD_ID`, 8 bytes); `req_uid_isfinished` 9 with a card, 0xb without.
- **Network (`net.rs`).** gs-system sets the cabinet's address (192.168.1.17) with iphlpapi's
  undocumented `SetAdapterIpAddress` (5 arguments, stdcall), looked up with `GetProcAddress`: Wine's
  stub aborts the process. Answered done from gs-system's `GetProcAddress`. The boot waits 10 s
  for a DHCP lease before that (the host's adapter has DHCP on).
- **e-Amusement offline (`eamuse.rs`).** bootmode.dll's boot runs 20 steps (I/O board, software
  security, factory settings, card unit, network, system security, e-Amusement connection, ...).
  The e-AMUSEMENT CONNECTION CHECK (step 13) waits for gs_network's board state to leave
  "connecting" (-3) while the test menu's e-Amusement option (`[settings+0xac]`, factory 1) is
  on: forever without Konami's servers. Its offline branch is taken (`jne` at bootmode +0x231b
  NOPed once the DLL is loaded): the game runs in local mode, its attract says so. On a fresh
  `dev/nvram` (no `testmode.xml`) step 17 shows the backup-initialized notice, which waits for
  TEST: the test menu opens, GAME MODE leaves it, and the settings are saved.
- **Screen (`screen.rs`).** gs-system's screen settings (`/config.xml` `/config/screen/`: frame
  the window, mon the monitor, draw the picture, draw_f) default to 1280x720 (0x1000205f): the
  game drew a 1280x720 picture in the window's corner. Their default becomes the screen size: the
  game renders its layout scaled to it (sharp, any resolution; the guns' coordinates scale alike).
  ek-lib's window (`CreateWindowExA`, `SetWindowLongA` styles) is made a borderless popup.
- **Debugging.** `WAL_KONAMI_AVS_LOG=1` puts the AVS log in the payload log: byy.exe boots AVS
  with `/config/log/level` 0 and a writer dropping every line; the level is made 4 and its
  `avs_boot` import passes ours. `WAL_DUMP_THREADS_AT=<s>,...` (payload-common) dumps every
  thread's stack and samples where they run: it found the boot waiting in bootmode.

#### Gashaaaan Refill! (IG2)

Gashaaaan Refill! (2009, game code IG2, Direct3D 9) is another kind of Konami PC: up to four
players throw real balls at two 640x480 screens and a camera board sees where they hit. The dump
is the cabinet's D: drive; `IG2/exec_root/gasya.exe` runs from its directory. No ACIO: the game
loads its own `ig2ACLib.dll` (`LoadLibraryA` + `GetProcAddress` of 53 functions), which boots AVS
and drives two serial boards, and reaches the BM-System (credits, e-Amusement) through
`libbms.dll`, a client of the cabinet's `bms_IG2.exe` service (`\\.\pipe\bmssvc`). The
profile installs the payload as `libbms.dll` (`payload_install`).

- **Two screens.** `setting.tbl` asks `adapterNum = 2` (one swap chain per monitor) and crashed
  on the missing second one; with `windowed = 1` (the profile's copy) the game draws its
  1280x480 picture, both screens side by side, into one back buffer.
- **In-memory patches only.** gasya.exe's CRC is checked at boot
  (`data/system/ig2system1.crc`, SYSTEM DATA CHECK ERROR 5-1550-0000 otherwise); the patches
  check the code they replace.
- **ig2ACLib (`ig2.rs`).** Its internal objects (`CIg2ACLibIOIob`, the I/O board;
  `CIg2ACLibIOBallio`, the camera board) have a no-hardware mode, their boot's argument 1 (boot
  done, security plug checks passed, nothing read): forced (rva 0x3500 and 0x2570). The master
  volume setup through the winmm mixer (SPEAKER SETTING 5-1570-0186 under Wine) returns done
  (rva 0xfd60). `GetProcAddress` gives the payload's functions for the inputs:
  `ig2ACLibIOGetButton` (24 bytes: 9 TEST, 10 SERVICE, 11 coin mech, 12-15 the start buttons of
  players 1-4), `ig2ACLibIOCoinGetIncrease`, `ig2ACLibIOGetBallPosSensorPosNum` / `Pos` (hits as
  int16 x, y in the 1280x480 picture, the size the game gives `ig2ACLibScreenSetWidth/Height`;
  the test menu's CAMERA CHECK draws them). A player's pointer and throw button make a hit, one
  per press, another every 150 ms while held.
- **Boot warnings.** gasya warns CLOCK 5-2551-0005 and ADJUST SCREEN 5-2551-0006 until the
  operator sets the clock and the picture position in the test menu (settings flags at
  0x6add80 / 0x6add81): both checks skipped (rva 0x2d4fc, 0x2d52c).
- **Save drives.** Settings, stage tables and rankings go to E:/ and F:/ (created from
  `data/system/default_save_root`), ig2ACLib's bookkeeping to D:/E:/F: `BOOKKEEPINGS`: the
  profile maps E: and F: to the dump's `E/` and `F/`. Without them a play crashed (division by
  the size of an empty stage list).
- **BM-System (`bms.rs`).** libbms's requests return 0 (accepted), their `_wait` 0 (done) or 1
  (failed): `bms_init_wait` done, the clock the PC's (ms since 1970), e-Amusement status 2 with
  no error ("operating in offline mode"), no rankings, server messages or keepalive. The credits
  are kept in the payload with the coin options the game sends (`bms_credit_update_option`, 13
  bytes: freeplay, start1-4 the credits each kind of start costs, player1-4 their slot, slot1-4
  coins per credit, `bms_IG2.exe`'s coin.xml layout): a game is start kind 1, 2 credits by
  default (`/coin/p<player>s<kind>a1`).
- **Crosshairs (`crosshair.rs`).** Nothing shows where a throw lands (the real balls did):
  `WAL_CROSSHAIR=1` draws each player's aim before `Present` (payload-common's d3d9
  overlay: alpha-blended quads, the game's states in a state block, its render target and depth
  buffer restored), a built-in shape (`WAL_CROSSHAIR_STYLE`) or the player's PNG
  (`WAL_CROSSHAIR_P1`..`_P4`), in the player's color, `WAL_CROSSHAIR_SIZE` % of
  the screen height. The module is payload-common's (`crosshair.rs`, options `WAL_CROSSHAIR_*`,
  formerly `WAL_KONAMI_CROSSHAIR_*`): the Type X touch screen pointer uses it too; an aim spanning
  the screen is placed in the picture when it is letterboxed (`WAL_D3D9_ASPECT`) or the game's
  desktop is fitted (`WAL_DESKTOP_SIZE`).

#### Castlevania: The Arcade (HCV)

Castlevania: The Arcade (2009, game code HCV, Direct3D 9): two players swing motion-sensing whips
at the screen, an IR camera follows them. `HCV.exe` loads its own `hcvACLib.dll`
(`LoadLibraryA` + `GetProcAddress`: I/O board, camera, bookkeeping, test menu items) and links
`libbms.dll` (the BM-System client, as Gashaaaan Refill!): the profile installs the payload as
`libbms.dll` (`hcv.rs`, its BM-System side is `bms.rs`).

- **No-hardware mode.** The dump's `setting.csv` has the debug switches `emulateIO` and
  `emulateSensor` TRUE: the game boots hcvACLib without hardware (`hcvACLibBoot(1)`), but reads
  no buttons (the I/O update's emulateIO branch clears them), no coins (the coin thread waits
  for a debug counter) and no whip (the emulated sensor only shifts the history). Three
  patches: the button read (`je` -> `jmp` at 0x483978) and the coin read (0x482e66) taken again,
  answered through `GetProcAddress` (`hcvACLibIOGetButton`, `hcvACLibIOCoinGetIncrease`,
  `CoinConsume`); the emulated sensor's call (0x483965) points at the payload, which writes each
  player's current sample after the history shift.
- **Secret character.** `setting.csv`'s `useSecretChara` (FALSE in the dump) opens the Little
  Witch on the CHARACTER SELECT: its read (0x4059b7, the bool reader 0x64a6f0 into the settings
  +0xd0) is replaced with TRUE.
- **Buttons** (`hcvACLibIOGetButton`, 30 bytes; the game's bits): 11 SERVICE 0x1, 12 TEST 0x2,
  14/15 the players' START 0x8/0x10, 16/17 the whips' red buttons 0x80/0x100 (next
  sub-weapon), 18/19 their yellow buttons 0x20/0x40 (menus: point and press; in game: throw the
  sub-weapon at the whip's position). Bytes 10 (0x200) and 13 (0x4) are unknown.
- **Whip samples** (I/O object 0x7280e8 + 0xb8, player 2 + 0x4e14, 0x18 bytes, 30 kept): x, y
  (f32, the 1920x1080 picture), speed, camera x, y (i32), on screen, seen. The game reads only
  the current x, y (0x7281a0): `GAMEPLAY` (30 updates per second) follows it with a
  spring-damped cursor per player (stiffness 350, damping 25, at +0xc44 / +0xc68), whose
  position the whip (`GAMEPLAYERWHIP`, 0x44d0d0) keeps 5 of. The whip cracks when in those
  5 a 2-update move exceeds 300 pixels and the stroke from 4 updates ago to 1 ago crosses the
  whole screen, from beyond one edge to beyond the opposite one (x < 0 and x > width, or the
  same in y): it hits along that line. A short slash does nothing. The payload's swing: the
  target 0.55 screen heights (widths) beyond the top for 2 updates, then beyond the bottom for 3 (or
  up), at the aim's x: a vertical stroke (0.5 is the least that cracks from any aim; each
  swing alternates direction); the aim itself is kept on the screen, so moving
  the gun fast never cracks the whip. Found with hardware read watchpoints on the samples.
- **Controls.** `WAL_KONAMI_WHIP_SHOT=0` (GUI option, default 1): classic movement, with the
  attack control held, a drag longer than 5% of the screen swings the whip in its direction
  (a stroke at the drag's angle through the pointer), the drag goes on from there. The raw pointer can never crack it: its stroke's
  ends must be beyond the screen's edges (the spring doesn't overshoot that far).
  `attack` (the gun's trigger) swings the whip, or presses the yellow button
  after `switch` (toggles the trigger, per player); held, it repeats every 12 updates.
  `subweapon` (yellow) and `select` (red) are the cabinet's own buttons.
- **Unlimited sub-weapons** (`WAL_KONAMI_UNLIMITED_SUBWEAPON=1`, default on): the throw's
  hearts check (`jl` at 0x444a9c) and payment (`sub [status+0x98], cost` at 0x444eb4) removed;
  the hearts stay full (the number under the sub-weapon icon is the throws they allow).
- **Open:** the attract demo movie (`movie/playDemo.wmv`, WMV3, DirectShow into the game's own
  renderer) stays black: winedmo decodes it, quartz logs `get_output_samples Failed to get
  sample for source 0, hr 0x80040211`.

#### Action Deka (GDK)

Action Deka (2008, game code GDK, Direct3D 9, 1024x768, Japanese): two players act out a cop
story holding a motion controller in each hand (punch, shoot, throw, drive). `deka.exe` loads
`libacio.dll` (the payload: the GDKI controllers, devices 7-10, and the card reader, whose
device 2 holds the system plug), links `libbms.dll` (pointed at the payload's BM-System, as
Otomedius), and drives the rest itself: a Cypress EZ-USB I/O board and a 3D sensor (`deka.rs`).

- **EZ-USB board** (CyUSB driver, IOCTL 0x220020/0x220024: switches, coin counter, lamps,
  security, SRAM, the tactile transducers): deka.exe links Konami's EZ-USB library; its flat
  `usb*` functions (0x4829f0-0x482d30, cdecl, 0 done, 0x60 busy, -33 failed; boot error
  5-1502-0967 = 1000 + -33, open failed) are pointed at the payload. Switches (active high):
  TEST 0x8000, SERVICE 0x4000, P1 START 0x800, P2 START 0x400; coins: the board's 16-bit
  counter. The "USB SOUND" is the controllers' tactile effects (`data/tacsound/*.tac`
  uploaded to the board, played per controller): silent.
- **3D sensor** (an ultrasonic hand tracker on COM1, 38400 8N1: commands 0x03, the frequency id
  0xb0-0xb3, 0x0c, then 0x06 / 0x07 polls answered by 10-byte packets, 8 data bytes, 0x75 and
  the complement of their sum): not started, the profile passes the game's own `-no3d` (the
  boot list shows 3D SENSOR BAD). The boot's settings checks then stop on the clock (setting
  0x1d, 5-2503-0000) and the sensor's calibration (setting 0x1c): both `jne` (set) become
  `jmp` (0x4c1670, 0x4c16b8).
- **The hands**: the game's debug input, DebugGameIO (`*0x713084`, keyboard and mouse on the
  developers' PCs), overrides the controller API: its per-frame update (vtable, 0x4cff70) sets
  each controller's motion code (+0x2c, the previous frame's at +0x3c, +0x50 enabled), its
  methods answer the aim (0x4d0160: 1024x768, y down) and the trigger (held 0x4d00f0, pressed
  0x4d00c0). Replaced by the payload for both players from their guns: the right hand aims at
  the pointer and holds the trigger; the left hand reports no aim (0, 0: used, the game asks
  "USE RIGHT"). Motion codes (the game's names): 0x10 GUARD, 0x12 / 0x14 ST_UP_HARD /
  ST_DOWN_HARD (a punch, alternating hands, high or low with the aim), 0x19 HOLD then 0x1b
  THROW on release, 0x1c SPECIAL (both hands). Reload: the aim off the screen (the game's
  on-screen test, 0x4d3330) while the button is held.
- **Steering**: the angle of the line between the player's hands (radians, the engine's
  0x4d3230 has no debug override): pointed at the payload, the d-pad turns it toward ±1.2 at
  2.4 rad/s (beyond ±1.05 the game takes a hard turn), back to 0 on release. Driving is in
  stage 3 (the military base).
- **Controls** (default map): b1 trigger + punch, b2 guard + reload (right click), b3 throw,
  b4 special, left / right steering.
- **Stage 4** (IPO) locked until 1-3 are cleared: `WAL_KONAMI_UNLOCK_FINAL=1` (GUI option,
  default on) clears the select's lock flag (`sete cl` -> `mov cl, 0` at 0x4b76ff).
- Text in Shift-JIS: `WAL_ANSI_CODEPAGE` / `WAL_FONT_CODEPAGE` 932.

#### Hyper Sports 2011 Remake (KC2)

Hyper Sports 2011 Remake (Konami China, 2011, game code KC2, Direct3D 9, 1024x768, Chinese):
ten events (100 m, 110 m hurdles, long jump, javelin, boxing, shooting, vault, weightlifting,
table tennis, fencing) for one or two players, each with three buttons (blue, green, red, left
to right) and no stick. `KC2\AMSports.exe` runs on Konami's `gf.dll` framework (window,
Direct3D, DirectInput, sound, HTTP, the cabinet's boards) and imports no I/O library the
payload could replace: the profile starts it through wal-loader (`payload_loader`, the payload
installed as `python4.dll`), the common hooks go to gf.dll (`iat::set_game_module`: the window,
the Direct3D device and DirectInput are its), and the executable's gf.dll imports are answered
(`hypersports.rs`).

- **Python 4 I/O board** (USB, `GfPython4`, through gf.dll's `GfAcio`): `init` (the board
  thread) is not started; `getState` 2 (ready; 3 busy, 4-5 errors), `getSecurityPlugStart` /
  `setOutputStart` / `setCoinBlockersStart` accepted (a command id), `getSecurity` the two
  plugs (0x28 bytes each, 8-byte id + 32 bytes; the game keeps 20 characters of each as a
  string: plug 0 `KC2:C:A:A`), `getCoinStock` / `consumeCoinStock` the coins (slot 0: the game
  combines the slots). The LED controller (`GfLedController`, the board's serial channel 2):
  ready (state 2), 2 nodes, outputs accepted.
- **Switches**: the game's two `GfuVirtualInput`s (one per player, 16 buttons, table at
  0x5552f8) assign each virtual button to input device 2, the board's (gf.dll's extension
  device, fed by the board thread through `GfInput::pushExtensionInputData(time, type, index,
  value)`), and to the keyboard (device 0, the developers' keys). Board buttons: P1 5 / 6 / 7
  and P2 15 / 16 / 17 (blue, green, red: virtual buttons 2, 1, 0), 20 test, 21 service (virtual
  3, 4), 23 the coin switch (the I/O check's COIN MECH); keys Z / X / C, numpad 1 / 2 / 3, T, S.
  The payload's switch thread (started by `GfPython4::init`) pushes each switch change from
  the virtual sticks, as the board thread does: gf.dll's own pressed / released / repeat
  logic follows. The fake DirectInput (`WAL_DINPUT_DISABLE=1`) has a keyboard without a key
  pressed (gf.dll's input start throws without a keyboard device): the developers' keys, and
  the debug ones the game reads directly (WASD free camera, V...), are never pressed.
- **License check** (error 5-1505-0000 SECURITY ERROR): the game reads `hid.bin` from the
  cabinet's D:, E: or F: drive, decrypts it (RSA, then Blowfish CBC) and compares it with the
  PC's hardware id, at boot (a task, 0x45b0f0) and from a thread while running (0x45f190).
  The dump has no `hid.bin`: both return 1 ("security OK!", `mov eax, 1; ret`).
- **Clock check** (5-2503-0000 "the clock is un-setting up", setting 0x20 set by the test
  menu's CLOCK): the boot's `jne` (set) becomes `jmp` (0x45d35e): the PC's clock is the host's.
- The play log goes to `E:\SPORTS_OPTION\log` (else F:): the profile maps E: to the dump's
  `E` folder. The internet ranking (`hypersports.k573.com`) is not emulated.
- **Controls** (default map): b1 blue (left in the menus), b2 green (OK, start), b3 red
  (right).

#### Silent Hill: The Arcade (GHL)

Silent Hill: The Arcade (2007, game code GHL, Direct3D 9, 640x480): two players with light guns.
`KSHG.exe` links `libbms.dll` (the BM-System client, as Gashaaaan Refill!) and `shaiolib.dll`,
the cabinet's I/O library (button board, the two gun boards, the security plug and card readers
on serial ports): the profile installs the payload as `libbms.dll` (`shg.rs`, its BM-System side
is `bms.rs`), which puts a jump to itself at the entry of each shaiolib function. They are cdecl
and return -1 done (true), 0 failed (false).

- **Buttons** (`shaiolib_BUTTON_status`, 9 bytes, 0x80 held, 0x01 changed since the last read):
  TEST, SERVICE, the coin mechanism, P1 START, P2 START, P1 OPTION L, P1 OPTION R, P2 OPTION L,
  P2 OPTION R (the game's bits 0x1, 0x2, 0x4, 0x8, 0x10, 0x40, 0x80, 0x200, 0x400; the guns'
  triggers are 0x20 / 0x100). The option buttons are read by the game (each player's input
  record bits 1 and 2) besides the test menu's INPUT CHECK.
- **Guns** (`shaiolib_GUN_status`, per gun 16 bytes): x, y (i32, the 640x480 screen), a shot since
  the last read, off the screen. A shot is the trigger's press (and held) bit for that frame:
  one per pull; off the screen, the shot reloads (`offscreen`, the pointer out of the window).
  The game's 5-point calibration (test menu, `Data/TestDat/gun.1`) is skipped: the calibrated
  branch of the gun read (`jne` at 0x4d1303) is removed, the position is the screen's.
- **Coins**: `shaiolib_COIN_count` (coins inserted) then `COIN_consume`; the credits are the
  BM-System's (`bms_credit_coin_throw`). The dump's settings are FREE PLAY.
- **Security plug**: the boot asks `shaiolib_SECURE_check_no_error` for each region's plug id
  (`GNGHLJAA` Japan, `GNGHLAAA` Asia, `GKGHLEAA` Europe, `GKGHLUAA` USA, `GKGHLKAA` Korea,
  `GKGHLABA`): the one of `WAL_KONAMI_REGION` (default E) answers. The SYSINFO ids are hex
  strings (the game groups them by 4).
- **Speaker channel**: without `Data/TestDat/speaker.1` (the test menu's SOUND OPTIONS > SPEAKER
  CHANNEL: one byte, 0 2ch, 1 5.1ch, then its CRC-16 high byte first: CCITT from 0xffff, the
  game's table at 0x688330) the boot stops on 5-2551-0006 "A speaker channel is un-setting up".
  The payload writes it at each boot from `WAL_KONAMI_SPEAKERS` (`2`, default, or `5.1`).
- **Movies** (attract demo, story scenes): raw MPEG-2 video streams (`Data/Movie/*.vid`,
  `Data/eng/Movie/*.vid`, one sequence header at the start of each file) played through
  `RenderFile` into the game's own renderer filter, cued with `IMediaControl::StopWhenReady` then
  run at once. They stayed black with GE-Proton11-10: its quartz stopped the graph after the
  Run (StopWhenReady's deferred stop), winedmo's splitter restarted where it had read ahead
  instead of the file's start (no sequence header: "Invalid frame dimensions 0x0"), and its MPEG
  picture assembly resent a rejected picture for ever. Fixed in the runner (protonge branch
  `winedmo-mpeg-movie-restart`, patches 0099-0101; quartz.dll, winedmo.dll, winedmo.so). The
  dump's `clvsd.ax` (CyberLink decoder, its `FixSilentHillCodec.bat`) is not needed.

#### Gashaaaan (F24)

Gashaaaan (2006, game code F24, OpenGL) is the first game of the series (Gashaaaan Refill! is
its 2009 successor, another dump and profile). `bin/ballshooting.exe` runs `system.dll`, which
boots AVS (BALLS.INI: `/dev/nvram` on `F:\`, `/dev/raw` on `E:\`: the profile maps both) and two
serial boards through the game's own MSVC C++ libraries: `iob.dll` (I/O board) and
`ballio.dll` (camera board, "bs2"). The payload is installed as `bin/iob.dll` (`f24.rs`; its
exports under their MSVC names in `libacio.def`).

- **iob.** `iob_get_status` 3 (ready: 4 asks a firmware update, 5 a hardware error),
  `iob_check_plug` 1 (both plugs; 2 and the missing one otherwise), the ids `0140` + 16 hex
  digits (software, network), `0100` + 6 bytes (hardware). `iob_step` (every frame, the IOSTEP
  task) takes the buttons: `iob_f24_button(n)` 0-3 START of players 1-4, 4 TEST, 5 SERVICE, 6-7
  the coin mechs (the test menu's INPUT CHECK), `_trg` its press this step;
  `iob_f24_nr_coin(slot, &mask)` a 6-bit counter (mask 0x3f) the game counts the steps of. The
  ball supply motors (`ballout_*`, status request then read) report done, no error.
- **ballio.** A jump at each export's entry to the payload (the stage DLLs, loaded later,
  import them too): `ball_io_boot`, `_step`, `_point_get` (int16 x, y per hit, `size` bytes, the
  count returned; nothing while `ball_io_mode_set` is 0), `_enable` / `_disable`,
  `bs2_get_status` 2 (booted: the boot waits for it, <= 0 is a HARDWARE ERROR),
  `bs2_calibration_get_state` 1, `bs2_get_error_code` 0. The hits are in the scene's pixels
  (1248x468, `sfgl_set_screen_size`): a player's pointer and throw button, one per press,
  another every 150 ms while held; a throw outside the picture misses. ballio itself has a
  developer mode without the board (`ball_io_boot`'s second argument 0: mouse clicks as hits,
  `util.dll` `check_kbd_trg` / `mouse_util_get_point`), unused. The test menu's CAMERA CHECK
  shows STATE OK; its ENVIRONMENT CHECK and CALIBRATION still call the original bs2 functions.
- **Two screens as one (`f24_screen.rs`).** The cabinet has two 640x480 projectors side by
  side. The game draws its scene once, 1248x468, in the upper left of its 1280x480 window,
  copies the window to a texture and draws it back as two 640x480 halves (system.dll rva
  0x3f39, `fgl_draw_frame`), the left one 8 pixels to the right, the right one starting 16
  pixels before the left one's end (the projectors overlap) plus the test menu's screen
  offsets: a band shown twice in the middle. `sfgl_get_double_screen_mode` (util.dll) answers
  0 (the halves copied in place) and before fgl.dll's `SwapBuffers` the scene is blitted over
  the borderless screen-sized window (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE: screen`),
  stretched to it (the profile's default) or aspect kept (`WAL_KEEP_ASPECT_RATIO=1`, Batocera's
  KEEP ASPECT RATIO); the pointers are mapped onto that area. The test menu draws in the left 640x480.
- **Crosshairs.** `WAL_CROSSHAIR` as Gashaaaan Refill!'s, drawn after the scaling with
  the fixed-function pipeline (`crosshair::draw_gl`: the game's attributes, matrices, texture
  units and program saved around them).
- A fresh `F/` shows OPTION SETTINGS / BOOKKEEPING BACKUP DATA ERROR: TEST repairs them.
  AVS keeps the times of the `F:\` files in `F:\info.bin`, and takes a file of other times for
  a corrupted one: a SquashFS image keeps whole seconds only, so the times AVS reads are
  rounded down to the second (`f24.rs`, `libavs-win32.dll`'s `GetFileInformationByHandle` /
  `FindFirstFileA`); backup data repaired with it stays valid in an image. Backup data saved
  before (an image's: its times lost) is repaired at once: on the error screen (system.dll's
  boot check, state 6) `iob_f24_button_trg(TEST)` answers the press.
  The attract says local mode (e-Amusement offline), nothing patched for it.

#### Otomedius

Otomedius (2007, game code GGG, Direct3D 9, 1280x1024 fullscreen) imports three Konami
libraries: `libacio.dll` (the ACIO bus with the card reader and a PJEI board), `libbms.dll` (the
BM-System client) and `libtouchpanel.dll` (a serial touch screen). The payload is installed as
`libacio.dll`; its DllMain sees otomedius.exe and points the exe's libbms and libtouchpanel
imports at its own functions of the same names (`otomedius.rs`, walking its export table), so
one payload instance serves the game. The dump's `otomedius_edit.exe` is a community edit (save
paths moved off E:/F:, flags forced): the original exe is run, E: and F: mapped to the dump's
`E/` and `F/` (`nvmem\testmode.bin`, `bookkeep.bin`).

- **PJEI (`pjei.rs`).** The 40-byte status buffer: byte 4 start 0x80, up 0x40, down 0x20, left
  0x10, right 0x08, buttons A 0x04, B 0x02, C 0x01; byte 16 test 0x20, service 0x10, coin 0x04
  (the game's mask tables at 0x5c30a0 / 0x5c30e8; spice2x's Winning Eleven PJEI layout). Coins are
  a stock the game reads and consumes. Security plug `GEGGGJAA`: check status 0x14, missing check
  8. `ac_io_avs_boot` succeeds without AVS (only libacio used it).
- **Touch screen (`touchpanel.rs`).** `ac_touchpanel_new(1280, 1024)`; reads give x, y in that
  space and a touched dword. The touch is player 1's right stick (the profile's mouse or gun
  drives `rx`/`ry`, its button `b4`), mapped into the picture: the 5:4 picture is fitted in the
  screen with its aspect. `ac_touchpanel_check_wait_receive` is true while still waiting (the
  boot's diagnostic; spice2x's Winning Eleven answer is the opposite), the init screen waits for
  the panel's state 3.
- **Data check.** Boot step 0 reads `ChkSum.bin` (the data files and their hashes), missing from
  dumps (FILE BROKEN 5-1504-0000): without the file the step handler is replaced by "step 1" (rva
  0x2d492); a present file is still checked.
- **BM-System.** The older libbms: blocking credit calls (`coin_throw`, `get_status`,
  `confirm(player, kind)`, `consume(player, kind)`, `service`), `time_get_clock_blocking`; card
  management, matching, player data and the cabinets' TCP links fail (no network). A first boot
  initializes the backup files (BACKUP FILE ERROR 5-2502-0003, TEST: test menu, GAME MODE saves).
- **Text.** `WAL_ANSI_CODEPAGE=932`, `WAL_FONT_CODEPAGE=932` (Shift-JIS font names). The menus'
  news ticker stays garbled.

#### Winning Eleven 2008

WORLD SOCCER Winning Eleven ARCADE CHAMPIONSHIP 2008 (game code HCK, Direct3D 8, 1024x768
fullscreen): the dump is the cabinet's D: drive, `HCK/bin/game.exe` imports `libacio.dll` (PJEI
board, PJEC PS2 pad port, card reader), `libtouchpanel.dll` (`ac_touchpanel_new2`), AVS and
`libacweeq.dll` (speaker equalizer). The payload is installed as `libacio.dll`; DllMain sees
libtouchpanel's `ac_touchpanel_new2` loaded and points the touch panel imports at its own
(`winning_eleven.rs`). The dump's `D3D8.dll` is dgVoodoo (Direct3D 12 output): hidden, Wine's is
used. spice2x supports the later Winning Eleven 2012/2014 (KCK, NCK) with the same boards.

- **D: drive.** `D:/HCK/data`, `D:/HCK/save`, `D:\romcheck.bin`: payload-common's D: redirection
  to the dump root (`..\..` from the executable, `WAL_KONAMI_DDRIVE`), also on sound.dll, which
  opens its banks itself (`drive::redirect_module`; "failed not found AudioObjectContextData"
  otherwise), with `mmioOpenA` (the menu music and sounds: silent otherwise; `redirect_module`
  also redirects a module's `mmioOpen`). The attract movies (`D:/HCK/data/movie/demo_<n>.dat`,
  WMV) are a DirectShow graph's `RenderFile`: payload-common's `CoCreateInstance` hook redirects
  the drive's paths there too (it was only installed with `WAL_GAME_DRIVE`).
- **Security plug and devices.** The game tries four software plugs, `G[U|C]HCK[J|H][A|B]A`. The
  `U`/`A` ones are spec A, the Live Monitor PC (the shop's spectator screen, no controls), the
  `C`/`B` ones spec B, a player cabinet; the device check then needs the card reader and the pad
  port with spec B, neither with spec A (error 8 otherwise). Only `GCHCKJBA` is answered present
  (`pjei::set_software_plug`, status 0x12 for the others).
- **ROM check.** Boot checks every file of `romcheck.bin` (CRC32/MD5, function at rva 0x17b40):
  `libacio.dll` is the payload, passed (FILE BROKEN 5-1515-0000 otherwise); the others are
  still checked.
- **Network.** `ags_network` init reads the adapters with `GetAdaptersInfo` into a fixed 2 KB
  buffer (3 adapters): a PC with more got ERROR_BUFFER_OVERFLOW and the game quit at once (exit
  -1). The game's import returns those that fit, the ones with an address first (`net.rs`).
- **Sound.** `fck_set_eq` (libacweeq, the Realtek driver's equalizer through `RtEffect.dll`)
  fails without that driver: SOUND INIT ERROR 5-1511-0101. Answered done.
- **Network options.** e-AMUSEMENT and LIVE MONITOR CONNECT (test menu NETWORK OPTIONS, in the
  test-mode data `D:/HCK/save/weags_config_data.bin`, checksummed) are on by default: the boot
  stops on 5-2002-0000 (center server, SERVICE continues), then 5-2505-0000 (Live Monitor not
  found). After the data is loaded (rva 0x13400, the struct at 0xc83088: e-AMUSEMENT, then LIVE
  MONITOR CONNECT) both are set off unless `WAL_KONAMI_EAMUSEMENT=1` / `WAL_KONAMI_LIVE_MONITOR=1`
  (e-AMUSEMENT also kept with a services host, below): attract in local mode, free play by
  default.
- **e-Amusement server.** `WAL_KONAMI_EASRV=1` (the profile's default): the payload starts the
  fake server (crate `wal-easrv`, 127.0.0.1:33701) and the game boots online. game.exe builds
  its e-Amusement configuration itself (rva 0x715c0): `/ea3/network/services` is
  `services.<the PC's DNS domain>` (its `GetNetworkParams` domain, the shop's network), a host
  AVS posts to as `http://<host>/`; the payload's `property_node_create` import gives
  `127.0.0.1:33701` there (`WAL_KONAMI_SERVICES=<host[:port]>`: an external server's). The
  2008 library (libavs-win32.dll):
  - posts `POST /` with text XML (`<call model=..><services method="get"/>`): the server
    takes the module and method from the call. Its answers stay binary XML, encrypted;
  - reads an answer into the game's 4 KB `sz_xrpc_buf`: the full services list (23 items,
    1608 bytes of binary XML) fails ("read error by mem"): HCK gets its own modules only
    (cardmng, clientlog, facility, goalreplay, message, netconfig, package, pcbevent,
    pcbtracker, playerdata, posevent, ranking, sidmgr, system, takeover, local, keepalive) at
    the server's root;
  - `ea3_get_status` is 1 ("checking the network": notice 3, "e-AMUSEMENT PASS and services
    unavailable") until its `server` keepalive answers: with the services list's `keepalive`
    item it turns 3 (online) after about 10 s and the notice goes.
- **Its own modules** (`easrv/hck.rs`, the `ac_net_*` code of game.exe). Most answers are
  wrapped: the module node holds `data` strings, base64 chunks (≤ 1020 characters) of a
  document whose `/player` node has the fields (restore_recvdata, rva 0x95e20); the session
  and takeover answers are plain fields of the module node:
  - `netconfig.get` (`config_name`): `player/config_value`, base64 text. `SETTING` is the
    shop-to-shop match's settings (`ONLINE_MATCH_SETTING`, five `|`-separated numbers; "GET
    SETTING: FAILED" without), `OSHIRASE` (news), `TABOONAME` (banned names): empty.
  - `dbfunc_call` / `dbfunc_call64` (stored procedures: leagues, tournaments): `result`, base64
    text (the first reads it wrapped, the second in the module node): empty.
  - `ranking.getlist`: `player/result`, `record_num` 0.
  - `playerdata.login` / `logout`: `status` (`invalid` a failure), `comid` (the session id),
    `login` / `logout` s32. `takeover.load` (Winning Eleven 2006 and location test data):
    `flg_comp`, `flg_play_we06`... 13 s32, 0.
  - `playerdata.usergamedatadetail_send` / `_recv`: the player's records by key (`T1I01`,
    `T1S01`... 42), a CSV line each of the columns `vk,i1..i8,d1..d7,l1..l8,s1..s3` (`vk` the
    key) and maybe a `bin`. Sent as `d`: base64 of the text's length (u32) and the text in
    LZSS (Okumura's: 4 KB ring of zeros from 0xfee, flag bytes from the lowest bit, 1 a
    literal, 0 a 12-bit position and length - 3); read back (`_recv`: `vkeycsv` the keys,
    space-padded) as `player/record/d`: base64 of the plain text in the asked columns, or
    `<NODATA>`, `record_num` their count. Kept as `dev/nvram/easrv/<eaid>/HCK/<key>.kbin`
    beside game.exe (the run directory). A card's first login sends its new records after
    `bindmodel`; the next ones load them.
- **Cards.** The reader is the old slot one (`ac_io_icca_req_uid`), no keypad: the card's PIN
  is touched on the screen (ICCardPasswordCheck scene, rva 0x2c740 its start, 0x2c800 its
  end; 20 s). The payload touches its keypad buttons for the player's numpad (`kp0`..`kp9`)
  and, with a configured card (`WAL_KONAMI_CARD_PIN`), the PIN by itself 1.5 s after the
  screen opens (`touchpanel::set_auto_touch`). Then `authpass`, a new card's `bindmodel`, the
  data load, the e-AMUSEMENT PASS notice, ICCardSucceed; a new player names a team (stick,
  A/B) and picks an emblem.
- **In-match sound effects.** CRI ADX (in game.exe, its own DirectSound device) streams the
  commentary and the crowd from `src/wedat/root/*_sound.afs`; the other match effects are
  DirectMusic (`src/wedat/area/0/text/sound/internal` banks). The prefix's native DirectMusic
  (other games' tricks) fails with Wine's dsound (it asks dsound for a private interface): the
  profile uses Wine's. After the performance starts, rva 0x57d1a0 sets the audio path's reverb
  (a DMO effect, not in Wine) and turns the effects off when that fails: kept on. Wine's
  `InitAudio` creates the default audio path inactive, which deactivates the synthesizer's port
  (every note dropped, "Failed to update performance latency"): activated after the setup (rva
  0x57df30). Wine's performance still processes few of the game's messages.
- **Network notice.** "Operating in local mode: e-AMUSEMENT PASS and e-AMUSEMENT service not
  available" (notice 1 of the table at 0xc898c0, set by rva 0x28770, drawn by rva 0x28880 while
  its id is not 0) is the case offline: id 1 is set as 0.
- **Controls.** The player chooses the arcade stick or the home (PS2) controller by touching the
  screen (ControllerSelect scene, a button touched twice is taken); the touch hit test (rva
  0x46670, called at 0x467f6) answers the arcade panel, taken at once: the PJEI board, stick and buttons A-F
  (byte 4 A 0x04, B 0x02, C 0x01, byte 6 D 0x80, E 0x40, F 0x20). The touch panel is never
  touched (every virtual button is a pad button). The PJEC pad (72 bytes, analog controller 7,
  present 0x5a, buttons bytes 4-5, sticks 8-11) is answered from the same stick.
- **Text.** `WAL_ANSI_CODEPAGE=932`, `WAL_FONT_CODEPAGE=932`.
- **Debugging.** `WAL_KONAMI_GAME_LOG=1`: the game's log function (rva 0x30510) is the build's
  shared empty `return 0` (other empty functions land there with other arguments): it jumps to
  the payload, which logs a call whose first argument is a readable text format (a small printf,
  `%s` pointers checked with `VirtualQuery`). AVS's log comes through it too.

#### Winning Eleven 2012

WORLD SOCCER Winning Eleven ARCADE CHAMPIONSHIP 2012 (game code KCK, Direct3D 9, 1920x1080) is
an AVS game like Road Fighters 3D: the dump's `contents/modules/weac12_bootstrap_release.dll`
(the `.windowsloader`), run by `wal-avs-boot.exe` (installed beside it in `modules/`). The boot
moves up to `contents/` (`prop/`, `data/`, `dev/`) when `prop/` is there and not in
`modules/`; the DLLs load from the program's directory. Its AVS is 2.13 (numbered exports:
`XC058ba5...`, ea3 `XEb552d5...`, spice2x's table for 2.13; chosen when `avs_boot` is missing),
its heaps 30 MB (`WAL_KONAMI_AVS_HEAP_MB`). The payload is its `libacio.dll` (`we2012.rs`).

- **Region.** The cabinet runs the game from `D:\KCK-00<n>-<version>\contents`; the game looks
  for `KCK-001` (Japan), `KCK-002` (Asia), `KCK-003` (China) in its current directory (rva
  0xedfef0) and picks with it the language data (`data/convert/ac/j`, `english` or `ctrad`,
  `csimpl`), the attract movies, the touch screen, the LCD monitor. From another directory no
  movie is picked: the first one after the attract's title is a null name (the crash after the
  attract with spice2x, which runs it from `C:\game\contents`). The licence check compares it
  with the soft id code's destination (`J` 2, `A` 1, `C` 0; SECURITY ERROR 5-1512-0006 when they
  differ). `WAL_KONAMI_DEST` sets both: wal-avs-boot gives that destination (in place of the
  dump's `/ea3/soft/dest`), the payload answers the check for its code. This dump is the
  English one (`english` data, `j` renamed `j-old`): `A`.
- **Touch screen.** Player 1's right stick (the profile's mouse or gun) and button 7. Asia and
  China: the serial panel of `libtouchpanel.dll` (`ac_touchpanel_new2(640, 480)`), its imports
  answered, over the whole picture. Japan: a USB touch monitor (an Elo, a mouse for Windows):
  the boot looks for its device among the PC's (rva 0xedb1a0, TOUCH PANEL ERROR 5-1513-0000:
  answered present), the game reads `GetCursorPos` (hooked) and its window's left button
  messages (byte 1 of its key table, written by the payload); the PC's mouse messages are not
  dispatched. The controller and mode choices are touch buttons; untouched, their timers pick
  the arcade stick and the COM match. The controller choice (AcSceneControllerTypeSelect_2012,
  vtable rva 0x14d90ac, slot 5 once a frame) is taken for the player: while it is on screen the
  payload touches the arcade controller's button on the serial panel (Asia, China; the Japan
  USB touch is not driven).
- **LCD monitor (Japan).** COM1: `0<command> <value>\r\n` (`MODEL?`, `BRI`, `CON`, `BL`, `CSM`,
  colors) echoed `9<command> <value>\r\n`, then `9OK \r\n` (spice2x's answer); LCD MONITOR ERROR
  5-1514-0000 without it.
- **Boards.** PJEI as Winning Eleven 2008's (stick, A-F, test, service, coin; coin slot noise
  flag never set), PJEC (the PS2 pad, the touch button left out), I36I (the PS3 pad port: no
  pad, a DualShock 3's USB descriptor, as spice2x). The security plug is the AVS environment's.
- **Offline.** e-Amusement local mode; its notice ("Operating in Local Mode", network notice
  id 1, drawn by rva 0xfe5a00 while the id is not 0) is not drawn (the other notices are); free
  play from the dump's
  `dev/nvram/coin.xml`. The online update fails each minute (`F:\KCK-000\update`), harmless.

- **e-Amusement** (`WAL_KONAMI_EASRV=1`, the profile's default; `WAL_KONAMI_SERVICES` an
  external server). The dump's network options (`dev/nvram/networkopt.xml`, the test menu's
  NETWORK OPTIONS) are all 0: "Operating in Local Mode", no card. With a server, their getter
  (rva 0xf03c70, a trampoline) returns them with the network on. The card's PIN screens
  (AcSceneIcCardConfirmPassword_2012 / RegistPassword_2012, vtable slot 5) shuffle their keypad
  (14 patterns, the scene's +0x234, the game's digit strings at rva 0x172b718): the configured
  PIN (`WAL_KONAMI_CARD_PIN`), else the numpad, is pressed through the keypad's own handlers
  (key press rva 0xfb35a0, esi scene / edi key; commit rva 0xfb3360). The server's `KCK`
  module (`easrv/kck.rs`): `userdata.login` (session, is_new), `load` (the player's data as
  saved, or a new player), `save` (kept in `dev/nvram/easrv/<refid>/KCK.kbin`), `takeover`
  (none), `userenv.get`. The game requires its answers' some 240 values and lists (each
  missing one: "the line is congested", the game ends): while the card's login runs
  (AcCardEntryLoadUserDataSeq_2012, vtable slot 8) and `userenv.get` is parsed (rva 0x10bd150,
  also after a game), the game's libavs `property_node_refer` reads a missing value as 0 (a
  date: the game's release) and `property_search` answers a missing group or list entry with
  the answer's `empty` group (the server's). The player list needs 32 entries (`player/num`).
  A new card: e-Amusement terms (touch YES), the team, a match. Not verified yet: the save at
  a game's end and its reload (user test pending).
- **Touch** (`touchpanel.rs`, screen calibration). The game calibrates its serial panel in
  screen pixels every frame (`ac_touchpanel_caribrate_set`: raw (128, 128) is screen (128, 128),
  raw (1792, 952) is screen (1792, 952), identity until the test menu's TOUCH PANEL CHECK
  calibrates): the panel's raw point is the pointer in screen pixels (`read_on_direct`), its
  point the raw one through that calibration (`read_on`); reading the calibration's size as the
  panel's (as before) put the touch 7% short toward the bottom right. The red touch pointer
  (`WAL_CROSSHAIR`, `crosshair.rs`, `right-stick` axes) skips lost devices: its state
  block kept the game's resources and every `Reset` failed (the game froze).

#### GTI Club: Supermini Festa!

GTI Club: Supermini Festa! (2009, game code HGT, Direct3D 9, 1360x768) runs `gti3.exe` from the
dump root (`gti3japkey.exe` is the Japanese build: plug `GQHGTJAA` instead of `GKHGTEAA`, the
only difference). It imports `libacio.dll` (the HBHI board: switches, coins, security plug; the
card reader) and loads a second libacio, `dllibacio.dll` (`LoadLibraryA` + `GetProcAddress`,
rva 0x403d0), for a second ACIO bus with the HGTH board: the motorized wheel, the pedals. The
payload is installed as `libacio.dll`; the exe's `LoadLibraryA("dllibacio.dll")` loads the
payload again (`gti_club.rs`): one instance for both buses. The dump's `dinput8.dll` (with
`SDL2.dll`, `MetroFramework.dll`) is a third-party force feedback plugin: hidden.

- **HBHI switches.** Packet byte 1: test 0x20, service 0x10, coin mech 0x04 (the coins are a
  stock: `ac_io_hbhi_current_coinstock` / `consume_coinstock`); byte 8 (the test menu's INPUT
  CHECK): 0x01-0x08 SELECT UP / DOWN / LEFT / RIGHT, 0x10 START, 0x20 ACTION, 0x40 / 0x80 the
  lever's SHIFT UP / SHIFT DOWN. The game's switch flags (0xa7e784, built at rva 0x41037) also
  hold the wheel and pedals past half way. Test menu: SELECT UP / DOWN move, START executes;
  INPUT CHECK exits with SHIFT UP + START.
- **HGTH (`hgth.rs`).** `ac_io_hgth_get_recvdata`: 32 bytes, +2 wheel, +4 accelerator, +6 brake
  (16 bits). The game maps them with its calibration (min 0x918c74, max 0x918c68, wheel center
  0x918c14; ANALOG CHECK shows them): the sticks are scaled onto the calibration it has, so no
  calibration is needed. `ac_io_hgth_set_senddata` gets the motor's 4 bytes (0x918cbc, built at
  rva 0x1475d0): byte 0 the torque -127..127 against the wheel's turn (self-centering; wheel
  right, torque negative), byte 1 a decaying vibration level 0..63 (crashes, hits): constant
  force and vibration outputs (`WAL_KONAMI_FFB` percent).
- **Calibration check.** Boot step 0x44ba90 wants the three calibrated flags of the test menu's
  CALIBRATION (ERROR 5-1550-0000 "PERFORM THE I/O CALIBRATION" otherwise): passed in memory
  (rva 0x4bad8). A fresh `CONF/NVRAM` shows 5-1514-0000 (backup initialized) once: TEST opens
  the test menu, GAME MODE saves.
- **Hidden content.** The test menu's GAME APPEARANCE OPTIONS takes a password that sets a
  content level (0x331d71c, saved in the backup, 0..3; the codes, 64-bit values rotated by 4
  bits at 0x692fcc: KXTY8YP2 1, QH84EVVA 2, GZ6LYJD5 3, the U.S. course, Money Run and 2 VS 2
  car football). The backup load (0x51c470) answers level 3, the saved backup's and the fresh
  one's.
- **Boot time.** The boot reads every drive's volume and free space (`GetLogicalDriveStringsA`):
  Wine's Z: (the host's `/`) took over 30 s. The game gets C: alone.
- **Rendering.** DXVK draws the test menu's black background and the HUD's dark panels white:
  wined3d (`dxvk: false`). The fullscreen 1360x768 device fails under Wine, and payload-common's
  windowed retry (the window resized to the screen after it was created) showed the back buffer
  small in the corner, its right side cut. The profile makes the device windowed from the start
  (`WAL_D3D9_WINDOWED_SIZE: 1360x768`) in a borderless screen-sized window
  (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE: screen`): the picture stretched to the screen.
- **Input filtering.** DirectInput (the PC keyboard) disabled; `GetKeyState` (Shift, a debug
  display) answers no key.

#### Contra: Evolution

Contra: Evolution (2011, game code KC1, Direct3D 9, 1920x1080 windowed) is `AMContra.exe` at the
dump root; this dump is the Chinese release (`KC1:C:A:A:2011011801`, GB2312 fonts: code page
936). It imports `libacio.dll`'s functions under their MSVC names (`?ac_io_begin@@YA_NEPBDPAHH@Z`,
cdecl): the payload exports those names too (`libacio.def`, linked by `build.rs`), but this build
never calls them. The cabinet reaches it in other ways, answered in `contra.rs`:

- **Controls: the DirectInput keyboard.** CButtonMan maps DIK codes to actions with a table of
  {DIK, action} pairs (0x55cea8-0x55cfd4): 0/1 start P1/P2, 2/3 left, 4/5 right, 6/7 down, 8/9
  up, 10/11 select (weapon), 12/13 shot, 14/15 jump, 16/17 a fourth button the INPUT CHECK does
  not show, 20 test, 21 service, 38-54 test menu navigation sharing P1's keys (W/S/Enter). The
  game sets up the system keyboard and mouse (`CreateDevice`, `SetDataFormat`, `EnumObjects`,
  `GetDeviceInfo`: a failure throws, boot error 0x81). The fake DirectInput (`WAL_DINPUT_DISABLE`)
  gets a system keyboard (`dinput::set_keyboard`): every key of each pressed action, read from
  the table; the mouse is answered, never moving.
- **Coins: the control panel on COM1** (115200 8N1). The game writes `0x90` (its start); the panel
  sends 2-byte events, `0xB0 0x0a` a coin of slot 1, `0xB0 0x09` slot 2 (code table 0x583d10,
  handled at 0x4c62c0). The INPUT CHECK's COIN MECH switch stays off; the credits count.
- **Clock.** The settings getter (0x463a50, `return settings[index]`) reports setting 0x36, the
  operator clock set, on: the game uses the PC clock (unset: boot error 5-2503 "The clock is
  un-setting up").
- **Fonts.** `GetCharABCWidthsA` with a double-byte character and `first == last`: Wine converts
  both bytes and writes two ABC structures, overwriting the game's stack (crash at 0x43506d, then
  0x43540a in the attract). `WAL_ANSI_CODEPAGE` answers the call per character with the wide
  function (code page 936).
- The USB HID I/O board fallback (VID 055c PID b3ab, SetupDi + HidD) finds no device: unused.
- The dump's `d3d9.dll` (DXVK) is hidden; `d3dx9_42` is a winetrick.

#### Road Fighters 3D

Road Fighters 3D (2010, game code JGT, Direct3D 9, 1920x1080, GTI Club's engine: its file cache
is still `FileCache_Gti3`) has no executable: the game is `jgt.dll` (exports `dll_entry_init`,
`dll_entry_main`), which the cabinet's launcher runs. The dump's `.windowsloader` names the DLL;
the konami system's `dll_host`, `wal-avs-boot.exe` (`crates/loader/src/avs_boot.rs`), is
installed beside it and started as `wal-avs-boot.exe jgt.dll`. It follows the launcher's steps
(spice2x's `avs/core.cpp` and `avs/ea3.cpp` are the reference):

1. `kbt.dll` and `kld.dll` loaded when present: jgt.dll finds kbt's `bt_get_ikey_status` with
   `GetModuleHandleA` (a null call otherwise). The dump's are spice2x's stubs.
2. `avs_boot` (`libavs-win32.dll`, the 7-argument AVS 2 legacy form, two 16 MB heaps) with
   `prop/avs-config.xml`, its `/sntp/ea_on` off and `/time/gmt_offset` the PC's.
3. `libavs-win32-ea3.dll`, then `jgt.dll` loaded (its `libacio.dll` import: the payload).
4. `prop/eamuse-config.xml` (`ea3-config.xml` when there is one): its `/ea3/soft` model, dest,
   spec, rev, ext give the AVS environment's `/env/profile/security_code` (`G*JGTJAA`),
   `soft_id_code` (`JGT:J:A:A:2010072003`), and the system, hardware, software, account ids.
5. `dll_entry_init("JGTJAA2010072003", prop/app-config.xml's /param)`, `ea3_boot(/ea3)`
   (offline: `5-2002-0000 EA3_BOOT_CODE_SERVICES_ERROR`, the game runs in local mode), then
   `dll_entry_main()`.

`WAL_KONAMI_AVS_LOG=1` writes the AVS log to `avs.log` in the run directory. The payload's
hooks go to the game DLL: `wal_payload_common::iat::set_game_module(jgt.dll)` before the common
payload starts (its import hooks, `WAL_PATCHES`, the code page fix use the game module).

- **HBHI switches** (`rf3d.rs`). Packet byte 1: test 0x20, service 0x10, coin mech 0x04; byte 8:
  VIEW 0x04, 2D/3D 0x08, the shift lever's switches up 0x10, down 0x20, left 0x40, right 0x80
  (an H pattern: 1 up-left, 2 down-left, 3 up, 4 down, 5 up-right, 6 down-right). `gear_up` /
  `gear_down` step a sequential gear through it. No START: the accelerator starts and confirms,
  the wheel selects. Test menu: SHIFT UP / DOWN (the lever) move, VIEW executes.
- **HGTH** (`hgth.rs`). jgt.dll loads `libacioEx.dll` (`LoadLibraryA` + a `GetProcAddress`
  table of the generic, HBHI, HGTH and HDXS functions, every one needed): the payload again.
  Wheel and pedals at full range (0..0xffff). Motor: bytes 0-1 a 12-bit torque centered on
  0x800, 0 the motor off (full right 0x8f8, full left 0x708 at low speed): the constant force
  against it, full scale 0x7ff (`WAL_KONAMI_FFB` percent). Bytes 2-3 (10 bits, not following
  the wheel) unused.
- **Card reader.** The workflow API (`ac_io_icca_workflow`, `ac_io_icca_get_status`, 24 bytes:
  state, sensors, id; `ac_io_icca_get_uid_felica`): the `card` control presents the card.
- **Rendering.** DXVK draws the race countdown's digits as black blocks: wined3d.
- **Input filtering.** DirectInput disabled, `GetKeyState` no key, `GetLogicalDriveStringsA`
  C: alone (as GTI Club).

#### Thrill Drive 4: Crazy Streets

Thrill Drive 4: Crazy Streets (2007, game code F04, `F04:E:A:A:20070330` export build, Direct3D
9, 640x480) is `td4/td4.exe` (`td4_go.exe` is a patched copy, unused). It imports `libacio.dll`
(the PJEI board: switches, coins, the wheel's and pedals' potentiometers, security plug
`GKF04EAA`; the card reader) and loads `libtd4io.dll` (`LoadLibraryA` + `GetProcAddress` of 27
functions, rva 0x48cd0) for a second ACIO bus: the D44H board (the wheel's motor) and the D44S
board (the seat, SEATBELT on the boot screen). The payload is installed as `libacio.dll`; the
`libtd4io.dll` load gets it again (`td4.rs`). The dump's `libacio.dll` and `libtd4io.dll` are
third-party stubs, and its `TD4.ini` (`[IO] HANDLE=KEYBOAD`, a development mode reading the PC
keyboard) is hidden.

- **PJEI switches.** Byte 4: START 0x04, the shift lever's UP 0x20 / DOWN 0x40, VIEW 0x80; byte
  16: test 0x20, service 0x10, coin mech 0x04 (the game's switch flags, rva 0x4ac1b). Test menu:
  LEVER UP / DOWN select, START executes; INPUT CHECK exits with LEVER UP + START.
- **Wheel and pedals** at +8 / +10 / +12 of the PJEI status (u16): the game maps them with its
  calibration (min 0x7e3974, max 0x7e39b4, wheel center 0x7e39a8): the sticks are scaled onto it.
  The wheel's potentiometer reads high on the left (default min 0xff40 = left, center 0x7a40,
  max 0 = right): left goes to the calibration's higher end, right to its lower one. The test
  menu's CALIBRATION shows the wheel with a neutral calibration (min 0, center 0x8000, max
  0xffff): following its min / max there showed the wheel inverted.
- **Wheel kind.** td4.ini's `HANDLE` (KONAMI, HAPP, KEYBOAD), else the test menu's setting, else
  the cabinet default, HAPP for this build (a Happ wheel without motor, BOARD2 "NO DEVICE"):
  rva 0x48460 always answers KONAMI, the D44H motor board.
- **D44H motor** (`ac_io_d44h_set_senddata`, 4 bytes, GTI Club's HGTH layout): byte 0 the torque
  -127..127 (in a race +-80 against the wheel's turn: wheel right, torque negative), byte 1 bits
  0-5 a vibration level (crashes), byte 3 0..254 from the speed: the constant force and the
  vibration (`WAL_KONAMI_FFB` percent). `ac_io_d44h_directreq_sendandrecv` takes one argument
  (called every frame).
- **Boot CALIBRATION** (rva 0x49b30, line 7): the motor turns the wheel to each end stop (torque
  ramps to -127 / +127) and reads the potentiometer there, at least 50000 apart (ERROR
  otherwise); the result is the wheel's calibration. While it runs (step 0x7e3b0c 1-5) the
  emulated wheel follows the motor (negative torque: 0xffc0, positive: 0x0040) and no force
  feedback is sent. About 45 s, as on the cabinet.
- **D44S seat** absent: its requests fail, the boot shows SEATBELT OK ( NO DEVICE ).
- **DATA CHECK** sums every file of `data/td4check.bin` (32-bit word sums; libacio.dll and
  libtd4io.dll among them, so BAD with the payload) at a pace of tens of minutes for the 1 GB
  `td4pack.dat`: skipped as HOLD LEVER UP + START does (rva 0xd8a47).
- **Operator settings.** A fresh `CONF/NVRAM` stops at PLEASE SET THE CLOCK (flag 0x98f64c,
  getter rva 0x108640) and PLEASE CALIBRATION (steering, accelerator, brake flags 0x98f59c-e,
  getter rva 0x108610): both reported set (the PC clock; the sticks follow the calibration the
  game has).
- **Rendering.** The game draws 640x480 whatever its window: `WAL_D3D9_WINDOWED_SIZE: 640x480`
  in a borderless screen-sized window. Its window (the DirectX sample framework's) has a menu
  bar (File: Go/stop, Single step, Change device, Exit): `WAL_WINDOW_POPUP` removes top-level
  windows' menu bars (at creation and the game's `SetMenu`).
- **Input filtering.** DirectInput disabled (td4.ini's keyboard mode), `GetLogicalDriveStringsA`
  C: alone.

#### Wartran Troopers

Wartran Troopers (2005, game code D00, Direct3D 8, two 640x480 monitors, four guns) is
`D00/game/game.exe`; it imports no Konami I/O library: its cabinet is two serial links and
`panicdialog.dll` (the fatal error box), which the payload replaces (`payload_install`; the
message goes to the payload log). `wartran.rs`, `two_screens.rs`.

- **I/O chain on COM2** (115200 8N1): the "pcio" hub and the boards behind it. Packets start
  with `AA`, their checksum is the sum of the bytes after the first. The boot syncs (`AA AA AA
  55`, `AA AA 00 00` echoed), counts the boards (`AA AA 00 01 01 ..` -> `AA AA 00 01 01 n sum`,
  4), reads each one's id and firmware version (`AA AA n 02 00`, 28 bytes: id big endian at
  +11, version at +16: equal to the dump's firmware file, else the game uploads it) and status
  (`AA AA n 03 00`, 13 bytes). Boards: 1-2 id 0x201 (`D00.bin`, players 1-2 and 3-4: setup
  commands only), 3 id 0x601 (`C22D_1-0-0.bin`), 4 the hub (0x10000,
  `pcio-20030929-v108.bin`). Then `AA 00 node cmd data sum` requests: the line echoes them,
  the board answers `AA 01 node cmd reply sum`. The reply's length is taken from the game's
  request queue (I/O context `[0x5843b8]`, 0x80c-byte entries at +0x60c, the one sent at
  `[+0x608] & 15`, reply length at +0x808). Command 0 (reset) answers `1 0` (done, no error:
  zeros kept the 0x601 board's boot waiting, IOB initialized check and wait).
- **Switches.** Hub command 0x28 (66 bytes: 2, then the switch bytes): byte 0 START (P1 0x80,
  P2 0x20, P3 0x10, P4 0x08) and SERVICE 0x40, byte 2 TRIGGER (same bits), byte 1 TEST 0x02
  and the coin mech 0x01, bytes 4-5 the coin counter (big endian, the game counts its steps).
  Board 3's command 0x49 (17 bytes: 1, then data): bytes 12-14 OPTION, SHOULDER, SENSE of the
  four guns (bit 3 - player, active low); bytes 0-11 counters shown in a debug screen, 0. The
  test menu opens on TEST's press (an edge) during the attract, P1/P2 trigger move, P1 start
  executes; INPUT CHECK shows the four players.
- **Security plugs.** Hub commands 0x40 / 0x41 select the system / game plug, 0x48 reads its
  64 bytes: the game plug holds the region at bytes 3.. (11 known: `GND00JA`, `GKD00UA`,
  `GKD00EA`...; else SECURITY FAILED 2-1004-0000). `WAL_KONAMI_PLUG` (default `GKD00UA`, US
  English).
- **Gun boards on COM1** (19200 8N1, two units of two guns; silent: GUN BOARD INITIALIZE
  ERROR 2-1003-0006). A request's first byte is the unit | command << 2, written byte by byte
  (buffered to the request length the game set, `0x1a2f20c`); a reply (`0x1a2f210` bytes)
  ends with the complement of its other bytes' sum. Version (command 1): 1. Poll (command 2,
  7 bytes): both guns' raw x, y, their off-screen bits, 0x75 at byte 5. The game's gun read
  (rva 0xb2c00, `gun, &x, &y, &offscreen, &raw_x, &raw_y`) puts the raw bytes through the
  cabinet calibration and smoothing to 0..1 over both screens: replaced by the pointer, mapped
  to the picture's area in the window (off screen outside it, or with the `offscreen` control:
  the guns reload shooting off screen).
- **Two screens** (`two_screens.rs`, shared with Lethal Enforcers 3). `WARTRAN.INI` (absent) `NR_SCREEN` defaults to 2:
  with `GetAdapterCount` >= 2 the game creates one full-screen device per adapter in its own
  window (x 0 and 640), players 1-2 on the left screen, 3-4 on the right; with one adapter it
  plays on the left screen alone. IDirect3D8 reports two adapters (adapter 1 answered by
  adapter 0), both devices are windowed (copy swap, one back buffer) on the first screen's
  window, the second one hidden, and each `Present`s into its half of the 1280x480 picture,
  fitted in the borderless screen-sized window with its aspect. During 3D scenes the game
  slides each screen's picture 8 pixels outwards (rva 0x71f5 / 0x71fe, the cabinet's gap
  between its monitors): a black band in the middle of one window, the offsets made 0.
- **Settings drive.** The settings and bookkeeping are `<second fixed drive>\D00\GxD000.CFG`
  and `GxD001.CFG` (CONFIGRATION FILE IS NOT FOUND otherwise): `GetLogicalDriveStringsA`
  reports C: and E:, the profile maps E: to the dump's `E/`, seeded once with the dump's
  `D00/game/GxD00n.CFG` (free play).
- **Sound.** `sound.dll` plays through DirectMusic: the DirectMusic winetricks.
- **Input filtering.** `GetAsyncKeyState` (debug keys), `GetKeyboardState` (test menu
  shortcuts) answer no key, `GetCursorPos` a still mouse.

#### Lethal Enforcers 3

Lethal Enforcers 3 (2005, game code D22, Direct3D 8, two guns) is `game.exe` at the dump root;
it imports `iob.dll` (the I/O board) and `gunio.dll` (the two gun boards), both MSVC C++
(cdecl, mangled names in `libacio.def`). The payload replaces `iob.dll` (`le3.rs`); its
`DllMain` points `gunio.dll`'s exports at the payload (a jump at each entry: the game imports
both, gunio is mapped by then).

- **I/O board.** Boot: `iob_init(model, 1, 2)`, `iob_boot_wait` 1 (booted), no firmware update
  (`iob_check_need_firm_update` 0), the security checked (`iob_check_initial_security_checked`
  0: the original returns `-1` until done), `iob_check_error` 0. `iob_get_digital_in` fills a
  44-byte record each frame: +0 valid, +4 TEST, +5 the coin switch, +7 SERVICE, +8/+9 START,
  +0xa/+0xb TRIGGER, +0xc/+0xd OPTION (held), +0xe..+0x13 the same six as press counters (the
  game reacts to a change, not a level), +0x26 the coin counter (6 bits, the game adds the
  steps). Lamps, bass shakers, gun reactions, coin counters answer success.
- **Gun boards.** `gunio_boot_status` 1 (CHECKING GUN...OK; 0 waits, 2 BAD), no error, DIP
  switches 0. The game's gun read (rva 0x11d540, thiscall, player, `{x, y, off, raw_x, raw_y}`)
  turns `gunio_read_data`'s raw counts into the player's 640x480 screen through the cabinet
  calibration and smoothing: replaced by the pointer, x/y in screen pixels, off screen
  (-1, -1: the player takes cover, reloads) outside the player's screen or with the `cover`
  control (`b6`, the guns' off-screen shot).
- **Paths.** The data is read from `D:\D22\DATA\...`, the movies (MPEG-1, through DirectShow)
  from `D:\D22\DATA\movies\%s`: the 50 `D:\D22\` strings of the executable are rewritten
  in place to `.\` (the game runs from its folder). Settings, bookkeeping and rankings go to
  `F:\SAVE` / `E:\SAVE`: profile drives `E/`, `F/`.
- **Boot checks.** SYSTEM DATA CHECK1 (2-1005-0001) compares the CRC of `check.a`,
  `game.exe` and the five DLLs with `check.a`'s table: the payload's `iob.dll` fails it, the
  comparison (rva 0x136bb3) is skipped. CLOCK (PLEASE SET THE CLOCK BEFORE OPERATION) is the
  bookkeeping's "clock set" flag, set by the test menu's CLOCK screen (which sets the PC's
  time): the check (rva 0x136cb0) passes, the host's clock is right. GUN CALIBRATION (PLEASE
  CALIBRATE GUN) wants each gun calibrated: unused with the pointer, the check (rva 0x136cc0)
  passes.
- **1 or 2 players.** The cabinet has two monitors, one per player: the game takes
  `IDirect3D8::GetAdapterCount` (at most 2) as its player count and creates one full-screen
  640x480 device per adapter (the window resized to 1280x480). It always gets two, as Wartran
  Troopers (`two_screens.rs`): with one adapter a lone player's CPU rival (the "x m ahead"
  display) reads player 2's missing object at the stage start, a crash (rva 0xf2f9). Both
  screens side by side in one window (`lethal-enforcers-3-2p`, `WAL_KONAMI_PLAYERS=2`), each
  player aiming at their own (player 1 the left half, the other half off screen); or player 1's
  screen alone over the whole window (`lethal-enforcers-3`, `WAL_KONAMI_PLAYERS=1`: the second
  device does not present, player 2's START / TRIGGER / OPTION ignored; with
  `WAL_KONAMI_RIVAL_SCREEN=1`, GUI option "Show the rival's screen", both screens are shown, the
  CPU rival's run on the right). Stretched by default
  (`WAL_KEEP_ASPECT_RATIO=1`: aspect kept). Each game id has its own dump (the same files,
  one `.windowsloader` each: one image per version).
- **Direct3D 8 windowed.** `WAL_D3D9_WINDOWED_SIZE` on a Direct3D 8 full-screen device now
  clears `FullScreen_PresentationInterval` (1 here: D3DERR_INVALIDCALL when windowed), as the
  windowed fallback did.
- **Input filtering.** `game.exe` creates a DirectInput 8 device: the fake one without devices
  (`WAL_DINPUT_DISABLE`).

Headless tests: attract, coins, both players' start, name and stage selection by shooting,
play (scores, reload), test menu (TEST), both modes (1 player: the stage start with the CPU rival).

#### Cooper's 9

Cooper's 9 (2006, Direct3D 9, two guns, one 1280x512 screen) is `game.exe` at the dump root: a
memory dump of the HASP envelope-protected game. Its sections are the running image (the file
is the image size, the envelope's `.protect` section zeros once loaded), the import directory
points into `.protect` (empty: Wine resolves nothing) and the import address table (rva
0x112000-0x1123dc) holds the dumping machine's addresses (Windows XP DLLs at 0x7c80xxxx...,
envelope stubs at 0x0313xxxx). The entry point is the game's ATL `WinMain` (the C runtime
start-up had run when it was dumped). The game imports no DLL the payload could replace: the
profile installs it as `inpout32.dll` and starts the game through wal-loader
(`payload_loader: true`, a launcher option: wal-loader with the profile's first
`payload_install` path, from the game root when it has a folder, as the install); the game loads `inpout32` itself later (`LoadLibraryA`), the same
module. `coopers9.rs`, before the game's code runs:

- **Imports.** A real import directory is written where the header points (descriptors, lookup
  tables, names in `.protect`) and the 214 named slots resolved, so the common hooks find the
  game's imports as in any executable. The names come from the code calling each slot
  (arguments, results, strings passed), helped by a run of another loader's build (its rebuilt
  table read from memory, its addresses mapped back to Wine's exports). msvcr71's slots: from
  the calls (fopen "rb" / fread / fclose, sprintf, the ctype masks of the char classes,
  setlocale + _strdup, longjmp + _setjmp3, the CRT start-up's own...). Slots only the HASP
  library's code calls (bypassed) stay unnamed: a stub that logs the call.
- **WinMain.** Wine calls the entry with the PEB as argument: the instance. wal-loader restores
  the entry's first 5 bytes; the next ones (`push 0; push esi; push <module>`) jump to a thunk
  that sets esi to the image base first.
- **Data.** The cabinet mounted `data\data.bin` as Z: (`system("load.exe ...")`) and read
  `z:/data/bin/`: the dump has the files in `data\bin`, the path is made relative and `system`
  does nothing. Settings and bookkeeping (`expdat`, `extdat`, 2636 bytes each, rewritten by a
  save thread when they change) were in the game's folder: their name pushes point at
  `E:\expdat` / `E:\extdat` (`WAL_KONAMI_SAVE_DRIVE`, profile drive `e: SAVE`).
- **Dongle.** The HASP HL API is linked in: `hasp_login` (feature 0xffff0000), `hasp_logout`,
  `hasp_read`, `hasp_write`, `hasp_encrypt`, `hasp_decrypt` (rva 0xa3730...) are answered.
  The game checks the memory file 0xfff0: "GWP", A1 04 01, the region (index in "JUAKEGF",
  `WAL_KONAMI_REGION`, default E), 04, 0x64 at 9 and 15, 00 01 at 0x2e; a decrypted constant
  with 'C' at 0 and '9' at 7. It checks again at each coin.
- **I/O board** (parallel port, `Inp32`/`Out32`, the game's own poll thread). Commands: data
  port 0x378 gets the command (strobe on 0x37a), the reply is read there. 1/2: player 1's gun
  x/y, 3/4: player 2's (raw 0-255); 5: the shot (per gun nibble: trigger bit 0, on the screen
  bit 2; trigger without "on the screen": no position, the protect/reload shot); 6: triggers
  (bits 0, 4); 7/8 (status bit 5 picks): switches, bit 0/1 START, 2 TEST, 3 SERVICE, 4 COIN.
  Status port 0x379: first read 0x0f (board present, bits 1-2), then bit 7 low while a gun
  fired (the game then reads the positions), 0xdf otherwise. `(v << 4) | 0xd` / `| 0xe`: lamps.
- **Guns.** The game maps the raw positions through each gun's calibration (two points per
  axis) to 0-255, then x5 / x2 to the screen: the calibration is skipped (rva 0x19b57 jumps to
  the clamp), the pointer is reported over the whole picture. The boot's PLEASE CALIBRATE THE
  GUN (the settings' calibrated bit, rva 0x1dc00) passes. The light gun flash (the frame cleared
  to one color when a gun measurement starts, rva 0x190b0) is off (`WAL_KONAMI_GUN_FLASH=1`
  keeps it).
- **Movies.** AVI, MP42 (MS MPEG-4 v2) and WMV9, through DirectShow into the game's own
  texture renderer (RGB24). GE-Proton's winedmo decodes MP42 with FFmpeg but its WMV decoder's
  subtype list (and DMO registration) left MP42 out: quartz found no decoder, the movies stayed
  white. Fixed in the runner: GE patch `0098-winedmo-expose-mp42-video-decoding` (branch
  `winedmo-mp42-video-decoding` of ~/code/protonge); the WMV9 movies play on any runner.
- **Input filtering.** The game reads the keyboard through DirectInput 8 (debug keys: T test, S
  service, C coin...): the fake one without devices (`WAL_DINPUT_DISABLE`). `SetLocalTime`
  (the test menu's clock) does nothing.

Headless tests: attract (movies), coins, both players' start, mode selection by shooting,
story intro movie, play (hits, score, the protect shot), game over, test menu (TEST), saves.

#### Bishi Bashi Channel

Bishi Bashi Channel (2018, game code R66) is `contents/modules/bsch.dll`, a 64-bit AVS2 (2.17.3)
game DLL (Direct3D 9, AFP Flash UI), run by `wal-avs-boot64.exe` (the 64-bit build of
wal-avs-boot: AVS2's `avs2-core.dll` / `avs2-ea3.dll`, spice2x's 2.17 export names, one heap
for `avs_boot`, log writer `(chars, n, ctx)`; 192 MB heap, `WAL_KONAMI_AVS_HEAP_MB`). Four
players, each a red, a green and a blue button and a disk (a dial). The payload is the 64-bit
`wal_konami64.dll`, installed as `libacio.dll` and `libacio_pix.dll` (`bsch.rs`); spice2x's
`acio/bmpu`, `acio/pix`, `games/bbc` were the layout reference.

- **Boot checks.** The game finds `kbt.dll` with `GetModuleHandle` and calls its `k_bt0002`
  (which runs the game's init); the dump has none: the payload answers `GetModuleHandle` of
  `kbt.dll` / `kld.dll` with itself and exports `k_bt0001`, `k_bt0002`, `bt_get_ikey_status`
  (spice2x's stubs).
- **Boards.** BMPU (`libacio.dll`): a 56-byte status, test (`0xf0000000`) and service
  (`0x0f000000`) in each of its first four u32, the coin stock. PIX rvol (`libacio_pix.dll`): a
  60-byte status, the players' buttons by bit (player 1 red 44, green 107, blue 41; players
  2-4's red and blue active low), the disks' positions in bytes 20-23 (players 1, 3, 2, 4).
  The `libacio_pix.dll` copy's exports resolve the game's imports; the `libacio.dll` copy
  points them at its own (one state). HDXS (LEDs) and the newer ICCA calls (crypt init, card
  power, keep-alive) answered. COM1: the LCD monitor (as Winning Eleven 2012's).
- **e-Amusement off.** On (the dump's test menu setting), the boot stops on 5-2000 (no
  router). NETWORK OPTIONS are `/cabinet/net` in `dev/nvram/tm_net.xml` (Konami binary XML),
  e-amusement its first byte: cleared at start unless `WAL_KONAMI_EAMUSEMENT=1`. Off, the boot
  wants no network link (5-1507 "network connection detected": "OFFGETIP", from AVS's IPv4
  addresses, rva 0xdec30): skipped. The attract shows the local mode notice; free play.
- **Sound.** The game's WASAPI stream is exclusive mode (44.1 kHz, 16 bits, 5 ms); without it
  (winepulse.drv has no exclusive mode) the game was silent and its attract stopped on "Please
  wait" after the first demo. `WAL_WASAPI_SHARED=1` (payload-common `wasapi.rs`): shared mode,
  `AUTOCONVERTPCM`, one device period reported as the buffer, the game's full-buffer writes
  queued and given to the device as it has room.
- **Controls.** Blue starts a game and joins it (the attract's prompt); the disks from the left
  stick (or left / right). The entry screen's language choice is the card reader's keypad (not
  mapped).

#### The Bishi Bashi (IBB)

The Bishi Bashi (2009, game code IBB, Direct3D 9, 1360x768) is `launcher.exe` (the dump's name;
its "All Avatars (Chinese)" copy differs by two data bytes). Up to three players, each a red, a
green, a blue and a yellow button (yellow: start, join, and the mini games' "finish button").
It is built on Action Deka's framework: `libacio.dll` (HBHI board, card reader) is the payload,
the game's `libbms.dll` imports (BM-System) are pointed at the payload's functions of the same
names (`ibb.rs`; the UDP link between cabinets fails to open, the cabinet runs alone).

- **Switches.** The game reads 15 switches from the HBHI status (rva 0x6a0ff): switch `n` is
  player `n / 4`'s color `n % 4` (red, green, blue, yellow; the game's button `color * 3 +
  player`, table 0x49ae30). Switches 0-7 (players 1 and 2) are packet byte 8's bits; switches
  8-11 (player 3) analog channels 0-3, pressed below half way (`<= 0x7fc0`, released at full
  scale); then test (byte 1 0x20), service (0x10), coin mech (0x04, coins through the coin
  stock).
- **Clock check.** The boot stops on 5-2503-0000 "The clock is un-setting up" until the
  operator sets the clock (setting 0x1d): its `jne` (rva 0x2f940) becomes `jmp`, as Action
  Deka's.
- **Input.** The game's DirectInput keyboard (`data/xml/IBBconf.xml`'s debug keys) gets no
  device (`WAL_DINPUT_DISABLE=1`).

#### Dance Dance Revolution 2013 (MDX)

Dance Dance Revolution (2013, game code MDX, `MDX:J:A:A:2014032700`, Direct3D 9, 1280x720) is
`modules/mdxja_945.dll` (the dump's `.windowsloader`), run by `wal-avs-boot.exe` (AVS 2.15.8:
its `XCd229cc` export names, two heaps, the `(chars, n, ctx)` log writer). The dump also has
`mdxja_hm65.dll`, the P4IO cabinets' build. 945 does not import `libacio.dll`: wal-avs-boot
loads the payload from the game DLL's folder after it. The cabinet's boards (`ddr.rs`;
spice2x's emulation is the reference):

- **P3IO.** SetupAPI's P3IO classes (and P4IO's) answered with one device, `\\.\P3IO`
  (description "Generic Monitor": the HD cabinet), opened as `\\.\P3IO\p3io`; the inputs by
  `DeviceIoControl` (4 bytes, inverted), the commands as `AA <len> <data>` (`FF` escapes):
  version `JDX`, watchdog, lights, security plugs, cabinet type HD, video frequency, coin
  stock, and its serial port: an ACIO bus with the two card readers (ICCA, nodes 1-2) and
  the HD panel's lights (HDXS, node 3). COM1 the pads' lights (FOOT, `11` answers), COM2 the
  satellites (SATE, 7 `DDRS` ACIO nodes), COM3 the USB memory port ("not connected").
- **P4IO** (hm65): `AA <cmd> <seq> <len>` commands, 64-byte answers, 16-byte inputs, the pads'
  arrows from libacio's MDXF boards (`ac_io_mdxf_*`, a 16-entry ring per pad, the game's
  `arkGetTickTime64`). spice2x's P4IO plugs are later DDRs': hm65's SECURITY fails; 945 is run.
- **Security.** The plug's 32 data bytes: a 6-byte signature, then the game code in 6-bit ASCII
  (`c - 0x20`, four per 24-bit little-endian word). spice2x's black plug is DDR X's
  `GCJDXJAA` (libp3io logs `secplug:0 GCJDXJAA`): the payload signs `GCMDXJAA` with the game's
  own function (rva 0x138310: key = table rva 0x1abe08 ^ code ^ (0x7f - i), the reversed plug
  id; checked against spice2x's data first). SECURITY BAD 5-1505 remained: the boot check
  (vtable 0x101f2fe0, rva 0x20c00) also needs the dongles, `kbt.dll`'s `bt_get_ikey_status`
  (`kbt.rs`: both present).
- `/env/profile` ids: `WAL_KONAMI_PCBID`, `WAL_KONAMI_HARDID` (wal-avs-boot) override the dump's.
- **Codecs.** `k-clvsd.dll` and `xactengine2_10.dll` (movies, sounds) registered at start
  (their `DllRegisterServer`, regsvr32's work).
- **e-Amusement.** `WAL_KONAMI_EASRV=1` (the profile's default): the boot's NETWORK check
  passes, the game runs online. The `game` module is bemaniutils' DDR 2013 backend
  (`easrv/mdx.rs`): `common` (256 final event flags, 700 play counts), `shop`, `lock`,
  `tax_info`, `new` / `load` / `save` (the dancer: name, area, play counts, experience, stars,
  options, flags, groove radar, rivals by 8-digit code), `load_m` / `save_m` / `score` /
  `trace` (best score, rank, combo type, the last five scores, the best play's steps),
  `hiscore` / `area_hiscore`, `friend`, `load_daily`, `load_c` / `save_c`; kept in
  `dev/nvram/easrv/<refid>/MDX.kbin`.
- **Cards.** The game reads a reader's card (`0x131`) during the attract, inquires 4 s later,
  its PIN screen 3 s after (15 s): the configured PIN is typed on the reader's keypad 10 s
  after the read (numpad otherwise). A new card: the terms (menu left = yes, START), the
  DANCER NAME grid (90 s). Headless: login, a new dancer, a song (`save_m`), the game's end
  (`save`), reloaded at the next login.

#### Yu-Gi-Oh! 5D's Duel Terminal 6

Yu-Gi-Oh! 5D's Duel Terminal 6 (2011, US, Direct3D 9, 1280x1024) is `contents/exe/game.exe`
(the dump's `.windowsloader`): a card game cabinet played on a touch screen, a camera reading
the players' cards, a card dispenser. `libacio.dll` (HBHI board) is the payload (`ygo.rs`,
detected by `libpenteltp.dll`).

- **Drives.** The cabinet keeps the game in `D:\jd6`: D: is the dump root
  (`WAL_KONAMI_DDRIVE`, `..\..` from the executable), `jd6` skipped (`drive::skip_folder`).
  The AVS and media libraries (libavs, libgraph, libaep, libmv, sound, libamsd, ...) open their
  data themselves: their file functions are redirected too, and Wine's DirectShow quartz and
  winedmo (libmv's WMV previews). E: (`WAL_GAME_DRIVE`, settings `E:\CONF\NVRAM`) is the
  executable's folder.
- **Card dispenser.** The HBHI board's dispenser emulated (`hbhi::set_dispenser`): init and
  disburse finish (status 7), a card is always taken.
- **Touch panel.** Pentel, COM1, libpenteltp's protocol: the start command `81 02` answered with
  the two status responses (`C1 03 00`, `C2 0E 00 00` + 10 bytes), then one 8-byte packet per
  read until the stop command `82 02`: `82`, the touch flag (byte 1 bit 0), X and Y (14 bits,
  7 per byte, low first). The touch: player 1's right stick over the screen (the mouse or a gun
  drives `rx` / `ry`) and button 4, the 5:4 picture fitted in the screen.
  The dump's libpenteltp has two jumps of `CPentelTPObserver::update` (rva 0x25db, 0x2629)
  nopped: every packet after an untouched one was a touch edge whatever its flag and the
  release edge never came (one touch filled both calibration targets, the touch check showed
  nothing). The payload puts them back (bytes checked). Its poll parses one packet per call,
  also after an empty read: hence one packet per read.
- **Calibration.** A new cabinet stops at a notice until the panel is calibrated (test menu,
  TOUCH PANEL CHECK, test + service held 5 s: two targets). The panel being exact here, the
  game's calibration readers (rva 0x1b9b00 values, 0x1b9bf0 calibrated) jump to the payload's:
  with none saved, the two targets (64, 80) and (1216, 944) touched dead on, calibrated. One
  made in the test menu is used as is.
- **Camera.** `libcamera.dll` (DirectShow, none here): its imports answered, one 640x480 RGB24
  camera, blank pictures (no card under it).
- **Touch pointer.** `WAL_CROSSHAIR` (on by default): a small red dot where the mouse
  points (`crosshair.rs`, `WAL_CROSSHAIR_AXES=right-stick`).
- Headless: the attract, coins, mode select, Speed Duel's tutorial (a card dispensed, the duel).
- **Status.** User: 100%. Limits: no card scanning (the camera's pictures are blank: modes
  needing scanned cards untested); the dump lacks `data/sem0` (a few common sound effects fail
  to open, error 3); a black frame where a mode preview clip loops. The previews (segments of
  one WMV, seeked to) need the runner's winedmo.dll with GE patch 0103 (the WM reader's seek
  reads the file while flushing), else they stay grey after the first.

#### Hello! Pop'n Music (JMP)

Hello! Pop'n Music (2011, game code JMP, `JMP:J:A:A:2014122500`, Direct3D 9, 1360x768) is
`PMU/popn.dll` (the dump's `.windowsloader`), run by `wal-avs-boot.exe` (AVS 2.12 plain export
names, 30 MB heaps: spice2x's size for popn.dll). Two players, each an OK button (start) and
four buttons, left to right red, blue, yellow, green (blue / yellow move the menus' cursor).
`libacio.dll` (HBHI board, card reader) is the payload (`hpm.rs`; spice2x's layout).

- **Switches.** Packet byte 1: test 0x20, service 0x10, coin mech 0x04; byte 8: player 1's
  buttons bits 0-3, player 2's bits 4-7; the OK buttons are analog channels 0 and 1, released
  with the high byte's bit 0 set (pressed: 0). The test menu's INPUT CHECK shows them all.
- **kbt.dll.** The game calls `kbt.dll!bt_get_ikey_status` (found with `GetModuleHandleA` +
  `GetProcAddress`, rva 0x7a7e5) without checking it: a null call without the cabinet's
  library. `kbt.rs` (both builds, also Bishi Bashi Channel's) answers `GetModuleHandle` for
  kbt/kld with the payload, which exports the function (two dongles present).
- **DirectInput.** The game asks its DirectInput object for IDirectInputJoyConfig8 and calls
  its `GetConfig`: the fake DirectInput (`WAL_DINPUT_DISABLE=1`) refuses the joystick config
  interfaces (it answered itself, the call landed past its slots).
- `SetCurrentDirectoryW` is a no-op and `mixerSetControlDetails` always succeeds (spice2x's
  patches).
- **e-Amusement.** The test menu's NETWORK OPTIONS e-AMUSEMENT (off in the dump) is the u32 at
  0x28 of `dev/nvram/config.dat` (0x70 bytes, then their MD5): the payload sets it on with an
  e-Amusement server (`WAL_KONAMI_EASRV=1`, the profile's default, or `WAL_KONAMI_SERVICES`),
  off without, and rewrites the MD5. The loader's server (wal-avs-boot) answers the `game`
  module as the open-source Asphyxia CORE plugin `popn-hello@asphyxia` does (`easrv/jmp.rs`;
  answers `<game status="0">`): `common` (`flag id s1 s2 t`, `cnt_music` 36), `shop`, `new`,
  `load` (`last` chara/level/music_id/style, `chara id love`), `load_m` (`music` / `style` /
  `level score clear_type`), `save` (`last`, the character's `love` added), `save_m` (best
  score and clear); kept in `dev/nvram/easrv/<refid>/JMP.kbin`.
- **Cards.** A reader per player, devices 1 (player 1) and 2 (player 2): player 1's card
  (`WAL_KONAMI_CARD_ID`) in reader 1, player 2's (`WAL_KONAMI_CARD2_ID`, the GUI's second card,
  F5) in reader 2 (the same card in both: "同一カードは使用できません"). The game polls a
  reader (`req_uid`, `get_uid_felica`) from character select and its keypad
  (`ac_io_icca_get_status`) only while that reader's PIN screen is up: the configured PIN
  (`WAL_KONAMI_CARD_PIN` / `CARD2_PIN`) is typed 1 s after the keypad polling starts
  (`icca::set_pin_on_poll`); player 1's numpad types too. Then authpass, a new card's terms
  (はい / OK), `game.new`, `load`, `load_m`; each song `save_m`, the game's end `save`.
  Headless: two cards at once, each PIN typed, a full game saved and loaded back.

#### Metal Gear Arcade (I36)

Metal Gear Arcade (2010, game code I36, `I36:J:A:A:2011092900`, Direct3D 9, 1920x1080) is
`game/launch.dll` (the `.windowsloader`), run by `wal-avs-boot.exe`; it loads `system.dll`
(the game) and its modes (`bootmode.dll`, `testmode.dll`, `gamemode.dll`). The dump's `MGA.exe`
(a PyInstaller launcher registering the cabinet on savemga.com, a revival network whose hosts
no longer resolve), `spice.exe` and dgVoodoo (`D3D9.dll`, `DDraw.dll`, `D3DImm.dll`) are hidden.
`libacio.dll` is the payload (`mga.rs`; spice2x's `games/mga` and `acio/i36g` the reference).

- **Boards.** I36G, two ACIO nodes of 88-byte status buffers (`get_control_status_buffer(node,
  out)`; the `ac_io_i36s_*` names are the same functions, node 23 the head tracker of the 3D
  glasses, left empty): node 21 the cabinet (bits 42 coin mech, 44 service, 45 test, 124
  start), node 22 the gun controller (104 side lever, 105 side right, 106 front bottom, 107
  side left, 108 front top, 109 trigger, 124 top, 125 switch; bytes 42-45 the thumb stick's y
  and x, 16 bits centered on 0x7fff, forward 0). The test menu's INPUT CHECK shows them all.
  `ac_io_begin` takes `(port, device, version, out, flags)` here (one more argument than the
  other games): `acio.rs` finds the version pointer.
- **Gun aim.** COM1, the gun board (Team FrontLine gun I/O): a 1-byte command, the answer and
  a checksum (0xff minus the bytes). 0x08 the position (x, y 10 bits big endian, y 0 at the
  bottom: the dump's `calib.xml` L 0 R 1023 U 1023 D 0), 0x04 version, 0x10 frequency DIP
  switches, 0x14 / 0x18 / 0x19 / 0x1a set the frequency, 0x1c LED board check. Menus are
  answered by aiming and pulling the trigger (MG / credit choice, control type).
- **Boot checks.** bootmode.dll's device check (rva 0x19f30, a state machine; state 0 waits
  for `MGO::GetPhase() >= 2`) finds kbt.dll / kld.dll with `GetModuleHandleA` in each mode DLL
  (hooked as system.dll loads them: `kbt.rs`, now also kld's `bt_fcheck_init` / `_main` /
  `_finish`). Its e-AMUSEMENT CONNECTION CHECK (checker poll rva 0x2a00) waits forever on the
  services' error 2000 (the error table marks it fatal; no button continues it, with the
  test menu's e-AMUSEMENT option off too): its sub-state 2 goes to the non-fatal path
  (`GsNetwork::SetEaLicense`, then the network ready flag) with result 1. Skipping the check
  altogether is wrong: ess.dll (e-Amusement services) is started there, and the first credit
  call later blocks on its uninitialized lock. The game then runs in offline mode (the
  "オフラインモードで運営中" notice). With the fake e-Amusement server (`WAL_KONAMI_EASRV=1`, the profile's
  default) the check passes for real and the patch is not applied.
- **e-Amusement server** (crate `wal-easrv`, run by wal-avs-boot before `ea3_boot`: MGA asks for
  the services at once, before its system.dll loads the payload). This older client posts text
  XML (`EAMUSE.Httpac/1.0`, HTTP/1.0, LZ77): `kbin.rs` reads it too. `I36.system.getmaster`
  (spice2x's `S_SRVMSG` / `INFO`, the unlocks; `MAPPAT`, `RK_TERM` `result` 0 as bemaniutils'
  MGA backend), `playerdata.usergamedata_scorerank` (`result`, `rank/rank` s32,
  `rank/updatetime` u64, read by ess.dll), `usergamedata_recv` with the player as `eaid`
  (records `PLAYDATA`, `OUTFIT`, `WEAPON`, `MAINRULE`).
- **Card reader** (`icca.rs` compat mode, spice2x's): a touch reader, its status START (2) while
  a card is on it, the id at byte 2; a touch lasts 2 s (the game then waits for the card to
  leave: "e-AMUSEMENT PASSを忘れずに..."). F4 touches the card; with `WAL_KONAMI_CARD_PIN` set,
  START too (the attract's START opens the player entry). The PIN is asked on the reader's
  keypad (spice2x's key codes: level at 12 << 16 and 20, edge at 16): the configured PIN is
  typed by the reader 10 s after the first touch of a login (later touches do not restart it;
  the login before the PIN screen), again every 4 s until 30 s; or by the player on the
  numpad (`kp0`..`kp9`, player 4's numpad keys off in the profile). Headless: touch, PIN, terms (YES by gun), the card player's MG screens.
- **Sound.** spice2x's patches: the sound engine failure (rva 0x571852, error 500) ignored,
  `setvolume_MGA_dll.dll!setvolume` not called (the cabinet's amplifier).
- **Credits.** The coin mech switch shows in the INPUT CHECK but adds no credit (the game's
  coins come another way, not found): the dump is set to free play in the test menu (COIN
  OPTIONS, saved in `dev/nvram`), e-AMUSEMENT off in NETWORK OPTIONS.
- **3D.** The game draws line-interleaved stereo (`mga_real3d_oddeven`, the cabinet's passive
  3D monitor, `app-config.xml` `draw3d_cx/cy` 1920x540 per eye) by default. The caution screen
  names START the "Start / 2D⇔3D" button; in the tutorial START opens the 3D level selector
  (1-10, default 5). Not forced to 2D yet.
- `WAL_KONAMI_AVS_LOG=1`: wal-avs-boot ignores launch.dll's `log_change_output` /
  `log_change_level` (the game sends the AVS log elsewhere otherwise; spice2x's fix).

#### Steel Chronicle Victroopers (KGG)

Steel Chronicle Victroopers (game code KGG, `KGG:J:A:A:2016062901`, Direct3D 9, 1920x1080) is
`arkkgg.dll` (the `.windowsloader`), run by `wal-avs-boot.exe` (AVS2 2.16.7: `libavs-win32.dll`
with `XCnbrep7...` export names, spice2x's numbering, one heap). arkkgg.dll's I/O library
`lib_acio.dll` drives `libacio.dll`, the payload (`sc.rs`); the game's code is `gamekgg.dll`
(Direct3D 9, the window, DirectInput), which arkkgg loads in `dll_entry_init`: the common
payload starts then, with gamekgg.dll as the game module (arkkgg's own lookups, kbt/kld,
`setvolume`, the LCD, are hooked on arkkgg.dll from `DllMain`). spice2x's `games/sc` and
`acio/hbhi` are the reference.

- **Locale.** The game's text is Shift-JIS throughout its DLLs: the profile runs it with
  `LC_ALL=ja_JP.UTF-8` (code page 932; the error screens are mojibake otherwise; spice2x: other
  locales crash).
- **Fonts.** Its fonts are TrueType collections (`data/common/font/df-heiseigothic-w*.ttc`)
  added with `GdipPrivateAddMemoryFont`: Wine's GDI+ parses plain TrueType only, adds no family,
  and the game then reads an uninitialized family count (crash in `GdipCloneFontFamily`). The
  dump's native gdiplus.dll is not used (a KnownDLL in the prefix). The payload adds each face on
  its own: a copy of the collection with that face's table directory at the start (its table
  offsets count from the file's start).
- **HBHI.** Two flight sticks: packet byte 1 test 0x20, service 0x10; byte 8 L 0x01, L1 0x02,
  L2 0x04, jog left 0x08, jog right 0x10, R 0x20, R1 0x40, R2 0x80; analog channels 0-3 the left
  stick's x (inverted) and y, the right stick's x (inverted) and y. Coins: the board's coin stock.
  `ac_io_get_version` answers firmware 2.0.0 (spice2x's answer, every game).
- **Touch screen.** libtouchpanel3m.dll reads `\\.\TwTouchDriver` (`ReadFile` until nothing is
  left, at most 100 reports): 24-byte reports, type 5, status at 12 (1 touching, 3 released), x
  and y at 16 and 18 (spice2x's factory calibration: offsets -1500 / 3100, both axes flipped). A
  serial streaming device in libtouchpanel3m.dll (serial.rs now hooks each install's modules):
  player 1's `start` at the pointer `accel` / `brake` (the profile's mouse or gun). Without it:
  I/O ERROR 5-1551-2399 (touch panel). The pointer is shown as a small red dot (crosshair.rs: `WAL_CROSSHAIR_AXES=pedals`,
  style `disc`, `_COLOR`, `_HIDE=0`; GUI "Touch pointer" and its size).
- **LCD monitor** on COM1: Winning Eleven 2012's handler.
- **Test menu settings** (`dev/nvram/testmode-v.xml`, a CRC-32 in `testmode-v.crc`): the payload
  marks the touch panel and the sticks calibrated (5-2506-0000 otherwise; the factory
  calibration spans the payload's full ranges), the clock set (5-2503-0000 otherwise) and free
  play (`WAL_KONAMI_FREEPLAY`, default on), and rewrites the CRC. `dev/raw` (AVS's log and share
  cache) is created.
- **e-Amusement.** The boot's network check (5-2000-0000 "cannot communicate with the router",
  no button continues) wants Konami's servers: `WAL_KONAMI_EASRV=1` starts the payload's fake
  server (`easrv.rs`, 127.0.0.1:33701; wal-avs-boot points `/ea3/network/services` at
  `http://127.0.0.1:33701/service/`: `localhost` and `/` are refused, "bad services host /
  URI"). Requests are `?model=..&f=<module>.<method>`, encrypted (`X-Eamuse-Info`): the
  answers must be too ("crypt level not match" otherwise). Answers: spice2x's (services,
  pcbtracker, message, facility, pcbevent, package, tax, eventlog, machine control, KGG
  `system.getmaster` / `hdkoperation.get`), built at start as binary XML (`kbin.rs`: the
  kbin writer, its byte / word packing of small values; `easrv/answers.rs`) and RC4-encrypted
  for spice2x's `X-Eamuse-Info: 1-53d121c7-a8b3` header (key: MD5 of its 6 bytes and
  e-Amusement's secret). The game boots online (`boot_status: 2,0,0`) to its attract.
  `WAL_KONAMI_SERVICES=<url>` (the GUI's "External e-Amusement server"): an external
  server's services URL instead, wal-avs-boot sets it and the fake server is not started.
- **Cards and players.** The fake server decodes every request (`kbin.rs`: RC4 with the key
  of its `X-Eamuse-Info` header, MD5 of its 6 bytes and e-Amusement's secret; LZ77; the
  binary XML reader) and answers (`easrv/cards.rs`, `easrv/playerdata.rs`; data in the dump's
  `dev/nvram/easrv/`):
  - `cardmng`: `inquire` (an unknown card: status 112, the game registers it on its screens:
    terms, a PIN entered twice, `getrefid`), `authpass` (116: wrong PIN), `bindmodel`; cards
    and PINs in `cards.txt`. `WAL_KONAMI_CARD_PIN` (GUI "e-Amusement card PIN") registers the
    card of `WAL_KONAMI_CARD_ID` beforehand: the game only asks its PIN.
  - `system.convcardnumber`: the printed number (`data/card_number`, `result` s32: the ess.dll
    receiver fails without it).
  - `playerdata.usergamedata_recv`: `result` 0, `player/record/d...`, `player/record_num` (0:
    a new player; `result` 1 ends the session). ess.dll sends a record as `<mask hex>,<name>,
    <field hex>,<columns...>` (encoder rva 0x2bf40) and reads it back as `<field hex>,
    <columns...>`, one per bit of the asked mask (decoder rva 0x2b290, bit 32 none): `d` is the
    record without its first two fields, its `bin1` kept (Asphyxia's and bemaniutils'
    answer; sent whole, every column was read one place off);
    `usergamedata_send`: each `d` stored under its name (its base64 text's second field:
    `rl1_bas`, `rl1_cus1`... 35 records, `<refid>/<name>.kbin`); `usergamedata_inheritance`
    (the previous version's `lt2_*` / `lt1_*` records) and `usergamedata_condrecv` (rankings):
    no records.
  - KGG's own modules, their fields read from arkkgg.dll's receivers: `group.getid`
    (`dataid`, `groupid` s32, `date`), `climax.load` (`recordcount` u8 0),
    `soundvoltex.printcount` (`playcount`, `printcount` s32), `soundvoltex.playcheck`.
  - `WAL_KONAMI_EASRV_LOG=1`: every request and answer as XML in `easrv.log` (the protocol of
    a game's next module). Headless: a card logs in, a new player is created (the S.I.V.A.
    story), a second login loads it. `machine.testmodev_send` (the operator settings) is still
    refused by the game's receiver (no effect seen).
- **Flow (headless tests).** Touch → e-AMUSEMENT PASS (SKIP: guest) → story → "Charge Your STE"
  (1-5 credits, touch) → ... (not driven further headless).

#### Silent Scope: Bone Eater (NDD), Widescreen Edition

Silent Scope: Bone Eater (game code NDD, 64-bit, Direct3D 11) runs as its community Widescreen
Edition: the dump's `game/BoneEater.exe` is that edition's runtime (natemac's, GPL-3.0, spice2x
with its own hooks; source at github.com/natemac/SilentScope-BoneEater-WidescreenEdition). It
runs `arkndd.dll` / `gamendd.dll`, emulates the cabinet's boards itself (the gun on the BMPU
status buffer) and draws the portrait main screen, the front LCD and the scope on one 16:9
picture. The `.windowsloader` names `game\BoneEater.exe`, the profile gives it the edition
launcher's arguments (`launch-settings.json` defaults). The payload (`ndd.rs`) is its
`dxva2.dll` import (override `n,b`, the one function forwarded to the system's), loaded before
the runtime's `main`:

- **Setup.** The edition's launcher (`Bone Eater WS-Edition.exe`) refuses the run directory's
  links ("Redirected user-data path refused"); the payload does its setup: `../user` gets a
  copy of the dump's `conf`, `avs-config.xml` with the nvram / raw mounts pointed there,
  `layout-v1.txt`, the cabinet marker `conf/raw/dx` (widescreen aiming), the gun calibration at
  the full 0-4095 range in `testmode-v.xml` (CRC-32 rewritten), the clock set, free play
  (`WAL_KONAMI_FREEPLAY`).
- **Keys.** The controls (`../user/bone-eater-controls.xml`, rewritten each start) are spice2x
  naive keys, one per native input (F13-F23, Q for the scope); `GetAsyncKeyState` /
  `GetKeyState` / `GetKeyboardState` answer them from the virtual sticks, every other key
  released (runtime, arkndd.dll, gamendd.dll); `-inputfocus never`.
- **Gun.** Gun X / Gun Y unbound: the runtime reads the cursor over its main window;
  `GetCursorPos` returns player 1's left stick across the largest visible window of the process
  (not the `MultiDisplay` scope outputs), `SetCursorPos` does nothing.
- **Scope.** The edition makes tap (2x) / hold (10x) from its scope key's window messages
  (`--scope-bindings Q`): the scope button posts `WM_KEYDOWN` / `WM_KEYUP` to the main window
  with lParam bit 25 set; `PeekMessageA` / `GetMessageA` turn real keyboard and mouse button
  messages into `WM_NULL`.
- **Fonts, pads.** gamendd.dll's TrueType collections go to Wine's GDI+ face by face (`ark.rs`,
  shared with Steel Chronicle; otherwise the "No font loaded" assert), its XInput pads read as
  not connected.

#### Chase Chase Jokers

Chase Chase Jokers (2023, game code UJK) is `chaseproject.exe`, a 64-bit Unity (Mono, Direct3D
11) game; its code is managed (`ChaseGameScripts.dll`, `Scenes.dll`, `AioModules.dll`). The dump
also has a BepInEx setup (Unity Doorstop's `winhttp.dll`), hidden: the game runs as shipped. The
payload is the 64-bit build `wal_konami64.dll`. `ccj.rs`, `ccj_trackball.rs`, `ccj_pointer.rs`,
`ccj_mono.rs`; spice2x's `games/ccj` was the layout reference.

- **Local mode.** The cabinet's launcher hands the game `kamunity.dll` (AVS, e-Amusement,
  xrpc) with `-hp`; absent, `KamunityInstance.isActive` is false and every caller has its local
  path: no network, no card login (guest play), "UJK:**kamunity disable**" in the corner of
  the attract. `-no_sioman`: no serial devices (COM1: card reader, unused then).
- **AIO libraries.** The I/O board (BI2X, the TBS variant: `-tbs_mode` is forced on) is driven
  through `DllImport`s of `aio`, `aio-iob` and `aio-iob2_video`. Unity only finds them through
  the DLL search path, the game's folder first (not `Plugins/x86_64`, where the dump has them):
  the payload is installed there under the three names (`payload_install`). Three modules:
  their answers are stateless (opaque objects, node manager and controller always ready, the
  firmware update done at once); `libaio-iob2_video.dll`'s copy runs the payload.
  `aioIob2Bi2xTBS_GetDeviceStatus` fills the TBS INPUT at offset 4 of DEVSTATUS: DevIoCounter
  (a new value each read, else I/O error 1520-14 after 15 s), CoinCount (the game follows its
  steps), bTest, bService, bCoinSw, bHPDetect, StickX/StickY (u16; the game's X is the field
  `StickY`, high = left; its Y the field `StickX`, high = up; default calibration 0..65535,
  center 32768), bTrigger1 (dash), bButton0 (special, lit), bButton1-3 (attack, jump, slide).
- **Trackball** (`ccj_trackball.rs`). A raw input mouse named `VID_1241&PID_1111`: the game
  lists the raw input devices, subclasses its window (`ChaseProject`) and reads the device's
  `WM_INPUT` reports (relative motion). user32's win32u imports (`NtUserGetRawInputDeviceList`,
  `NtUserGetRawInputDeviceInfo`, `NtUserGetRawInputData`) add the device; a thread posts
  `WM_INPUT` with the device's report handle while the right stick turns
  (`WAL_KONAMI_TRACKBALL_SPEED` counts per 10 ms at full tilt, default 12). The real mice and
  keyboards' reports are emptied.
- **Touch screen** (`ccj_pointer.rs`). The main monitor is a touch screen; the menus read it
  as Unity's mouse (`SubTouchInputModule`). UnityPlayer's `GetCursorPos` answers the pointer
  (`accel`, `brake`: a gun's or mouse's position), `PeekMessageA` / `GetMessageA` drop the real
  keyboard and mouse messages and deliver the payload's (tagged `WM_MOUSEMOVE`,
  `WM_LBUTTONDOWN/UP` for the native `touch`), `GetAsyncKeyState` / `GetKeyState` only the left
  button. The title, the card screen's guest start and the credit button are touched; the
  special button decides in the later menus (stick moves).
- **Monitors** (`ccj_mono.rs`). The boot's I/O check wants a 1920x1080 main monitor and a
  1280x1024 sub monitor (5-1525, operable: Service goes on). Its `BootItemIO.processed` static
  is set through Mono's embedding API (`mono_image_loaded`, `mono_class_from_name`,
  `mono_field_static_set_value`) at the board's creation: the check only looks at the board.
  The sub monitor's picture (`-single_monitor` not needed: Unity sees one display) is not shown.
- **Matches.** In local mode the matching scene shows a debug menu (Single / Local / Global
  match, blank buttons); `-nomatchselect` picks the local match: the game hosts it, the lobby's
  timer fills it with COM players.
- **Test menu.** TEST opens it during the attract; TEST moves, SERVICE selects.

#### Rainbow Bomber Girl

Rainbow Bomber Girl (2022, `game.exe`, Unity 2017.2, Mono, 64-bit, Direct3D 11): its code is
managed (`Assembly-CSharp.dll`, the I/O board's `UsbIO.dll`). The cabinet's launcher
(BM-System) runs it with its system library's module handle (`-hp`); without it the game runs
as Unity started it, with its development stand-ins (`DefaultSystemImport_*`): free play, no
test or service switch, a watchdog that quits after a minute. The dump also has a BepInEx setup
(Unity Doorstop's `winhttp.dll`): the payload (`wal_konami64.dll`) takes that file's place
(`payload_install`, `dll_overrides: winhttp=n,b`): UnityPlayer imports winhttp, so the payload
loads before any of the game's code; its one import (`WinHttpGetIEProxyConfigForCurrentUser`) is
forwarded to the system's. `bbg.rs`, `bbg_system.rs`, `bbg_io.rs`, `bbg_screen.rs`, `mono.rs`.

- **System library** (`bbg_system.rs`). The game binds its system interface by export name
  (`SystemImport_System`, ...: `GetProcAddress(module, name)` for each of a module's functions).
  Once the game has created `SystemImport` (its stand-ins), a payload thread attached to Mono
  replaces the System, Io, Consume and Entry modules by the game's own bindings, initialized with the
  payload's handle (`IFSystemImportBase.Initialize`), the backing fields set with
  `mono_field_set_value`. System: boot checks passed, the watchdog's lock function called at
  once and its unlock never (the stand-in calls both: the game quit after ~40 s), `IsAsia`
  (`WAL_KONAMI_REGION=asia`: the export version's English texts and pictures, where it has
  them). Io: test, service, coin (`IoGetBtnPress`). Consume: credits (a coin is one credit; a
  game `WAL_KONAMI_PRICE`, default 1, a gacha 1; `WAL_KONAMI_FREEPLAY=1`), PASELI not offered.
  e-Amusement coin, network, logs and test settings keep the stand-ins.
- **e-Amusement card** (`bbg_entry.rs`, Entry module). Started alone (not by BM-System), the
  game's card entry takes its debug path: its manager (`GameEntryStateManager`) is given the
  card in player 1's reader (`icca.rs`: `WAL_KONAMI_CARD_ID`, F4, a configured card in from the
  start) as `cardID` and `debugCardType` (NEWCARD / CONTINUE), the PIN screen
  (`GameEntrtyState_PassAuthentication.debugPass`, else 0123) the card's PIN, from the Entry
  exports (Unity's main thread, every frame of the entry scene). The entry state returned in
  the game's numbering (its enum has DATA_VER_ERR before END). A new card: terms, PIN twice,
  card number (skip), name: the player created, an empty record (the game's defaults). A known
  card: its PIN, the record (`EntryGetPlayerData`). A configured PIN (`WAL_KONAMI_CARD_PIN`) is
  typed by itself: the keypads' digits move each time, the screen's `OnNum` is called digit by
  digit (`mCurrentState`: PassAuthentication 6, PassRegist 7). The game saves through NetworkV2
  (stand-in kept: lobby, messages); at `EntryPlayerSaveEndNotice` the payload rebuilds that save
  (`ProcessPacketBridge.CreatePlayerSaveCard`, `Marshal.StructureToPtr`: the native layout) and
  applies it to the record: class experience, title, last girl, play count, win streak,
  headphone volume, girls opened (OR), each girl's experience (+ the experience won, the rank
  from `GirlRankData.GetRank`), chat set, skill deck, dress. Not kept: skills, dresses and items
  won, missions, events, titles won, chats, play records. Records: `dev/nvram/bbg/<card>.bin`
  in the run directory (`WALBBG1`, the PIN, the 19232-byte `READ_PLAYER_DATA`; layouts from
  Mono's own `Marshal.OffsetOf`). Ejected card: guest play. `WAL_SCREENSHOT=<s>`: Unity's
  `ScreenCapture` (tests: the composed picture is black in headless gamescope captures).
- **I/O board** (`bbg_io.rs`). UsbIO's `IoManagerMain` talks to the board on COM6 (System.IO.Ports:
  its ANSI `CreateFile` `DllImport`, resolved through Mono's `GetProcAddress`, opens no COM
  port: the board's boot fails at once). Every 10 ms the payload builds the board's 16 status
  bytes (UsbIO's `IoState`, bit fields from the most significant bit: stick channels 1 and 2,
  12 bits each, test / service / coin, byte 9: the three rows of three buttons, push buttons,
  headphone jack, the stick's button) and calls `IoState.SetDataArray` and
  `IoManagerMain.SetBtnState` (the board's packet thread's job), then marks the boot done
  (`m_BootStep` BOOT_SUCCESS_END). The game's default stick type (TYPE_2) reads the stick turned
  a quarter (X = 4096 - channel 2, Y = channel 1): the channels follow `m_StickType`.
- **Touch screen.** `TouchPanelManager`'s touches came from a cabinet library the game no
  longer has: the payload writes touch 0 (`arrPointerInfo[0]`: DOWN on the press, UP on the
  release, the position in the panel's coordinates: x 0..1280, y 0..-720 from the top), the
  pointer being a gun's or the mouse's position over the touch screen's place on the screen.
- **Screens** (`bbg_screen.rs`). The cabinet drives a portrait main monitor and a landscape touch
  screen mounted on its side as one 2160x1920 desktop; the game lays its cameras out for it (at
  the screen's own size its touch screen UI no longer fits its picture). UnityPlayer is shown a
  2160x1920 desktop (screen metrics, monitor info, the current display mode, its window's client
  and window rectangles, `WM_SIZE`, its maximized size), the game runs windowed
  (`-screen-fullscreen 0`, `Screen.SetResolution`'s full screen flag dropped: its internal call
  is wrapped at `mono_add_internal_call`), its window kept the screen's size, borderless
  (`WAL_WINDOW_SIZE=screen`, `WAL_WINDOW_POPUP=1`; every resize clamped at
  `WM_WINDOWPOSCHANGING`). The game draws into a 2160x1920 texture of the payload's own
  (`IDXGISwapChain::GetBuffer` / `ResizeBuffers` / `GetDesc` in DXVK's swap chain vtable), the
  swap chain's real buffers the window's size; before each `Present` the picture is drawn for
  the screen (shaders compiled with `d3dcompiler_47`, in a device context state of its own):
  the main monitor upright at the screen's height on the right, the touch screen turned back
  upright at the bottom left, above it the marquee of the dump's `bezel.png` stretched to the
  space left (`WAL_KONAMI_LAYOUT=main_left`: mirrored, no bezel; `raw`: the game's picture),
  a ring where the pointer is on the touch screen.
- **Input.** The real keyboard and mouse never reach the game's development keys: UnityPlayer's
  window messages dropped, raw input reports emptied, `GetAsyncKeyState` / `GetKeyState` 0.
- **Locale.** The game reads its data tables (CSV) with the user's number format: under a
  locale with a decimal comma (French) the game stopped at its start: `LC_ALL=en_US.UTF-8`.

### 6.3i Arcade PC (`crates/payload-arcadepc`)

PC-based arcade games without a cabinet I/O API to replace. System `arcadepc`, 64-bit payload
`wal_arcadepc.dll` installed as `xinput1_3.dll`.

#### Galaxy Rangers

`WXR.exe` (Unity 2019.4.10, Mono, 4 players, ENGLISH_NT_4P_V5.24.40411). The cabinet aims with
camera guns (`GUNSDK`: MindVision/JHCap cameras, `Calibrate.dll`) and a serial I/O board
(`BaseCom`: coins, the operator buttons, tickets). The dump's `Assembly-CSharp.dll` is a
community build (DemulShooter's "Modded Dump", MD5 860ed661…; the original and decrypted
assemblies are kept beside it) whose `ArcadeControls` class replaces both, configured by
`Controls.ini`: per player an XInput pad (`DllImport("xinput1_3.dll")`) whose left stick is the
aim as an absolute position (-1..1 over the screen, no dead zone, Unity's y up), RT shoots
(held: repeated at the weapon's rate), LB reloads, RB changes the weapon, START starts, BACK
inserts a coin (on their press); or a raw input mouse. `F2` opens the settings (`IntoSetting`),
the arrows browse them (down next, up previous, right/Enter increase or select, left decrease).

- **Pads (`pads.rs`).** Four pads always connected, from the virtual sticks: `lx`, `-ly` the
  left stick (the launcher's gun position), `trigger` RT, `reload` LB, `change` RB, `start`,
  `coin` BACK. A shot with the gun off the screen (`offscreen`, the gun's off-screen source on
  b6) is a reload, as the cabinet's guns. `XInputGetCapabilities`, battery, audio GUIDs answered;
  no keystrokes, rumble ignored.
- **Inputs from the launcher only (`payload-common`'s `input_filter.rs`).** The game reads the keyboard through Unity
  (window messages, raw input) and `ArcadeControls`' own `GetAsyncKeyState` polling of every key
  each frame and raw input thread (keyboard and mice). Wine's user32 functions are jumps through
  its win32u imports (`GetAsyncKeyState` → `NtUserGetAsyncKeyState`): hooked in user32's import
  table, every caller (UnityPlayer's imports, the Mono `DllImport`s) gets ours.
  `GetAsyncKeyState` / `GetKeyState` / `GetKeyboardState`: only player 1's operator keys (natives
  `menu menu_up menu_down menu_left menu_right` = F2 and the arrows, from test and the
  directions) are down; `GetRawInputData`: keyboard reports release no key (virtual key 0xFF),
  mouse reports move nowhere without buttons; UnityPlayer's `PeekMessage` / `GetMessage`:
  keyboard and mouse button messages become `WM_NULL`.
- **Early load.** Unity's input tries `xinput1_4.dll`, then `xinput1_3.dll`: the profile
  disables xinput1_4 (`dll_overrides`), so UnityPlayer loads the payload at start (before
  the Mono class's first call), its own XInput devices being our pads.
- **Display.** The profile's `Controls.ini` (`files`): 1920x1080 full screen (Unity's
  borderless full-screen window, scaled to the screen; the dump's is a 1280x720 window), every
  player on its pad (`AimSource=XInput`, no mouse). Unity's log: `Player.log` in the run
  directory (`-logFile`).
- **Not done.** Outputs (recoil, start lamps, life, ammo: DemulShooter reads them through its
  own Unity plugin), the operator's diagnostics overlay (F1), reloading `Controls.ini` (F5).

#### Top Gun Arcade / Quick Shot Arcade

SpiderZSoft's Top Gun ("BiuBiuGun", Unity 2019.3, Mono, 32-bit: the x86 payload
`wal_arcadepc32.dll`, installed as `xinput1_3.dll` too, xinput1_4 disabled), a portrait cabinet
(1080x1920) with one camera gun. Quick Shot Arcade is the American repack of the same game.

- **Dumps.** The original's `Assembly-CSharp.dll` is encrypted (decrypted by its packed
  `SpiderZSoft.exe`, which exits under Wine); the Quick Shot repack's (a Batocera Wine-prefix
  image, game in `drive_c/Quick Shot Arcade.pc`) is decrypted, with its `Main` changed to a
  608x1080 window. Both dumps are built from it, the repack's launcher, overlays and scripts
  removed, the executable renamed (`TopGun.exe` / `TopGun_Data`, `QuickShot.exe` /
  `QuickShot_Data`: Unity names the data folder after the executable), and `Main`'s
  `SetResolution(608, 1080, false)` patched back to `SetResolution(1080, 1920, true)` (IL at file
  offset 0x144A3: `20 38 04 00 00 20 80 07 00 00 17`).
- **Portrait screen (`WAL_DESKTOP_SIZE=1080x1920`, `payload-common`'s `desktop.rs`).** Unity
  makes no window taller than the desktop (at 1080x1080 the UI, laid out for 1080x1920, was
  cut). The game's modules see a 1080x1920 desktop (`GetSystemMetrics`, monitors, work area,
  display modes) and their top-level windows are fitted onto the real screen (608x1080 at 656,0
  on 1920x1080, centered): Direct3D stretches the 1080x1920 back buffer into it. DXVK's
  `ResizeTarget` (`MoveWindow` from `dxgi.dll`) is hooked too, and the windows are subclassed:
  Unity's procedure puts its 1080x1920 back in `WM_WINDOWPOSCHANGING` and `WM_GETMINMAXINFO`
  (Wine's X size hints: the window manager kept the window 1080x1920), fitted again. Gun
  positions are mapped back (`desktop::from_screen`).
- **Top Gun: the cabinet hardware (`topgun.rs`, `WAL_ARCADEPC_BOARD=syfire`).** The dongle
  `Dog_Ch.dll` (cdecl `open_dog`, `do_something([0,1])` → `[1,0]`) and the SY Fire I/O board on
  COM1 (38400 bauds, Mono's `SerialPort`): board → PC `B5 B6 len cmd data.. sum` (cmd 3 coin,
  7 operator keys: bit 0 settings, 1 up/left button, 2 replenish, 3 down/right button, 7 refill;
  9 doors; 17 a shot at the game's pixels from the top left, (0, 0) off the screen; 6 tickets
  paid), PC → board `A3 A9 len cmd data.. sum` answered `4F 4B cmd`; cmd 5 (tickets to pay) is
  answered paid at once (else a ticket error). Mono resolves its `DllImport`s with
  `GetProcAddress`: hooked in `mono-2.0-bdwgc.dll`'s imports, the dongle and `SerialPort`'s
  kernel32 functions (`serial::replacement`) are ours. Settings: a short press of the settings
  key (held 3 s: refill all). Data: `D:\gametemp` → the dump's `NVRAM`.
- **Quick Shot: the repack's plugins (`demul.rs`, `WAL_DEMUL_PORT=33610`).** Its logo comes
  from a BepInEx plugin (`TopGunCanvasCheck`, `Overlays/QuickShotLogo.png`) that needs
  Argonlefou's DemulShooter plugin, which replaces the dongle, the board and the gun: the
  payload is that plugin's TCP client. Each read is one 37-byte packet: per player (4) the aim
  `x`, `y` (`f32`, the game's pixels, `y` from the bottom), bit masks of trigger, reload,
  action, weapon change, flags (bit 0 hide the crosshairs, bit 7 inputs from the packets); a
  shot off the screen aims at (0, 0). The coin and the operator buttons are the plugin's
  keyboard keys (`5`, `0`, arrows, Enter): natives `key_<name>` (`native_map`) posted to the
  Unity window as `WM_KEYDOWN`/`WM_KEYUP` marked in their `lParam`'s reserved bits, which
  `input_filter` lets through. Other DemulShooter plugins use other packet layouts (the
  Batocera `batocera-wine-guns` bridge lists them).
- **Crosshair.** The common crosshair option (`WAL_CROSSHAIR`, on by default in both
  profiles: the cabinet's camera gun has none) draws player 1's crosshair over the picture
  (`pointer_d3d11`, a ring with arms and a center dot).
- **Bezel (`WAL_BEZEL=<image>`, `payload-common`'s `bezel.rs`).** The image (GDI+, loaded at
  run time), fitted on the screen, is shown by four strip windows around the game's window
  (following it), none over the picture: a full-screen window behind the game was stacked
  above it by GNOME (black screen). Top Gun: `Layer.png` (the original dump's ReShade layer,
  its Gemini watermark removed); Quick Shot: `Overlays/Bezel.png` (the repack's).
- **Fonts.** The settings menu's texts use Unity's built-in `Arial.ttf`, the system's Arial:
  the registry names `arial.ttf` in `C:\windows\Fonts`, which held none (invisible texts). The
  launcher links the runner's fonts (`share/fonts`, as Proton installs them) there at each
  launch, the prefix's own kept.
- **Open.** Recoil and lamps outputs, a real gun on the box.

#### Happy Water Sprite II

SpyderZSoft's "SpriteT2" (yeruigame, Unity 3.5.6, Mono 2.6 `mono.dll`, 32-bit), two water guns
(analog aim, no trigger: the game opens a playing player's valve). "Monsters vs Water Sprite"
is a repack of the same game whose game code skips the repacker's start logo (`CLogo`, the
data folder's `logo1.png`), clears the credits at boot and draws its own loading logo (its
`sharedassets2.assets`) at 600x300. The dump is rebuilt from it without the repacks' BepInEx,
Doorstop, DemulShooter and launchers, with Happy Water Sprite II's `sharedassets2.assets` (the
Water Sprite 2 logo) and `WaterSprite2.exe` (the save path follows the executable's name).

- **Board (`commking.rs`, `WAL_ARCADEPC_BOARD=commking`).** `CommKingDeviceManager` on COM1,
  19200 bauds, the game's own overlapped kernel32 calls (`DllImport`, through Mono's
  `GetProcAddress`: `mono_imports.rs`, `mono.dll` too). Frames `AA len cmd data.. xor 55`
  (`len` = data + 2). Answered: online (1), ID (2), valve (8, player) and pump (9) receipts, water
  level (0x0B: normal), tickets (4 on: 5 player 1 every 150 ms). Sent (`serial::push`, which
  completes the game's pending `WaitCommEvent`): coins (3), keys (0x0E/0x0F: 0 start P1 / its
  big gun, 1 start P2, 2 set, 3 select (the system menu: set enters, select moves), 4..8),
  aim (0x0A: x1 y1 x2 y2 big endian, 40 + 900 * position, y from the top).
- **Calibration.** The game maps the aim through 13 points (sys.dat, the system menu's
  PositionCorrection): the dump's are 900 times each point's place, (0, 0), (450, 0), ...
  (225, 225)..., for both players.
- **Dongle and registration.** HASP HL (`hasp_windows_96625.dll`, the payload installed under
  that name and loaded by `wal-loader`): login, the read/write file (`000` + the text the game
  checks), the session's serial. The game's registration (PlayerPrefs `tmp1`..`tmp5`, MD5s of
  `xbly`, a GUID, the board's ID and the serial; its MD5 starts at 0x67452201) is written by the
  payload at start; Unity 3.5's PlayerPrefs strings are REG_SZ. `ExitWindowsEx` ignored.
- **Display.** 1920x1080 full screen (Direct3D 9); the game shows each gun's aim.

#### Mech Tornado / Fire Hero

ridong's "jixiesheqiu" (Unity 2017.4, Mono `mono.dll`, 32-bit), its I/O DLL `gameport.dll`
(MFC, a serial board behind a C++ object). The dump is rebuilt without the repack's BepInEx,
Doorstop and DemulShooter; the game's ini (`d:\jijiasheqiu.ini`: settings, calibration,
bookkeeping) is on the cabinet's `D:` (the dump's `D` folder).

- **Board (`gameport.rs`, `WAL_ARCADEPC_BOARD=gameport`).** The game's `dllInstance` imports
  the DLL's flat C API (cdecl, the object `GetInstance` returns as first argument): the payload,
  installed as `mecha_Data/Plugins/gameport.dll` and loaded by `wal-loader`, answers them through
  Mono's `GetProcAddress` (`mono_imports.rs`, for that module only): `init` 1, coins buffered per
  player (`GetBufferCoin<n>p`, `SubBufferCoin<n>p`), start keys, the guns' two buttons
  (`Get<P>Frie1/2`: one the laser gun, both the water), the operator's setting / left / right /
  ticket keys, the aim as 10-bit ADC readings (`GetWadc<n>pX/Y`, `Get<P>WadcX/Y`), every other
  getter 0 (no error, no warning), every setter (lamps, motors, pump, tickets) ignored.
- **Aim.** x 1 + 1022 * x, y 1 + 1022 * (1 - y) (0 is ignored); the game maps it through four
  corners per gun (the ini's `gun`, `sgun`, `sdgun` points): the dump's are (1, 1023),
  (1023, 1023), (1, 1), (1023, 1); in laser mode two points (`lgun`: (1, 1023), (1023, 1)).
- **Fire Hero** (Mrcq "xiaofang", same codebase, Unity 2017.4, 64-bit: the payload is
  `xinput1_3.dll`, xinput1_4 disabled, and the dump keeps its `gameport.dll`, whose functions
  Mono resolves to the payload's; every function of the module answered, its camera's
  `OpenCam`, `ShowCamWnd` included). Two water cannons: one button the water, both the bomb.
  The repack's game assembly (ini beside the game, English forced, the serial warning hidden)
  calls a community mod (`HeroFireInputs.dll`, its TCP inputs unused: the hardware path, the
  keyboard and mouse filtered); its backups, script and logs removed, `FireHero.exe`. The
  calibration corners are in the dump's `xiaofang.ini`.
- **Cabinet.** Mech Tornado's ini: laser mode (`mode=2`, the user's choice; 1 water: one button the
  laser, both the water; 0 the ball machine, 2 players), 4 players (`playernum=4`). The game
  draws each gun's crosshair.

#### Mech Dinosaur / Wisdom Zombies

JxSoftWare's "RobotDragon" and "ShotZembies" (Unity 2017.2, Mono `mono.dll`, 64-bit: the payload
as `xinput1_3.dll`, xinput1_4 disabled), four players: guns on pivots (Mech Dinosaur) or water
guns that shoot while their player plays (Wisdom Zombies). The dumps are rebuilt without the
repacks' BepInEx, DemulShooter, dgVoodoo and launchers; the databases (`D:\SettingData.db`:
settings and the guns' calibration, identity; `AcountData.db`, `RankData.db`) on the cabinet's
`D:`; `Game.json`'s per-seat gun angles zeroed (a skew for the cabinet's physical guns).

- **Board (`jxboard.rs`, `WAL_ARCADEPC_BOARD=jxboard`).** `PortManager` on COM1 (`Game.json`:
  115200 bauds), Mono's `SerialPort` (`mono_imports.rs`). Frames `3A len data[len] crc 3B`,
  `data[0]` the command, the game's CRC (CRC-16/MODBUS's table, bytes swapped at each step,
  from 0xFFFF; checked against the game's own frames). The board's command 1 (40 bytes) answers
  each read (at most every 8 ms): 24 key bits (0..3 triggers, 4..7 starts, 19 the water level
  fine, 20..23 menu, OK, up, down: Wisdom Zombies reads up and down the other way, its
  profile's `native_map`), 8 axes (x, y per player, big endian, 0..4094, y up), tickets left
  (0), coins since the last frame. The PC's frames (lamps, motors, tickets) are ignored.
- **Crosshair.** The games draw no gun cursor in play: the common option (Direct3D 11).
- **Maps.** The map select unlocks the maps as the player passes them
  (`SelectMapManager.ShowPassLevel`: `OpenLocked(1..4)`): the dumps' assemblies open all of them
  (each call's `ldc.i4.1/2/3` patched to `ldc.i4.4`; file offsets: Mech Dinosaur 0x5977B,
  0x5978E, 0x597AD; Wisdom Zombies 0x3FBCB, 0x3FBDE, 0x3FC17).

#### Top Gun 2

JXCompany's "ShotRedLight" ("Top Gun 2: War of Gun", V2.0.5_20250325, Unity 2017.2, Mono `mono.dll`,
64-bit: the payload as `xinput1_3.dll`, xinput1_4 disabled), a portrait cabinet (1080x1920)
with two guns seen by a camera. The dump is rebuilt without the repack's BepInEx, DemulShooter
plugin, Doorstop and ReShade (`dxgi.dll`, its shaders and presets), and the copy of the game's
assembly (`Managed/Original.rar`, the same file); the executable renamed (`TopGun2.exe` /
`TopGun2_Data`); the databases (`D:\SettingData.db`: settings, English; `AcountData.db`,
`RankData.db`) on the cabinet's `D:`.

- **Board (`jxboard.rs`, `WAL_ARCADEPC_BOARD=jxcamera`).** Mech Dinosaur's JX board, two
  players, its keys at other bits (player 1: trigger 0, left 1, right 2, weapon change 3, start
  4; player 2: start 5, trigger 6, left 7, right 8, weapon change 9; menu, OK, down, up 20..23:
  up and down swapped in the profile's `native_map`). The left and right buttons choose the
  modes and maps.
- **Dongle (`softkey.rs`).** `GameMain.CheckKey`: a "SoftKey" USB HID dongle (vendor 0x3689,
  product 0x8762) found with `SetupDi*` (ANSI) and `HidD_GetAttributes`, its EEPROM read with
  feature reports (`HidD_SetFeature` report 2: command 0x12, address, length, password;
  `HidD_GetFeature` report 1: status 0, the bytes): the length at 0, then `JXKJ88188188123`.
  Its `DllImport`s are the payload's (`mono_imports.rs`), `CreateFileA` and `CloseHandle`
  chained to the serial ports' replacement (Mono's `SerialPort` asks `CreateFileW`).
- **Camera guns (`jxcamera.rs`).** A camera over the screen (OpenCV's blob detector on a
  `WebCamTexture`) finds each gun's spot, mapped to the screen by `Game.json`'s `Srn*`
  settings: `WebCamTextureToMatHelperExample.GetSrcPos(i)` (the weapons' aim, Unity's pixels,
  y up) and `GetHPos(i)` (from the center: the name entry's cursors) are replaced
  (`Mono::replace`) by each player's gun on the picture. Unity 2017's Mono returns a
  `Vector2` in a buffer of the caller's, its address in RCX before `this` (RDX), the index in
  R8. The camera is never opened (none on the box): `WebCamTextureToMatHelper.Initialize()`
  and the `VideoCapture` settings (`SetCapture`, `SetNormalCapture`, `GetExposure`) do
  nothing.
- **Display.** `WAL_DESKTOP_SIZE=1080x1920` (the game's `SetResolution(1080, 1920, true)`),
  the common crosshair for both guns (the cabinet's guns have no sight; `pointer_d3d11`'s
  `more_players`: player 2's in blue), `WAL_BEZEL=bezel.png` (the repack's ReShade layer, its
  corner mark removed) with `WAL_BEZEL_FIT=1` (its opening, wider than the picture, fitted to
  the game's window). The window not managed (`WAL_DESKTOP_UNMANAGED`, `WAL_DESKTOP_ACTIVE`: a
  desktop's top bar pushed it down), so the payload is loaded at the start (`payload_loader`,
  `wal-loader64.exe`: before Wine's X11 driver), Unity's window messages filtered once
  `UnityPlayer.dll` is loaded.
- **Landscape (`WAL_UNITY_LANDSCAPE=1`, the profile's default; "0": the cabinet's portrait;
  `payload-arcadepc`'s `topgun2_landscape.rs`, the game's level test in `jxcamera.rs`).**
  This dynamic display patch is private to Top Gun 2 (`WAL_ARCADEPC_BOARD=jxcamera`);
  other ArcadePC games use their standard bezel behavior.
  The levels full screen on a 16:9 screen, not stretched; the menus, attract and loading
  screens as on the cabinet, a portrait column in the bezel. Before the common start, the
  portrait setup is undone (`WAL_DESKTOP_SIZE`, `WAL_BEZEL`, `WAL_BEZEL_FIT` removed from the
  environment, the bezel's image kept). `Screen.SetResolution(int, int, bool)` is replaced:
  the screen's size, full screen (the 4-argument overload, compiled: `Mono::compiled`); the
  cameras keep their vertical field of view, the 3D scenes take the wider aspect. The UI
  canvases are constant pixel size, laid out on 1080x1920 around the screen's center (at 1080
  high the levels' scores, time and messages were off the screen): `CanvasScaler.
  HandleConstantPixelSize`'s IL (patched before its first compilation) passes `Screen.height
  / 1920` to `SetScaleFactor` (1 on the cabinet's screen), the portrait layout in the screen's
  middle. The bezel is drawn on the game's picture (`images_d3d11.rs`): the image's parts
  left and right of its opening against the column (9:16 of the height), at the screen's
  height, black beyond; hidden in the levels: `GameMain._State` (the singleton
  `MonoSingletonManager<GameMain>.instance`) PLAYGAME, PAUSEGAME, GAMEOVER, ACCOUNT or
  PLAYERRANK, and no scene loaded for 1.5 s (`LoadManager.Instance.IsLoading`: the level's
  state is set as its loading screen shows, the screen hidden a second after the load).
  Headless: attract, select and loading screens in the bezel, the level full screen with its
  scores, time and "Free Play" lines, aim and hits.
  Tried on Top Gun Arcade (2026-11-05): its canvases are constant pixel size too, but its
  stages' 3D cameras render to a texture shown in the portrait UI and its targets are UI
  elements: the play stays a portrait column (the UI's red background beside it), not kept.

#### Zombie Crisis

SpiderZSoft's "zombie" (Unreal Engine 4.25, 64-bit, `zombie\Binaries\Win64\ZombieCrisis.exe`,
`spiderzsoft.exe` renamed),
two guns. The dump is rebuilt without the repack's launcher, its shipping exe and debug files,
the HASP runtime, the camera tracker's tools and the crash reports; the game's `GamePort.dll` is
the payload.

- **Board (`zombiecrisis.rs`, `WAL_ARCADEPC_BOARD=zombiecrisis`).** The game's
  `ADllActorGameport` loads `GamePort.dll` and calls its only export, `GetInstance`: a C++
  object used through its 113 virtual functions (`this` first). The payload (installed as
  `zombie/Binaries/Win64/gameport.dll`, loaded by the game) answers `init` 1, the coins
  buffered per player (got, subtracted, cleared), the start keys, the guns' `Frie1` (the
  reload; a shot off the screen) and `Frie2` (the trigger; player 4's trigger is its
  `Frie1`, slot 77), the aim in the screen's pixels (1920x1080, top left 0), the operator's
  setting / left / right keys; every other slot (lamps, tickets, motors, the camera) 0.
  Presses are latched until the game reads them.
- **Calibration.** The game maps the aim through six points per gun (the service menu's
  hardware adjust; `Saved/savegames/uiback.sav`: `arr_gun60`, `arr_gun61`, `Vector2D`s: top
  left, middle, right, then the bottom row): the dump's are the screen's own (0, 0), (960, 0),
  (1920, 0), (0, 1080), (960, 1080), (1920, 1080), the aim unchanged. The game draws each
  gun's sight.

#### Skull of Shadow

The "Boys" cabinet ("海盗传说", Unity 2021.3, Mono `mono-2.0-bdwgc.dll`, 64-bit; the repack's
`PirateWar.exe` is a stub calling `UnityPlayer.dll`'s `UnityMain`, the game code decrypted),
four guns. The dump is rebuilt without the repack's BepInEx, DemulShooter plugin, launcher and
logs; its `TopData.db` (SQLite) holds the settings and the guns' aim calibration. English:
`Value_Language` 0, and 0 the default `StreamingAssets/Top/TopMenu1.ini` applies at each start
(the dump's was 1, Chinese).

- **Board (`boys.rs`, `WAL_ARCADEPC_BOARD=boys`).** `Top.Modul.Uart.A` (`TopUart.ini`
  `UartType 1`) through `BoysUSB.dll` (hidapi), resolved by Mono's `GetProcAddress`: 32-byte
  reports, `AA`, the inputs at their `TopUart.ini` places (bit n: `1 << (n - 1)`), the frame
  count at 30, the complement of the sum of the 31 bytes before at 31. Byte 1 the operator's
  keys (up, right, left / service coin, menu / down), byte 2 the coins, 3 the triggers, 4 the
  start keys (player n: bit 8 - n), byte 10 the lasers seen (`GunShot`). The PC's reports
  (lamps, motors, tickets, the lasers' timing, `GameState`) are ignored.
- **Gun camera.** The cabinet films the screen (`JHCap2.dll`, 320x240 grey): each gun's laser
  spot is found when the board says its laser is on, undistorted (the lens's fixed model),
  mapped through `Boys/ConfigGun3.ini`'s screen corners, then through the service menu's
  five-target calibration. The payload's camera answers black pictures (no camera error, the
  Unicode `...W` names Mono asks first included); the tracking of a laser seen
  (`Boys.CamGun3.GunLogic.ProcessRayTracking`) is replaced (`Mono::replace`, once the game's
  `GunLogic` exists) by the payload's: every gun's place on the picture (0..1 from the top
  left) stored where the spot found goes (`RayPointTread`, seen: `IsGetRayPoint` 0.3 s), the
  board reporting player 1's laser on all the time. The dump's calibration
  (`UartMouse_5_<player>`: the targets at 25 % / 75 % of the screen, scale 0.5:
  X 250 750 500 250 750, Y 750 750 500 250 250) maps it unchanged.
- **Dongle.** The developer's switch `C:\Boys` (`HASP.EnvConfig.IsBoys`: no retool request,
  unlimited time): the payload creates it.

#### Hot Shots Fire Rescue

Dacheng's "UAF" framework (`FireHero.exe`, Unity 2019.2, IL2CPP, 32-bit: `GameAssembly.dll`,
the 2022-11-23 v3.3), four water cannons. The dump is rebuilt without the repack's launcher
(memory patches: dongle, keyboard and mouse controls), its scripts and its board trace.
IL2CPP resolves the game's `DllImport`s with `GameAssembly.dll`'s `GetProcAddress`, hooked
like Mono's (`payload-common`'s `mono_imports.rs`). The input places are scene data, read
from the live `UAF_ControlManager` (Il2CppDumper's `dump.cs` offsets).

- **IO board (`uaf.rs`, `WAL_ARCADEPC_BOARD=uaf`).** `io_board.dll`: `openIOBoard` 0
  (opened), `readIOBoard` a 32-byte report the game copies as is (`UAF_Board.UpdateIO`; an
  input is `dataReceived[byte] & bit`), `writeIOBoard` (lamps, motors, pumps, tickets)
  accepted. Byte 2 test 0x20, service 0x40, repair 0x80; byte 3 coins P1 0x01, P2 0x02,
  starts P1 0x40, P2 0x80, the tank's level switches (0x04 low, 0x10 normal, 0x08 high:
  filled 0x14, else "Add Water"); byte 29 coins P3 0x01, P4 0x02, triggers P1..P4 0x04 0x08
  0x10 0x20, starts P3 0x40, P4 0x80. The control board (`control_board.dll`) is not used.
- **SuperDog** (`api_dsp_windows.dll`, stdcall: each function pops its arguments): logged
  in, data file 65524 zeros: `CheckDog` reads a perpetual licence.
- **Aim.** The cannons' potentiometers (bytes 8..24) go through a 13-point grid calibration
  (`Save.ini`, bytes shifted by 7); `UAF_ControlManager.MoveGunController` (RVA 0x42E910,
  prologue checked) jumps to the payload's: the target is the cannon's place on the picture
  in the game's centered pixels (x -960..960, y -540..540 up), its "moved" flag set. The game
  draws each cannon's target ring.
- **Encoding.** `UAF_DataManager.Awake`'s `Encoding.Default` throws (IL2CPP has no code page
  encoding but UTF-8, `CP1252` or `CP936` alike): caught by the game, as on the cabinet.

#### Bullseye Crack Shot

UDC's portrait gun game (Unity 2019.4, Mono `mono-2.0-bdwgc.dll`, 64-bit), one gun. Its
`Assembly-CSharp.dll`'s method bodies are encrypted on disk; the game's own runtime decrypts
each body when it parses its header (the image in memory stays encrypted). A readable copy for
study: every method's IL through Mono's `mono_method_get_header` (the runtime's decryption),
patched back into the file at its body (all 4369 bodies, the headers unchanged).

- **No board.** The dump's game never opens its board (`SerialPortComm`: `$`-prefixed 10-byte
  ASCII frames, `@` separated: `$A5` the trigger, `$A6` the operator's keys, `$Z5` the gun's
  place, `$B5` the coins) and runs the developer's keyboard and mouse mode
  (`GameData.KeyBoard`, `InputCtrl.Update`): the left button shoots at the mouse, `1` inserts
  a coin, `U` the settings, `Y` their menu, `]` back, `A` / `S` / `D` left, OK, right. The
  dongles (SuperDog, HASP) are not checked.
- **Payload (`bullseye.rs`, `WAL_ARCADEPC_BOARD=bullseye`).** The gun is the mouse: its place
  on the window's picture (client pixels) posted as `WM_MOUSEMOVE`, the trigger as the left
  button, the keys posted (`input_filter`: `post_mouse`, marked in `wParam`; `own_mouse`: the
  real mouse's moves dropped too, UnityPlayer's `GetCursorPos` answering the posted place and
  its `SetCursorPos` ignored, since Unity reads `Input.mousePosition` with `GetCursorPos`).
- **Display.** The game asks a 450x800 window (`CompanyLogo`: `Screen.SetResolution(450, 800,
  false)`), but its UI effects (the shots' trails, sparks, the screen's cracks:
  `GameUI.ShowFire`, `KillLife`) are placed at screen pixels on a 1080-wide canvas: off their
  place at any other width. The game runs on a 1080x1920 desktop (`WAL_DESKTOP_SIZE`, fitted on
  the screen) and `Screen.SetResolution(int, int, bool)` is replaced (`Mono::replace`, the
  overloads picked with `mono_method_desc_search_in_class`): the same aspect fitted on the
  desktop's height, full screen (Unity makes no window taller than the real screen: windowed,
  1080x1200 on a 1920x1200 screen). The dump's cabinet art goes around it
  (`WAL_BEZEL=Bezel.png`, converted to RGBA: Wine's GDI+ does not read its palette; the
  bezel's `GdiplusStartupInput` laid out as its C struct, 64-bit included), with the common
  crosshair (the game draws none).
- **Dump.** Rebuilt without the repack's launchers (portrait bezel, cursor), its injected
  helpers (`BullseyeTrace`, `BullseyeOutputs`, Harmony, MonoMod) and the runtime's archive.

#### Frenzy Express

Uniana's scooter game (2001, `xwin.exe`, OpenGL, 32-bit): a handlebar, a brake lever and two
pedals the player pushes by turns. The dump is a community repack whose `xio.dll` (the I/O
library, rebuilt with VC140) reads the keyboard with `GetAsyncKeyState`; its `boot.exe` is a DOS
stub, `_Frenzy Express GameFiles/` the repacker's leftovers.

- **Payload = `xio.dll` (`xio.rs`, `WAL_ARCADEPC_BOARD=xio`, `payload_install`, x86 build).**
  `xwin.exe` imports a C++ class `XIO` (thiscall methods by their MSVC names: the x86 build adds
  `xio.def` to the xinput exports) and reads exported bytes after each `update` (once per
  16 ms tick, then its game logic): `g_bKeyImm` keys held and `g_bKeyExc` keys pressed since
  the last update (bit 0 start, 1 service, 2 test, 3 coin, 4 left, 5 right), `g_bHandle` the
  handlebar (0 left, 0x80 center, 0xFE right), `g_bBrake` (0x80 half, 0xFF full), per pedal its
  position change (`g_bLAccelDif`), whether it grows (`g_bLAccel`) and its position
  (`g_bLAccelEnd`). The game drives with the change of the pedal it expects next and switches
  pedal when the other one moves; its demo writes these bytes (exported data, writable).
  The game never reads the `XIO` object (16 bytes it allocates).
- **Inputs.** The pedals behave as the repack's keys: held, a pedal goes down 2 per update (to
  0xFE), released it comes back 2. `pedal` (b1, or `accel` past a quarter) presses them by turns
  (6 updates each, 18 at a light `accel`); `pedal_left` / `pedal_right` (b3 / b4) one each.
  The left stick steers (absolute), the d-pad moves the handlebar 0x20 per update (back to the
  center released) and is the left / right selection keys, as is the stick past half way.
  `brake` (b2) full, `brake_half` (b5), the `brake` axis analog. Real keyboard and mouse
  filtered (`input_filter` on the executable's messages).
- **Boot calibration (profile `WAL_PATCHES`, applied by the payload).** Every boot calibrates
  the handlebar (center, RIGHT, LEFT, release) then the brake (untouched, pulled fully,
  release): each prompt lasts 300 frames (5 s) and the value is read when it ends; "ERROR THE
  STEERING WHEEL" then "DEFAULT SETTING" unless left < right. The readings are patched to the
  payload's full range (handlebar 0xFE right, 0x00 left; brake 0x00, 0xFF) and the prompts to
  30 frames: no error, the title in about 22 s instead of 45.
- **Flow (headless):** calibration, hardware check, title, 2 coins a credit, START: how to
  play, player select (the handlebar), START, costume select, pedal select, intro, the race
  (about 54 km/h with `pedal`); test: the operator's menu (left / right, START).
- **Music.** `xshow.dll` plays `show/*.mp3` through DirectShow graphs and loops them with
  `SetPositions(0)`: hung on GE's winedmo parser until the runner hotfix
  `winedmo-parser-rewind-after-eos` (§5.3).
- **Not done.** Lamps and vibration (accepted, ignored).

#### Hog Wild

Uniana's police car chase (*Hog Wild: Chase Up!!*, 2003, `bin/Racing.CRK.exe`, Direct3D 8,
32-bit; the "neoFT3D" engine's `ft3d_*.dll`): a steering wheel, accelerator and brake buttons, a
gear lever, a view button. The dump is a community repack: its executable is cracked (the boot's
checks bypassed), its `Interface.dll` a wrapper reading the keyboard in front of the original
(`Interface.ORI.dll`), its `D3D8.dll` / `DDraw.dll` dgVoodoo (hidden: DXVK's Direct3D 8).

- **Payload = `bin/Interface.dll` (`interface.rs`, `WAL_ARCADEPC_BOARD=interface`,
  `payload_install`, x86 build).** The game imports the C++ class `CInterface` (thiscall
  methods by their MSVC names: `interface.def`, merged into the x86 build's exports), and
  `ft3d_GameEngine.dll` reads its fields inline (the handle's center and limits): the payload
  forwards the 17 methods the game imports to the dump's original library, the object stays
  its own. That library drives the cabinet's board on the parallel port through the PortTalk
  driver: its port routines (`Inputb` 0x15f0, `Outputb` 0x1650) jump to the payload's and
  `OpenInterface` (0x1280) answers 1 without the driver (their first bytes checked: another
  build is left alone, logged).
- **Board (by the control register written before a read of the data port 0x378).** `0x24`
  buttons A, `0x2E` buttons B (a bit at 0 = pressed), `0x26` the protection check (0: the
  cabinet's), `0x2C` the next bit, MSB first, of the handle's ADC0831 (a conversion started by
  the chip select, data bit 2); `GetADC0831Value` gives the reading minus 0x80 (right positive,
  the I/O check's -120..120). `Input_Type` 0..10: coin (A 0x80), start (A 0x04), service (B
  0x04), test (B 0x08), accelerator (A 0x40), gear high (B 0x02), gear low (B 0x01), brake (B
  0x10), view change (A 0x02); 9 and 10 are not read. Lamps (siren, start, view), the handle's
  motor and the vibration are written to the data port: ignored.
- **Inputs.** The left stick steers (the d-pad's left / right fully), b1, the d-pad's up or the
  accelerator pedal past a quarter accelerates, b2, the d-pad's down or the brake pedal brakes,
  b3 / b4 the gears (the game shifts by itself), b5 the view. Real keyboard and mouse filtered
  (`input_filter` on the executable's messages: it also reads the arrows, Enter and the keypad
  with `GetAsyncKeyState`).
- **Patches (profile `WAL_PATCHES`, applied by the payload).** The boot's protection reads C:'s
  volume serial number and label (`GetVolumeInformationA`, 0x433e8c; a result the cracked
  executable ignores): the call never returned in Wine (the main thread waited in
  `GetVolumeInformationW`, the screen black), it is skipped (`add esp, 0x20; xor eax, eax`).
  The executable builds its menu inputs from the keyboard and the board, the handle's menu
  moves from `main_GetHandleValue`'s result: the cracked build stores 0 there (0x433be4
  `mov [esi+0x10], ebx`), its menus moved on the arrows only; the reading is restored
  (`mov [esi+0x10], eax`): turning the handle selects (repeating faster the further it is
  turned), the accelerator confirms.
- **Picture.** The 640x480 full-screen device made windowed (`WAL_D3D9_WINDOWED_SIZE`) in a
  borderless window the size of the screen's centered 4:3 rectangle (`WAL_WINDOW_SIZE=aspect`),
  black sides (`WAL_BEZEL=black`). `ftu_GameWin.dll` sets style 8 on its shown window: no longer
  visible to Wine, the bezel's strips never found it; `WAL_WINDOW_POPUP`'s `SetWindowLong` hooks
  now cover the game directory's DLLs and keep `WS_VISIBLE`. Not managed by the window manager
  (`WAL_DESKTOP_UNMANAGED`): GNOME's top bar pushed the full-height window 32 pixels down.
- **Flow (headless):** warning, Uniana logo, attract (demo replay, title, rankings), FREE PLAY
  (the dump's `mainmenu.cfg`), START: how to play, player select, map select, the chase (auto
  gearbox, ~140 km/h); TEST: the operator's menu, I/O check (handle, other inputs, lamps).
- **Not done.** Force feedback: the motor bytes (a centering spring by the handle's position,
  vibration and collision levels: a low nibble with a direction bit) are not decoded.

#### Wasteland Racers 2071

DND Arcade's racer (`Arcade.exe`, Direct3D 8, 32-bit, dump root), its menus in scripts
(`Data/Menu/Main/main.dsf`), its cabinet a force feedback wheel and pedals on a serial board,
up to 8 linked cabinets.

- **Payload = `SDC2.dll` (`sdc2.rs`, `WAL_ARCADEPC_BOARD=sdc2`, `payload_install`, x86
  build, `payload_loader`).** The game loads the board's library (cdecl exports by name,
  `sdc2.def`) when `Inputs.xml`'s `UseForceFeedbackWheel` is 1 (the profile's `Inputs.xml`).
  `Init(port)` 0, `IsConnected` 1 (else "Force feedback board disconnected", the game ends);
  `GetAxisData(axis)` `(raw - 32767) / 32767`: 0 the wheel (used as is), 1 gas, 2 brake (read
  as `1 - value` and scaled by `Inputs.xml`'s `Min/MaxAxisY/Z`: the profile's 1..2, released
  0, pressed -1; `Min/MaxAxisX` must be in range, -1..1, or "Calibration values out of
  range"); `GetDigitalIn` 16 buttons active low (0 boost, 1 camera, 2..5 cheats, 7 gear, 8
  coin, 11 maintenance, 12 stop); `GetCoin`/`SetCoin`/`ClearCoin` the board's coin counter:
  the credits (free play's buttons set it to a game's price, the start takes it: a stubbed
  counter never started a game); the coin button adds one.
- **Force feedback.** The board's motor is a position servo: `SetForce` its strength
  (0..32767), `SetSteeringPos` the position it holds the wheel at (the effects of
  `Feedback Parameters.xml`: the road's cracks move it +-4500 every 27 ms at strength
  10000). Played as a constant force toward that position from the wheel's reading (full at
  8192 away), every 10 ms on change.
- **Link.** The socket of the cabinets' link (UDP port 2070) is bound with a length the game
  never sets: Windows reads the 16 bytes of the address, Wine copied that many and crashed
  (patched to 16, 0x2ed42). A game starts once the link has elected a host: the request goes
  to 255.255.255.255 and the cabinet answers its own, which Linux does not deliver back to the
  sender ("FINDING SERVER..." forever): the address string is 127.0.0.1 (0x2fd0b8), one
  cabinet.
- **Loading.** The map's object loader presents its loading bar after each object: under
  the 60 fps limit (`WAL_D3D9_MAX_FPS`) the load lasted minutes behind a full bar, the window
  not answering (GNOME's popup); no limit (the game times itself). `WAL_D3D9_WINDOWED_SIZE`
  broke its partial presents (the race drawn in the bar's rectangle): the profile's
  `D3DLocalConfigData.txt` makes the game draw 1920x1080 (the dump's 2560x1440), in a
  borderless screen-sized window.
- **Attract movie** (`attractMode.avi`, MPEG-4 in a windowless VMR-9, `RepaintVideo` every
  frame): Wine's VMR-9 used its device from two threads without `D3DCREATE_MULTITHREADED`,
  DXVK crashed ~12 s in: runner patch 0112 (GE branch `quartz-vmr9-multithreaded-device`).
  Start skips it.
- **Inputs.** Left stick / wheel steers, b1 or the gas pedal accelerates, b2 or the brake
  pedal brakes, b3 boost (start too), b4 camera, b5 gear, test the maintenance menu,
  service stop; keyboard arrows as axes, full in 0.5 s. Quick start: start (movie), start,
  start (car), left (LOW motion), start.
- **Flow (user):** movie, title (free play), car, track, motion select, loading (~15 s),
  race.

#### Polar Wars

Maiqiang's "JiDiDaZhan" (Unity 5.6, Mono `mono.dll`, 32-bit, `UnrealGame/JiDiDaZhan/` below the
dump root: `polar-wars.windowsloader`), a two-player ball cannon cabinet on the "UnrealIO"
board (`CommSetting.ini`: 32-bit protocol over USB).

- **Board (`unrealio.rs`, `WAL_ARCADEPC_BOARD=unrealio`).** `UnrealIO32.CommLogicMain` talks
  to it through `UnrealHid.dll` (cdecl `CreateInstance`, `Open`, `Close`, `Read`, `Write`,
  `SetReadTimeout`, `Release`): the payload, installed as
  `UnrealGame/JiDiDaZhan/JiDiDaZhan_Data/Plugins/UnrealHid.dll` and loaded by `wal-loader`,
  answers them through Mono's `GetProcAddress` (`mono_imports.rs`, that module only). One
  exchange = a 32-byte `Write` (PC `F5`: lamps, motors, tickets, sum) then a 32-byte `Read`
  (0 = done; board `FA`, sum): byte 1 the operator keys (setting / menu down, left, ticket /
  right, up), 2 a coin per player per answer, 3 / 5 a ticket / capsule paid, 7..22 sixteen
  ADC bytes (each gun's x, y as 16-bit big endian), 23..26 the 32 key bits (start 0-1, triggers
  2-3; the ball sensors, motor faults, water level 0). Tickets asked (PC byte 8 nibbles,
  capsules byte 12) are paid one every 10 exchanges.
- **Aim.** 0..4080 over the picture (y from the top). The game maps each gun through 11
  calibration points (`Save/CorrectionData<n>P.txt`, at 10/30/50/70/90 % of the screen): the
  profile's files put them at those fractions of 4080, a linear map (the dump's are a real
  cabinet's).
- **Gun version (profile `Settings/GameSetting.ini`).** The dump's `硬件版本=1` is the ball
  machine (real balls shot at the screen, a shot when the ball sensor fires after the trigger,
  no cursor); the profile's file is the dump's with `硬件版本=0`, the potentiometer gun: shots
  while the trigger is held, each player's cursor drawn by the game. Operator menu changes last
  until the next launch.
- **Last stage unlocked (`unrealio.rs`).** The scenario select (`PS_Selete.StateStart`)
  locks the fifth stage (`level[4].Lock(true)`: chains, played last). At the board's first
  call (a managed thread, before that method is compiled) the payload patches its IL
  (`Mono::il_code`, `mono_method_get_header`): `Lock(false)`, and `PS_SeleteLevel.Lock` (only
  called there) skips its colliders part, so a stage already played stays closed (`Init`). The
  bytes are checked first: another build stays locked (logged).
- **Boot updater.** The scene before the title flashes the board's firmware
  (`IOUpdate_Data`) with 64-byte bootloader frames: those reads fail, it gives up in about 3 s
  ("Cancel the update").
- **Inputs.** Real keyboard and mouse filtered (`input_filter` on the executable's messages:
  the game also reads keys, F9-F12, the mouse buttons as triggers and debug keys such as Space,
  PageUp autoplay). Two coins a credit; at the scenario select every player with credits joins.
- **Not done.** Outputs (start lamps, vibration, ball motors, water spray). A static "ERROR: IO
  Exception!!" label in the operator menu (no script toggles it).

#### Pigs Might Fly

Gamewax's redemption game (2009, `Sega/PigsMightFly/PigsMightFly.exe`, Direct3D 9, 32-bit, `.windowsloader`
below the dump root): one lever launches a pig at flying saucers, tickets won by what it hits. The
dump keeps the cabinet's `C:\Sega` tree, with Sega Amusements' `Shell.exe` beside the game (coins,
tickets, operator menu). The game runs without it (`gamewax.rs`, `WAL_ARCADEPC_BOARD=gamewax`, x86
build, loaded by `wal-loader`):

- **Board.** The HID device of vendor 0x1043 product 0 (the board of SEGA Rally 3), found with
  SetupAPI (`0x43eaa0`), named "GAMEPAD" from its collection (usage 1 / 5) and read by a thread
  (`0x43ef50`): 15-byte reports, bytes 6 / 7 the buttons (bit 0 start, 4 the lever, 12 TEST, 13
  SERVICE). All answered by IAT hooks; the lamps' output reports are dropped.
- **No Shell.** The game quits without a Shell window on its command line (`/h=<window>`): the
  profile's `/h=1` is a dummy, since `[system] SHELLENABLE=0` makes it send nothing (`0x4400e0`).
  In that mode SERVICE is a credit (`0x43e720`, on its release): the payload's coin.
- **Settings.** `WAL_ARCADEPC_INI=<section>/<key>=<value>,...` answers `GetPrivateProfileStringA`
  (`[system]` is `.\game.ini`, `[GameSettings]` `..\ShellData\GameSettings.ini`, the others
  `..\ShellData\ShellData.ini`); the common `WAL_FREEPLAY` is `Credit/Freeplay`.
- **Tickets.** Tickets are counted by the Shell's messages: without them the game stays on VENDING
  ERROR. Its free play path pays none (`0x43f238`, `0x43f420`: 5 frames, nothing owed); the patch
  `0x3F240:9090` takes that path with coins too.
- **Display.** The cabinet's monitor is on its side: `SCREEN_WIDTH` x `SCREEN_HEIGHT` 1280x768 and
  `RENDER_FLAG=5` turn the picture. Overridden to 768x1280 and 0, upright, windowed
  (`WAL_D3D9_WINDOWED_SIZE`) in a 3:5 borderless window, in the dump's `bezel.png` (`WAL_BEZEL_FIT=1`).
- **Not done.** The operator menu is the Shell's (`Sega/Shell/Shell.exe`, not run): the settings
  are the dump's `ShellData/*.ini`. TEST does nothing here.

#### Space Warp 66

Touchmagix's ticket game (`SpaceWarp.exe`, Unity 2019.3, Mono, 64-bit, OpenGL, portrait): a ship
in a tube turned with one spinner, 66 rings for the jackpot. A Linux cabinet's game in a Windows
build (`spacewarp.rs`, `WAL_ARCADEPC_BOARD=spacewarp`):

- **Linux calls.** `/usr/bin/find /dev -name ttyUSB*` and `/bin/udevadm` are answered by Mono's
  `CreateProcessW` import (their output written to the pipe Mono gave, signaled handles). No disk
  serial and no file in `C:\dev\disk\by-id\`: the licence's two empty serials agree, its key is
  always `66c43ccaa9bc28ef668cac779b4cea2b`, written to `Config.xml` at each start.
- **Settings.** `/touchmagix/config/` is on the current drive: `C:\touchmagix` is the dump's
  `touchmagix` folder (`c_folders`). `Config.xml` is made with the game's defaults when missing (the
  dump's `StreamingAssets/Config.xml` is unused); the common `WAL_FREEPLAY` is `FreePlayMode`.
- **Boards.** Mono's `SerialPort` on `/dev/ttyUSB0` (credits: `G` answers `I`, `play` a coin,
  tickets dropped) and `/dev/ttyUSB1` (the spinner: degrees turned per line, positive to the left).
  Lines end with `\r\n`, its Windows `NewLine`. An empty read must be Mono's pending read over with
  0 bytes (`TimeoutException`): a read returning 0 at once gives an empty line, and in free play any
  line starts a game.
- **Spinner.** The mouse (`mouse_wrap`, `rx`: `WAL_SPACEWARP_MOUSE`, degrees per count), the left
  stick and `left` / `right` (`WAL_SPACEWARP_SPEED`, degrees per second) add up. The game keeps one
  line in a static string its `FixedUpdate` empties: the next one waits until it is (read from
  the payload, 100 ms at most: the attract screen does not empty it), so no turn is lost.
- **Display.** OpenGL's default framebuffer is the window's: a 1080x1920 fake desktop is clipped.
  `Screen.SetResolution` is fitted on the screen's height (Bullseye's replacement) in a 9:16
  borderless window, in the dump's `bezel.png` (`WAL_BEZEL_FIT=stretch`). The window is not managed
  (Wine's `Managed=N` in the profile's registry: a panel pushed it down, the picture cut; the payload
  at the entry point instead stops Unity before its first frame); the payload tells it it has the focus (Unity ignores posted keys otherwise).
- **Operator menu.** `test` posts `C`, the arrows and `service` / `b1` (Enter) drive it.

#### Galaxian 3

Namco's Theater 6 game (two LaserDisc players, six gunners) in the dump's patched MAME 0.289
(`mame.exe gal3`): the laserdiscs play from H.264 (`media\gal3\video-hd.mkv`, decoded to BGRA by
`media\gal3\decoder\reader.exe` into shared memory), the ROMs in `roms\` (`gal3.zip`,
`namcoc67.zip`). The payload is loaded at the entry point (`payload_loader`, `xinput1_3` native),
board `mame` (`mame.rs`):

- **Raw input.** MAME's keyboard and lightgun providers are `rawinput` (no mouse, no joysticks: the
  profile's arguments). The executable's `GetRawInputDeviceList` lists the payload's keyboard and
  one absolute mouse per player (MAME adds them from the list's end: player 1's is gun 1); the real
  devices are not listed, so MAME drops their reports. A thread posts `WM_INPUT` to the window of
  MAME's `RegisterRawInputDevices`, with marked handles `GetRawInputData` answers: each gun's
  position (0..65535 over the window) and buttons, the `key_*` natives' scan codes.
- **Mapping.** `ctrlr\wal.cfg` (the profile's `files`, `-ctrlr wal`): gun n is player n, its buttons
  1-4 trigger 1, trigger 2, start (any player's: the game's single attendant start), coin n; test is
  F2 (the build's default, 5, is coin 1's key too). MAME's menus: the arrows, `b3` Enter (popups),
  `service` Tab. Players 5 and 6 are unmapped (four virtual sticks).
- **Startup popups.** The warnings screen is never skipped while the ROM load reports a missing file
  (the optional `gal3.chd`, replaced by the H.264 media): `WAL_PATCHES` removes the `first_time` test
  of `mame_ui_manager::display_startup_screens` (its later runs' path: no info, no warnings).
- **Display.** DXVK's Direct3D 9; the dump's ReShade (`d3d9.dll`) is off by default (`reshade:
  false`: the game ran far below full speed with its effects). Under wined3d the laserdisc picture
  was green stripes (the emulation's own snapshots were right). `WAL_MAME_STRETCH=1` (GUI option)
  writes `keepaspect 0` to `ini\gal3.ini` at each start: the picture stretched to the screen.

#### Burning Striker

Eolith's two-player football game (2003, "Eoforce" PC, `ewcs.exe`, DirectDraw / Direct3D 7,
32-bit). The payload is loaded at the entry point (`payload_loader`), board `eolith` (`eolith.rs`):

- **Protection.** `0x40a5a0` creates the shared memory `_buring_shared_mem` and runs `vault.exe`
  through `TWIMNET.DLL`, which copies 15 bytes of the cabinet's BIOS (physical 0xFE05E, read with
  the absent `AccessHW.dll`) to its offset 8 and starts the game again; the second instance sets
  the status to 6 and goes on. Those bytes are the IDEA key of `3ddata.pak`'s directory (each 8-byte
  block decrypted, the key changed after each by the ciphertext; the files themselves are
  plaintext). The check returns 1 (`WAL_PATCHES`), the payload's shared memory takes its place
  (`0x565974`): status 6 and the key, read from the dump's `eolith.key` (not in our sources; taken
  from a running cabinet emulation's memory).
- **Board.** COM1, 9600 bauds, frames `00 b1 b2 b3 b4 50` read every 33 ms, active low: `b1` / `b2`
  the players (stick, buttons A/B/C), `b3` the starts, coins (held two frames), service, test.
  Bits kept released so a data byte is never `00` / `50`.
- **DIP switches.** `b4 >> 1`, read once at boot right after the port is opened, before the
  board's first frame: the getter (`0x403820`) is patched to return the payload's value. Level 2,
  one coin per credit, play time 7, mode free play (common `WAL_FREEPLAY`; free play also gives
  the attract its sound) or coins. `eoconfig.ini` is only written by the game, never read.
- **Movies.** `DDrawXCLDLL.dll` plays `EPics\*.avi` (MS-MPEG4) with `IAMMultiMediaStream` into a
  DirectDraw surface. Two GE-Proton qasf (DMO wrapper) fixes, patches 0116 / 0117 (runner
  `qasf.dll` rebuilt): stopping the movie deadlocked (the allocators decommitted after taking the
  streaming lock that the thread waiting in amstream's `GetBuffer` held: black screen, window not
  responding); the movie was upside down (the DirectDraw stream's top-down type, asked through the
  sample's `GetMediaType`, was ignored).
- **Display.** d7vk; the game's 640x480 exclusive full screen in Wine's emulated mode. d7vk hands
  the cooperative level and display mode to Wine's ddraw, whose wined3d (loaded later) minimized
  the window when another window took the focus: `window.rs` hooks d7vk's `LoadLibrary` and drops
  wined3d's minimizes; `WAL_DESKTOP_ACTIVE` / `WAL_DESKTOP_UNMANAGED`. A window of our size or a
  bezel broke the picture (3D unscaled, the bezel over the game in the emulated mode).

#### Come on Baby!

ExtraLine's party game (2000, `COBDEMO.exe`, 32-bit, Glide 2), two players with a stick, two
buttons and a start button. The payload is loaded at the entry point (`payload_loader`,
`xinput1_3` native), board `comeonbaby` (`comeonbaby.rs`):

- **Board.** The cabinet's I/O is on the parallel port: the reader (`0x4126d0`, a group byte)
  writes the group to the data port, strobes the control port and returns the status port's upper
  nibble, active low: groups 0 / 2 the players' sticks, 1 / 3 their buttons (A, B, start), 4 the
  operator's (test, service, coin), 8 the boot's I/O check (5, else `IO_ERROR!`). The dump's
  `in` / `out` instructions are already NOPs and its dgVoodoo `Glide2x.dll` imports
  `cob_patch.dll`, which hooks the reader with the real keyboard; the payload's jump replaces
  either.
- **Picture.** The runner's gxvk (`Glide2x.dll` in the profile's `files`, `useMonitorResolution`,
  no logo) in place of the dump's dgVoodoo 2.53: always windowed at 640x480 (resized again every
  2 s through calls resolved at run time), its full screen stretched to the screen; dgVoodoo
  2.87 hangs at boot under Wine, its "Wine fix" build loses the large sprites and the texts.
  gxvk 0.1 drew the sprites' chroma key (magenta boxes) and lost black texels: the runner has a
  later build (`v0.2-WIP-6-gc6d68eae`). The game hands `grSstWinOpen` no window and runs no
  message loop: gxvk took its 0x0 window (a few pixels on the desktop) and the picture went
  black once gxvk needed its window's messages. As for Xtom 3D, `grSstWinOpen` gets a window of
  the payload (the largest centered 4:3 rectangle of the screen, a thread and message loop of
  its own, gxvk's `SetWindowPos` / `MoveWindow` keep it there), the game's hidden, the black
  bezel around. The window's WM_MOVE no longer closes the game (`0x4121bc`: on a failed
  `grSstControl(move)`, dgVoodoo's).
- **Operator menu.** Test moves, service selects. The menu's select and the frame update (service:
  a credit) shared one edge detector (`0x4112f0`, its state flipped at each call while held): the
  frame update, called first, took a short press, only a long one selected. The menu's call
  (`0x46fb10`) is the payload's own edge detector.
- **Settings.** `eep.rom` (`COB 2.0`): byte 0x0c the coins per credit to start (1..9, 0 free play),
  0x0d to continue. The dump's: 3 coins. `WAL_FREEPLAY` sets both after the boot reads the file
  (its tail jump to the reader, `0x412c4a`, through the payload): 0 when on, 1 when off.
- **Protection.** `0x412790` compares `c:\windows\win386p.swp` with C:'s volume serial number:
  `WAL_PATCHES` returns 1.

#### Come On Baby! 2 Winter Championship

The sequel (ExtraLine / Expotato, 2003, `Cob2.exe`, 32-bit, DirectDraw / Direct3D 7 HAL at
640x480, 16-bit), same cabinet: two players with a stick, two buttons (green A, blue B) and a
start button (red). Payload at the entry point (`payload_loader`, `xinput1_3` native),
`graphics: d7vk`, board `comeonbaby` (`comeonbaby.rs`, its `BUILDS` table: each game's
addresses, the build picked by the bytes at its reader and settings jump):

- **Board.** The same parallel port reader (`0x41f6c0`, the same groups and bits), whose `in` /
  `out` are real here (privileged instructions under Wine): the payload's jump replaces the
  reader, `WAL_PATCHES` makes the lamps' writers (`0x41f640`, `0x41f690`) return. The coins are
  counted by a thread of the game (`0x41d300`, every 30 ms): the payload's 150 ms coin pulse.
  The operator menu's service select has its own edge state (`0x753470`): no menu hook needed.
- **Settings.** `eep.rom` (`COB 2.1`, 0x5c bytes, read at `0x41fb20` through the jump at
  `0x41fd00`): byte 0x0c the coins to start (`0x953664`), 0x0d to continue (`0x951e50`), as in
  the first game; `WAL_FREEPLAY` overrides both after the read (0 / 1). The operator menu's coin
  page shows 자유 게임 (free play).
- **Protection.** `0x41f6f0` compares bytes of a file with C:'s volume serial number:
  `WAL_PATCHES` returns 1.
- **Movie.** The attract's `movie/title.mpg` (MPEG, DirectShow's video renderer as a child of the
  game's window, `put_Owner`) plays through the runner's decoders (winedmo); the game first asked
  the renderer for `IDirectDrawVideo` (not in Wine: a `pBF->QueryInterface ERROR` box, then calls
  through a null pointer): that setup (`0x41e1e6`) jumps to its end.
- **Operator menu.** Test (F2) enters / moves, service selects (or the green button), 끝내기 /
  저장 및 끝내기 restart the game. Its texts are Korean (CP949 through `MultiByteToWideChar` on
  the ANSI code page, GDI `TextOutW`, font 굴림체 in `HANGEUL_CHARSET`): `WAL_ANSI_CODEPAGE` and
  `WAL_FONT_CODEPAGE` 949 (Wine's font fallback for Hangul: a CJK font on the system).
- **Picture.** The 640x480 exclusive full screen, scaled to the screen by Wine's emulated mode
  (d7vk then wined3d), `WAL_DESKTOP_ACTIVE` / `WAL_DESKTOP_UNMANAGED` as Burning Striker's.

#### Robin Hood

ICE's ticket redemption crossbow game (`data\game.exe`, Direct3D 9, 32-bit, 1280x720), one
player, a lightgun game (the gun's X position aims, `guns_enabled`). The payload is loaded at
the entry point (`payload_loader`, `xinput1_3` native), board `robinhood` (`robinhood.rs`):

- **Working directory.** The game reads `.\game.ini`, `log\PR*.CFG` (the operator settings) and
  `data\game.bin` from the dump's root (the cabinet's `start.bat`); the launcher starts it in
  `data`: the payload goes up one folder first.
- **Board.** The first COM port that opens (COM1-COM4, 9600 bauds, thread `0x490650`). The game
  sends 7-byte packets `cmd, timeGetTime (4 bytes), arg, sum` (sum of the first six) and reads
  the board's packets until the echo of its command: one byte for `00`, `FF`, `94`-`98`, `#`,
  `$`, `)`, `0`, else `cmd, d1-d5, sum`. Once per frame it polls with `%`: one packet per held
  switch (`80` advance, `81` spare, `84` select, `C` admin, `83` start, `D` free credit: the I/O
  check's names; bits 0x01, 0x02, 0x04, 0x08, 0x10, 0x20 of the frame's inputs), `@` per coin
  (`A` the second coin slot), then `%`, which commits them (`0x48f8c0`). The game's other
  commands: `00` / `#` at start, `)` the coin setting, `94` the I/O mode, `95` a lamp, `96`
  tickets to pay (100 at most per command), `0` the dispenser's state every 128 frames while
  tickets are owed: `Q` / `P` the tickets left (low / high byte), `B` a vending error
  (`TICKET VENDING ERROR`). The payload pays at once: `0` gets `Q 0`, `P 0`.
- **Crossbow.** A DirectInput 5 joystick (`jcontrol` 1): X (range -1000..1000, averaged over 16
  frames, `iostlt` / `iostrt` the calibration) the aim across the screen, buttons `jtrigb` (0)
  the trigger and `jstart` (1) start: the common `WAL_DINPUT_PADS=1` (player 1's `lx`, the
  gun's X position, buttons `b1,start`). At the default calibration (`iostlt` -90, `iostrt` 90,
  `iostct` 0) the picture's x is `(X / 0.99 + 1) / 2` of its width (`0x48fa10`): the gun's place.
  The game draws its own sight (`sightmark`). The PC keyboard (`GetKeyboardState`: the mouse's
  left button and space shoot, Enter starts, `T` / Escape admin, the arrows the menu) is hidden
  (`WAL_KEYBOARD_STATE_DISABLE`).
- **Dongle.** A HASP HL (vendor code at `0x61c808`, feature 1), its API linked in the
  executable: at boot (`0x4913c0`, after `game.bin`'s CRC check) the game logs in and reads 32
  bytes of its memory, which must start with `GWRH`; any failure ends the boot on error 12. The
  profile's patches: `hasp_login` returns 0 with handle 1, `hasp_read` the tag, `hasp_logout` 0.
- **Free play.** `coinfree` of the operator settings, loaded at `0x48f1d0` (the dump's: on):
  `WAL_FREEPLAY` replaces the loaded value (`mov ecx, 0/1` at `0x48f1e5`).
- Verified: free play and coins (`INSERT COIN(S)`, a credit), start, aim and shots (the targets
  hit, tickets counted), the collect screen, the payout (`96`, then `0` answered), the admin
  switch's I/O check (the switches' names above); the user played it with a gun.

#### Akuma Mortis Immortal

A two-player lightgun game (`data\game.exe`, window class `NGUN`, Direct3D 9, 32-bit, 640x480).
Dump: the cabinet's `data\` folder (the executable, `DK2win32.dll`, `expdat` / `extdat`, `bin\`:
the game reads `..\data\bin\`). The payload is loaded at the entry point (`payload_loader`,
`xinput1_3` native), board `ngun` (`ngun.rs`):

- **Board.** A gun board on the parallel port, through `inpout32.dll` (`LoadLibraryA("inpout32")`,
  `Inp32` / `Out32`, stdcall): the payload answers both, the dump's library (a driver installer)
  is never loaded. A command is the data port's byte (0x378) strobed by the control port (0x37a:
  0xd0, 0xd1, 0xd0), its answer read back from the data port in input mode (0xf8, 0xf9). 1 / 2
  player 1's gun x / y, 3 / 4 player 2's, 5 the guns' bits (0 / 4 the triggers, 2 / 6 the gun on
  the screen: a trigger off it is the reload), 6 the triggers alone (read at each frame), 7 / 8
  the switches (bit 0 / 1 the start buttons, 2 test, 3 service, 4 coin). `x << 4 | 0xd` / `0xe`:
  lamps and recoil, ignored. Status (0x379): bits 1-2 the board is there (else no reader
  thread), bit 7 a gun sample (the reader asks 1..5).
- **Flash.** Each gun sample makes the game draw one flash frame, the screen filled with the
  settings' color (cyan) for the gun's sensor: the payload asks a sample at each trigger (or
  reload) press. A sample every few milliseconds left the picture cyan.
- **Aim.** The calibration (`0x590d24`, 0x20 per player: x min / max, y min / max, the
  calibrated center byte -100 / +100, -115 / +115): x = (byte - min) * 3.1875, y = (byte - min)
  * 2.0788 over 640x480. The payload sends the byte of the gun's place in the 4:3 picture
  through the current calibration (the game's calibration keeps its center). The game draws no
  crosshair: the common crosshair option, on (`WAL_CROSSHAIR`).
- **Dongle.** A Dinkey (`DK2win32.dll`), its result an error code (`0x4edfc0`, "HARDWARE ERROR
  (n)" drawn instead of the game): the profile's patches make the boot's check (`0x40147e`,
  ordinal 4) true, the dongle's read (`0x406e50`) and its periodic checks (`0x406da0`,
  `0x406a90`) return at once, and the check of its cabinet flag against the dual screen mode
  (`0x40de56`, error 6) never fail.
- **Settings.** `expdat` / `extdat` (each byte negated, check sums), read into `0x590f10`..:
  free play is bit 0 of `0x590f2c`, copied by `0x418de0` (called at `0x4074ab`): `WAL_FREEPLAY`
  sets the bit before that copy.
- **Display.** Windowed 640x480 device in a borderless window, the screen's centered 4:3
  rectangle (`WAL_WINDOW_SIZE=aspect`, `WAL_D3D9_KEEP_WINDOW`), not managed by the window
  manager (`WAL_DESKTOP_UNMANAGED`: a top bar pushed it down), the dump's `bezel.png` around it
  (its opening fitted, the sides stretched: `WAL_BEZEL_FIT=stretch`).
- Verified: attract, free play, start, aim and shots scoring (headless); the user: 100%.

#### DCOP

Klockworks' one-gun live-action police shooter (`DCOP.exe`, Unity 2020.3, Mono, 64-bit,
Direct3D 11). The dump keeps BepInEx (Unity Doorstop's `winhttp.dll`, native) and Argonlefou's
DemulShooter plugin (`BepInEx\plugins\DCOP_BepInEx_DemulShooter_Plugin.dll` + `.ini`), its
`launch.bat` started DemulShooter first. `.windowsloader`: `DCOP.exe`. Board `dcop` (`dcop.rs`):

- **Game input.** The game plays with the mouse: `GunScript` shoots at `Input.mousePosition` on
  `Fire1` (left button, the machine gun while held), reloads on `Fire2` (right button); any key
  or button starts a game in the attract mode (`AttractMode`: `Input.anyKeyDown`, the "insert
  coin" screen) or continues one (`Continue`: `Input.anyKey`). There is no credit count: free
  play by design (`WAL_FREEPLAY` has nothing to change). The payload plays the mouse with the
  gun (`post_mouse`, Unity's `GetCursorPos` answered: `own_mouse`): its place on the window's
  picture, trigger the left button, `b2` (or a shot off the picture / with the gun off the
  screen) the right one; start, coin, test, up / down are posted keys (Return, 5, 0, arrows).
- **Plugin.** Its TCP input packet (port 33610) is only two bytes here (`TcpData`'s fields sorted
  by name: `EnableInputsHack`, `HideCrosshairs`); the game never reads the plugin's own aim or
  buttons. Its Harmony patches: Uduino's serial port discovery given no port (the cabinet's
  Arduino only drives lamps, recoil and the gun's light: outputs), `persistentDataPath` the
  dump's `NVRAM\` (`SAVE_TO_GAME_FOLDER=1`), the pause menu's `Escape` made `0` (Escape is its
  exit key), `SetMouseCursor` honoring the hidden crosshair, `Screen.SetResolution` from its
  `.ini` (1920x1080 windowed). The payload (`WAL_DEMUL_PORT`) sends it `[0, 1]` once: the game's
  cursor (the system cursor, at the real pointer) hidden; the common crosshair option draws the
  gun (`WAL_CROSSHAIR`, `pointer_d3d11`).
- **Operator menu.** `0` (test) opens "DCOP SERVICE MENU" (high scores, reset, Arduino pin test,
  volume, resume, exit): its items are hovered and clicked with the gun, or the arrows and Return.
- **Display.** The 1920x1080 window made a borderless popup, the screen's largest 16:9 rectangle
  (`WAL_D3D9_ASPECT`, `WAL_WINDOW_SIZE=picture`), no system cursor (`WAL_HIDE_CURSOR`). That is
  all on a 1920x1080 screen. On any other screen Unity letterboxes its 1920x1080 windowed picture
  inside its window (whatever the window's shape: no stretch with `WAL_KEEP_ASPECT_RATIO=0`, and
  the gun / crosshair were off what it showed), and takes the cursor's client coordinates as its
  1920x1080 pixels. There `dcop::prepare` (before `payload-common` starts) gives the game a
  1920x1080 desktop (`WAL_DESKTOP_SIZE`, the window rectangle dropped) fitted on the screen,
  stretched over it with KEEP ASPECT RATIO off (`desktop.rs`'s new `WAL_DESKTOP_STRETCH=1`: each
  axis its own scale, only with `WAL_KEEP_ASPECT_RATIO=0`), and the plugin's
  `Screen.SetResolution(1920, 1080, false)` is made full screen on it (`bullseye.rs`'s Mono
  replacement): a 1920x1080 swap chain, DXVK scales it into the window. The mouse is always posted
  in 1920x1080 units (the gun's fraction of the window's picture).
- Verified headless (2026-11-09): attract, start, shots scoring (241, 567, 1134), reload,
  continue / game over, the service menu opened and resumed with the gun; on a 1280x1024 output
  letterboxed 16:9 (aspect kept) and stretched over the whole screen (`WAL_KEEP_ASPECT_RATIO=0`),
  the menu item under the crosshair highlighted and clicked in both, 1920x1080 unchanged.

#### Ballistics

Grin's tube racer, arcade version (2001, `Ballistics.exe`, Direct3D 8, 32-bit; the dump also
has `Ballistics_NOHUD.exe`, debug builds `*d.exe`, `BallisticsLauncher.exe`, and the D-Box
motion seat's `dbox*.dll`). `.windowsloader`: `Ballistics.exe`. Board `ballistics`
(`ballistics.rs`), loaded at the entry point:

- **Controls.** The cabinet's controls are a USB "HAPP UGCI(tm) USB Driving GameFrame v2.02":
  the input setup (`0x42c920`) takes the DirectInput joystick of that exact name (joystick or
  gamepad type: the enumeration's callback skips driving devices) or throws "Could not find
  HAPP UGCI..." and quits. It binds its axes by their names' first letter: `X` (pitch and roll
  right: the steering), `Y` (roll left: the throttle, full at the top of the axis' range), and
  its buttons by name: `Button 0` select / thrust, `1` and `7` flip, `2` brake, `3` throttle,
  `4` roll right, `9` booster (all but 4 also the menus' "any button"). A DirectInput keyboard
  must exist too ("Could not find Keyboard input device!"; only W and F5 bound). The common fake
  DirectInput's wheel is that joystick, with new opt-in variables in `dinput.rs`:
  `WAL_DINPUT_WHEEL_JOYSTICK=1` (joystick device type), `WAL_DINPUT_WHEEL_BUTTON_BASE=0`
  (`Button 0`.., Windows' names), `WAL_DINPUT_WHEEL_SYSTEM=1` (the system keyboard and mouse
  listed and created next to it). Profile: X `lx`, Y `-accel` (released: no throttle), buttons
  start (select), `b1` throttle, `b2` brake, `b3` flip, `b4` booster; keyboard arrows steer and
  throttle. `Common\Bindings.txt` (the PC version's keyboard / mouse layout, a Swedish keyboard)
  is still parsed; it binds nothing that exists.
- **Coins.** The cabinet's coin program posts the registered message `BALLISTICS ARCADE
  COINDROP` to the game's window (class `Diesel default rendering window`) with the count of
  coins since it started: the window procedure adds the difference to the coin counter, a
  credit per `Price` coins (`Common\Options.txt`: 4). The payload posts it at each coin press
  (the readme's Escape coin is not in this build). Free play (`WAL_FREEPLAY`): the loaded price
  made 0, the game's own free play ("PRESS ANY BUTTON TO PLAY", a button gives a credit).
- **Restart.** After `Restart Time` (3600 s) in the attract loop the game broadcasts `SET OUTPUT`
  and quits (the cabinet's `start.bat` restarted it): the loaded time is made a year.
- **Display settings.** `display_settings.txt` holds the adapter and mode of the settings dialog;
  when it does not match the adapter the boot shows the dialog (the dump's came from another PC): the
  payload's dialog procedure presses OK after the game's set it up (full screen 800x600, 32-bit)
  and the game writes the file into the dump (the SquashFS overlay on Batocera). The device is
  made windowed 800x600 in a borderless window, the screen's centered 4:3 rectangle, black
  around (`WAL_D3D9_WINDOWED_SIZE`, `WAL_D3D9_ASPECT`, `WAL_WINDOW_SIZE=aspect`).
- **Direct3D 8.** wined3d (`dxvk: false`): DXVK's Direct3D 8 drew Sky Town's buildings and sky
  white (textures missing); wined3d draws them.
- **Motion seat.** `dbox.dll` (loaded with `LoadLibraryA`, its functions optional) is not loaded:
  its `programInit` crashed on its absent COM server.
- No operator menu (the settings are `Common\Options.txt`, all of whose entries must exist).
- Verified headless (2026-11-09): attract, coins (credits 4/4, the name entry), free play's
  "PRESS ANY BUTTON TO PLAY", name / track / speeder selection, a Sky Town race (throttle,
  steering off a wall, brake, flip, booster).

#### Bug Busters

MIZI Production's two-player spray gun game (2001), its PC release (`BBPC.exe`, DirectDraw
640x480, 32-bit; `Config.exe` its settings program, `config\BUSTER.CFG` the operator settings,
`COLOR.CFG`, `score.cfg`). The dump's `BBPC.exe` is the community no-CD build, one byte from
the original `BBPC.exe.old` (the CD check's loop `je` made `jne`); its `launch.bat` started
DemulShooter (`-target=windows -rom=bugbust`: memory writes of the guns, used as the reference)
first. `.windowsloader`: `BBPC.exe`. Loaded at the entry point, board `bugbusters`
(`bugbusters.rs`):

- **CD check.** `0x409f32` (a drive with the game's CD label, else a message box asking for it)
  returns 1 (`WAL_PATCHES`): the original executable runs too.
- **Inputs.** The PC build plays each player from the mouse (window messages, `0x4554e0`'s
  player), the DirectInput keyboard or a joystick (`control.cfg`): all end in the players'
  records (`0x45fba8` + 0x1c: x 0..640, y 0..480, trigger) and the spray's tank (`0x45509c`,
  floats, 1.0 full; a reload press adds 0.25). The arcade cabinet's serial boards are dead code
  (their reader is a `ret`), but each frame's switch check (`0x40a872`) still takes the I/O
  board's flags: `0x49e880` / `0x49ea84` the start buttons, `0x49e870` the test switch, `0x49732c`
  the setup menu's service switch; coins `0x460c1c` (+1 per coin, + the price per service
  credit, with the board reader's coin / credit sounds). The payload writes them from the virtual
  sticks in the game's thread before each frame (the main loop's `Sleep(0)`, an import hook):
  each gun's place in the 4:3 picture (Wine scales the 640x480 mode to the screen, 4:3 kept), its
  trigger (released off the picture), a reload (`b2`, or a trigger off the picture) fills the
  tank, the flags at the presses. A start without a credit is dropped (the title's first phase
  kept it until a coin, which then started a game). The real mouse and keyboard are filtered
  (`Enter` is a start with a free coin, `Esc` the exit, `1` / `2` the starts); the game's
  DirectInput is the fake one without devices (`WAL_DINPUT_DISABLE`). The title's free coins (9
  at each title, `0x44091f`) are not given.
- **Setup menu.** "BUSTER SETUP" (credit, lives, sounds, air compressor, difficulty, gun
  operation, mode select, controller / gun tests, factory default, save / cancel): test enters
  it at the next screen change; the title (which waits for a start, no attract cycle in free
  play) is faded out by the payload as a start does, its exit opens the menu. In it test moves
  to the next item, service changes the item's value or runs it (cancel and exit, the cursor's
  first item).
- **Free play** (`WAL_FREEPLAY`): the price read from `BUSTER.CFG` (`0x411faa`) and at the setup
  menu's exit (`0x43e9b4`) is 0: the game's "FREE PLAY"; the setup keeps the operator's price.
- **Crosshair.** The game draws the players' pointers; the common crosshair option off hides them
  in the game (`0x4263c1`, as for a joystick player), the menus keep them.
- **Display.** Wine's DirectDraw (`graphics: wine`): with d7vk the MPEG-1 movies (attract,
  story intro and cut scenes: amstream's DirectDraw stream surfaces blitted to the game's) stayed
  black ("Blt: Received an unwrapped source surface"). The 640x480 exclusive mode is scaled to
  the screen by Wine, 4:3 kept, centered (verified on an X11 screen without gamescope, Xvfb:
  1920x1080 black sides, 1280x1024 black bands; the guns aimed at the picture's corners and
  center put the game's pointer there). A 4:3 window instead (`WAL_NO_MODE_CHANGE`,
  `WAL_WINDOW_SIZE`) showed the exclusive picture unscaled at the screen's corner.
- **Sinden border** (`WAL_SINDEN_BORDER`, the launcher's `auto`): `payload-common`'s border is
  turned off before it starts (its window path keeps the game's mode and a screen-sized window:
  the 640x480 picture was drawn unscaled in the top left corner, and it draws nothing over
  DirectDraw). The payload draws it into the game's back buffer before each present (the
  present function `0x401a18` jumps to the payload, a trampoline runs its moved instructions):
  the back buffer copied to an offscreen surface of its format, cleared, the copy stretched into
  the frame's picture, the colored ring (and the outer black outline of `big`) color-filled at
  its edges. Wine then scales the framed 640x480 to the screen: the ring frames the 4:3 picture
  (black sides beyond). The picture's place in the surface is the launcher's aim picture
  (`Border::picture` of the screen, as fractions: on 1920x1080 `[68, 38, 1784, 1004]` -> 23,17 -
  617,463 of 640x480), the ring's thickness the screen's (2 % of 1920 = 38 screen pixels, 17
  surface pixels shown 1440 wide): the Sinden's position over its frame, mapped by the launcher
  into that picture, is the place in the game's surface, which the payload then uses whole.
  Verified on Xvfb 1920x1080: the white ring around the 4:3 picture (title, menus, movies, in
  game), the pointers at the picture's corners and center where aimed.
- Verified headless (2026-11-09): title, free play and coins ("CREDIT"), both players' starts,
  story / free select, movies, both guns' aim and shots scoring, the tank emptied and reloaded
  (`b2` and off screen), test into the setup menu, its items and exit, crosshair option off.

#### Hydro Thunder / Offroad Thunder (VCThunder)

Midway's Hydro Thunder (1999, `hydro\HYDRO.EXE`, the Diego I/O board) and Offroad Thunder
(2000, `offroad\offroad3.exe`), both 3dfx Glide cabinets, run by VCThunder 0.5
(<https://github.com/mourix/VCThunder>, MIT, by mourix: `VCThunder.exe <hydro|offroad>`, 32-bit;
its `docs/configuration.md` documents every ini key). Its stub executable reserves the games'
span at 0x100000 and loads `VCThunder.dll`, which maps and unpacks the original executable
there, binds its imports, emulates its board (keys, DirectInput wheel and buttons, XInput),
sound (WASAPI), timing, NVRAM (`save\`) and draws its Glide with `vcglide.dll` (SDL3 GPU on
Direct3D 12, else a CPU rasteriser presented with Direct3D 9). Its launcher (no title on the
command line) is never shown: the profiles run the title. Two dumps, the runtime beside each
game's folder: `Hydro Thunder` (`hydro\`, `hydro-thunder.windowsloader`) and `Offroad Thunder`
(`offroad\`, `offroad-thunder.windowsloader`), profiles `hydro-thunder.yaml` /
`offroad-thunder.yaml` (`args`: the title). 0.1 (the dumps' first runtime) needed two more
payload hooks: its DirectInput axes and `[ffb]` readers took the bare `vcthunder.ini` (the
Windows directory), and its keys were the only digital inputs. Loaded at the entry point, board
`vcthunder` (`vcthunder.rs`):

- **Settings.** The profile's `vcthunder.ini` (`files/vcthunder/`, 0.5 keys) replaces the dump's
  (0.1's, `data_dir` the author's PC): `data_dir = .`, `save_dir = save`, full screen,
  `aspect = 4:3`, the DirectInput wheel, its buttons, no keyboard; the payload sets `[graphics]`
  `aspect` / `widescreen` / `frame_rate` from the options at each start. The dump's logs are
  hidden (the runtime writes its own in the run directory).
- **GPU renderer.** Wine's builtin d3d12 is "unsupported" for SDL's GPU device: the profiles
  install the runner's vkd3d-proton (`{runner}/lib/wine/vkd3d-proton/i386-windows/d3d12.dll`,
  `d3d12core.dll`, native overrides): "D3D12/DXBC backend ready", water, roads and boats drawn
  (0.5 ships its shaders precompiled), a mailbox swapchain. Without them the runtime falls back to
  its CPU renderer (`vcglide_renderer = auto`). `frame_rate = 60` (option `WAL_VCTHUNDER_60FPS`,
  default on): an interpolated picture between each two of the game's ticks; headless, 33
  in-betweens of 2100 skipped for time, the game at 30 ticks a second.
- **Inputs.** The wheel and pedals are the common fake DirectInput wheel (`WAL_DINPUT_WHEEL`: X
  steering, Y gas, Z brake, its range's low end released), hooked into `VCThunder.dll` when it
  loads (`dinput::hook_module`: it resolves `DirectInput8Create` at run time). Its buttons
  (`WAL_DINPUT_WHEEL_BUTTONS`) are the virtual sticks' b1 b2 b3 b4 b6 b5 coin start service test
  up down b7 b8, numbered 1-14, bound by the ini's `[gamepad]`: boost / nitro, the three views /
  cameras, the select screens' previous / next item (b6 / b5, the runtime's `menu_prev` /
  `menu_next`), coin, start, credit, test, volume up / down (the operator menus' cursor). The real
  keyboard is filtered, XInput disabled (`xinput1_4`). The ini's only keys: the second coin slot
  (`6`, player 2's coin) and Offroad's gears (`E F G`): the runtime reads the gear whose key is
  held, neutral without one; its own shift up / down latch starts in neutral (the truck stays
  on the grid until a shift, the automatic transmission too), so the payload latches the gear
  (first at start, b5 / b6 shift up / down, b7 / b8 first / third gear) and holds its key.
- **Force feedback.** The runtime turns the game's motor DAC into a DirectInput constant force;
  the fake wheel sends it to the launcher (`WAL_DINPUT_WHEEL_FFB_OUT=1`: magnitude x gain x
  direction sign, 0 when stopped). Its sign on a real wheel is not verified (`[ffb] direction`).
- **Operator menus.** Test enters them. Hydro's network stack cleans Winsock up when it enters
  them (`WSACleanup` through the runtime): the payload's launcher connection closed and could not
  reconnect (no input in the menus): the payload holds one more Winsock reference. Volume up /
  down move, test selects, credit cancels.
- **Free play** (`WAL_FREEPLAY`): Hydro's `_coins_CreditsToStart` / `_coins_CreditsToContinue`
  (0x134508, 0x134518) answer 0 ("PRESS BOOST BUTTON TO START"); Offroad's free play flag
  (`_money_GetFreePlay` 0x1c4f1c: `[0x410e74] == 0xfeed12f8`) is held set ("FREE PLAY"). The
  code is patched once `VCThunder.dll` copied it, 1 s later (after its checksum of the unmodified
  code). Off: the operator settings' pricing (coins).
- **Display.** The runtime's borderless full screen went back to a 512x400 window under
  gamescope (0.1): its window is a borderless popup over the screen (`WAL_WINDOW_POPUP`,
  `WAL_WINDOW_SIZE=screen`), `aspect = 4:3` centers the cabinet's 4:3 picture. Option
  `WAL_VCTHUNDER_WIDESCREEN`: the runtime's Hor+ 16:9 (`aspect = 16:9`, `widescreen = true`).
- Verified headless with 0.5 (2026-11-10, GPU renderer, 60 fps): both games' attract and splash
  screens, free play and coins, Hydro's boat select and a race (throttle, steering, boost),
  Offroad's track / truck / transmission select and a race from first gear (gas, steering, shifts,
  nitro), both operator menus (navigation, Hydro's volume menu, Offroad's calibration screen),
  both games in widescreen. 0.1 was user-tested (2026-11-09).

#### Arctic Thunder

Midway's snowmobile racer (2001, `snow.exe`, Glide 3, 32-bit, the "goose" game library). The dump
is the cabinet's Windows 2000 disk copied as files; `arctic-thunder.windowsloader` (at the dump
root) names `Artic Thunder\Midway games\Arctic Thunder\snow.exe`, and `c_folders` maps
`C:\Midway Games` to the disk's `Midway games` folder. Payload at the entry point
(`payload_loader`, `xinput1_3` native), board `arcticthunder` (`arcticthunder.rs`):

- **Board.** The I/O board (serial, `magicbus.dll`) belongs to the coin and security manager
  `wincsm.exe`; `goose.dll` finds it by process name, reads
  `HKLM\Software\Midway Games\WinCSM\IOBlock` (the address in wincsm of an I/O block pointer and
  a control block pointer) and reads / writes wincsm's memory. The payload is wincsm: the blocks
  are its memory, the registry value is written at start and goose's search (`0x1000c860`) returns
  a handle of the game's own process. I/O block +8: the 0x330 bytes goose copies when the control
  flag is 0 (switch words, active low; per switch press / release counts that drive the game's
  handlers; four 10-bit analog values), +0x338 the pricing, +0x3cc the credits. Control block:
  +0x18 credits to subtract, +0x24 a service credit, +0x28 the coin lock (1 asked, 2 granted).
- **Inputs.** Players' word: bit 7 attack (the handlebar button), 0-6 directions and A-C. Coin /
  operator word (the diagnostics' names): 0 start, 4 service credit, 5 / 6 volume, 7 test, 8-12
  coins and bill. Analog 0 the handlebar (0..1020, the left stick), 1 the throttle (`accel`, or
  `b2` full). One credit a coin, one to start and to continue (the cabinet's table: four).
- **Free play.** `COIN_FREE_ONLY` of `game.env`: the game reads its settings through `getenv`
  first, the payload sets it (msvcrt `_putenv`) from `WAL_FREEPLAY`. The profile's environment
  also turns off the force feedback and its power-on calibration (it waited for the handlebar to
  follow the motor) and the 60 s DHCP wait.
- **Protection.** All 1741 textures (`*.wms`) are RC4-encrypted with a key from the cabinet's
  hard disk serial number (goose `0x10008f60`, reads it through `winio.sys` / the SCSI miniport,
  its digits a number modulo 2^28; MD5 of its 4 bytes, the base provider's salted 40-bit RC4 key)
  and flagged read-only + hidden, the only files `gobj.dll` decrypts. Without them the fonts were
  blank boxes and the first track texture an assertion. The dump's number, 0x0a6a209a, was found
  by trying the 2^28 values against the known first bytes (`05 80 00 00`): goose's serial reader
  returns it, gobj's `GetFileAttributesA` reports encrypted textures read-only + hidden, and
  goose's `CryptAcquireContextA` without a provider name gets the base provider (Wine's default is
  the enhanced one: 128-bit keys).
- **Floating point.** goose's process loop unmasks invalid / divide by zero (`_control87`, no
  `goose.ini`); Wine's `_control87` also unmasks them in MXCSR, and the Glide wrappers divide by
  zero: goose's `_control87` keeps every exception masked and MXCSR untouched.
- **Picture.** The dump's own `glide3x.dll` (nGlide 2, 2023): `NGLIDE_RESOLUTION=1` (desktop),
  `NGLIDE_ASPECT=1` (4:3 at full height), no splash (values found by trying them with
  `WAL_SCREENSHOT`, it draws through Direct3D 9). The runner's gxvk showed only a white screen
  in headless runs (not investigated further).
- **Operator menu.** The cabinet's test mode is dip switch S2-1 (dip word +6, bit 0, active
  low): wincsm closes the game and runs `diagnostics\diag.exe` (GDI menus, the game's
  `diagdll.dll` tests), which carries its own copy of goose's I/O code. `test` in the game asks
  the launcher for it (relaunch `exe=../diagnostics/diag.exe`) and the game ends; the payload in
  diag.exe answers its wincsm search (`0x40ebf0`) with its own blocks, sets its working directory
  to `C:\Midway Games\diagnostics` (it opens `\Midway Games\...` on the current drive: the
  launcher's was the dump's, on Z:), gives its `RegConnectRegistryA(".")` the local registry
  (Wine accepts only its own name) and registers the game (`WinCSM\Games`, `Default Game`).
  Menus: up / down (or volume down / up) move, a player button (attack) or test enters. S2-1
  reads off there, so Exit ("to the default game") ends diag.exe and the payload asks the launcher
  for the game again, as wincsm did.
  diag.exe's pages wait for a button in `SleepEx` / `Sleep` loops without handling messages:
  Wine put nothing drawn there on the screen (the Monitor color screens stayed black, the switch
  test lagged) until the thread looked at its queue, so its waits peek at the queue first
  (`PM_NOREMOVE`). The Monitor submenu itself was fine (an earlier empty capture was taken
  mid-redraw). Analog values move in the switch test (wheel 0 / 128 / 255, gas 0..255, start
  lit). Its labels and values overlap: the values sit at fixed pixel offsets while the fonts
  scale with the window (640x480 on the cabinet, the full screen here); cosmetic. Calibrate
  Feedback runs (throttle step) but its wheel step needs the feedback motor (none; the game
  takes the emulated, already scaled values and the profile turns the feedback off).
- **Not done.** No feedback motor, lamps, fan, seat shaker.
- Verified headless (2026-11-09): attract (demo races, high scores, tips), free play start,
  coin then start (1 credit used), driver and track select, a race with throttle and steering
  (left stick left turns left); test into the diagnostics, their menus (System Info, Diagnostics,
  Switch Test with moving analog values, Volume Levels, Monitor color screens, Calibrate
  Feedback's throttle step), Exit back to the game.

#### Ultimate Arctic Thunder

The 2004 build (new drivers and tracks, e.g. Haunted Forest), same cabinet and board
(`arcticthunder`, `arcticthunder.rs`'s `BUILDS` table, by `snow.exe`'s PE time stamp). The dump is
the cabinet's `Midway games` folder only: `ultimate-arctic-thunder.windowsloader` names
`Arctic Thunder\snow.exe`, `c_folders` maps `C:\Midway Games` to the dump root (`.`). Its goose,
gobj, diag.exe and wincsm differ from Arctic Thunder's only in their version data (same addresses);
snow.exe is a new build. The profile is Arctic Thunder's (environment settings over the dump's own
`game.env`, nGlide, keyboard mapping).

- **Textures.** Encrypted as Arctic Thunder's with another cabinet disk's number: 0x0cc4cc8d
  (found the same way against `05 80 00 00`).
- **Dongle.** snow.exe adds a parallel port dongle (`\\.\KeyDongle0` / `PARCLASS0`, a KEYLOK-style
  `kfunc(command, a, b, c)` at `0x401030`) and a monitor: `0x401620(0)` starts `ppmon.exe <pid>
  <cookie>` (not in the dump), which has 10 s to clear the cookie, else "Driver not found" and
  `FatalExit`. The payload is `kfunc`: 1 the challenge (0xaaaa 0x9c88 0x162a), the next call (its
  command and arguments built from the answer) answers 0x7fe3b2e0 (checked: 0xb2e0 low or 0x7fe3
  high, else the thread suspends itself and goose's watchdog ends the game, "PROCESSING LOOP
  TIMEOUT"); 4 / 6 read / write a word of the dongle's memory ("Play" at 0..6, which the boot
  checks, and a random word written then read back); 3 a value read twice that must match (else
  the game adds random drift); 2 / 5 no answer used. `0x401620` returns 0 (no monitor).
- Verified headless (2026-11-10): attract ("FREE PLAY"), free play start, coin ("1 CREDITS TO
  START", "1 CREDIT", 1 credit used), driver and track select (Haunted Forest), a race, test into
  the diagnostics, Exit back to the game (its title).

#### The House of the Dead 2: Remake

MegaPixel Studio's 2025 PC remake (`THE HOUSE OF THE DEAD 2 Remake.exe`, Unity 2022.3 HDRP, Mono,
64-bit, DXVK). Board `hotd2` (`hotd2.rs`, x64 build installed as `xinput1_3.dll`). The PC game plays
with the mouse and pads through Unity's Input System; its crosshairs move by the mouse's or the
sticks' motion.

- **Pads.** Unity reads pads with Windows.Gaming.Input, else XInput: the profile disables
  `windows.gaming.input` and `xinput1_4`, so it loads the payload. `pads.rs` (with
  `reload_on_left_trigger`) gives one pad, player 2's: trigger = right trigger, a shot off the
  screen or reload = left trigger, start = A, b2 = B, the stick centered, no BACK (the gamepad
  cursor toggle).
- **Player 1 = the mouse.** `input_filter::own_mouse` + posted moves and buttons (trigger the left
  button, off screen / reload the right one; the move posted again every 250 ms so a new menu sees
  a still pointer); b2 = Escape. The PC menus point and click with the gun; the payload draws a dot
  there (`pointer_d3d11`, while `Cursor.visible`: the game's cursor is the system's, at the real
  mouse).
- **Controllers.** `MP_ControllerManager.Update` (debug display only) is replaced (Mono's compiled
  code jumps to the payload): player 1 kept on `MouseController`, player 2 on the pad
  (`ForceAssignControllerToPlayerAndHandleOthers`, compared by device: the controller objects are
  made again with each level).
- **Crosshairs.** `CR_Player.handleMovingCrosshair` is replaced: the crosshair's transform placed at
  the gun in screen pixels (screen-space overlay canvas; `CR_Crosshair.getRay` shoots through its
  position), while `CR_ReadyCheckPanel.ArePlayersReady` and the player alive. `WAL_CROSSHAIR=0`
  scales it to 0 (shots unchanged).
- **Title screen, no PC menu.** `CR_MainMenu.Open` is replaced by nothing (`WAL_HOTD2_PC_MENU=1`
  keeps it): the game stays on its title screen and attract (`CR_MainMenuMovieController`: any
  input opened the menu), its `CR_SimplePressAnyKeyDisplay` text set to PRESS START (INSERT COIN
  without free play and no credit) twice a second (`TMP_Text.set_text`); player 1's start is the
  quick start, its trigger too (a campaign at most every 10 s: shots while it loads start no
  other). Verified headless: start or trigger begins chapter 1 (three shots, one campaign), INSERT
  COIN turning PRESS START at a coin.
- **Quick start.** Player 1's start while `CR_LevelLoader.IsMainMenuLoaded` (title, attract, menus)
  does what the New Campaign menu, Start New Game and the chapter selection's Play do:
  `CR_NewGampaignMenu` fields (`activeMode` = `WAL_HOTD2_MODE` arcade/original, `multiplayerMode`
  = cooperative, `difficulty` = `WAL_HOTD2_DIFFICULTY`), `prepareSaveDataForNewGame`,
  `MP_GameManager.LevelAborted`, `SetCurrentChapter(Ch1)`, `SetMultiplayerActive(false)` (player 2
  inactive), `CR_LevelLoader.LoadGameplayLevel(Ch1)`. Player 2 joins with A (the game's
  `OnPlayerJoined`, cooperative campaigns). In a chapter, start = `CR_Player.Revive()` (continue).
- **Free play** (`WAL_FREEPLAY`, default on): `CR_Data.UNLIMITED_CREDITS_ENABLED_FROM_CONTROL_PANEL`
  (the game shows FREE PLAY on its continue screen). Off: `DEFAULT_START_TOKEN_AMOUNT` = 0, coins in
  the menus are credits (start takes one, the rest become player 1's continues), coins in a chapter
  `AddPlayerTokens` for that player.
- Verified headless (2026-11-10): boot, quick start, ready check, cutscene, gameplay with both
  crosshairs exactly on the guns (corners and points checked), shots and scores, player 2 joining,
  death, continue with start (free play), coin mode credits; the PC menus clicked through with the
  gun (New Campaign, tutorial, character, chapter: the Original mode's trunk needs Tab to confirm,
  skipped by the quick start).

#### Mad Bullets

Ironbird's 2016 shooting game, the Steam release (`bin_x86\game.exe`, OpenGL legacy context,
32-bit, OpenAL; `steam_api.dll` a Steam emulator, run directly). Board `madbullets`
(`madbullets.rs`, x86 build), loaded by `payload_loader` (the game loads no XInput DLL; the
`xinput1_3` override native so the loader's `LoadLibrary` takes the payload). The profile installs
`openal` (OpenAL32.dll is not in the dump: the game did not start).

- **Input.** The game reads the mouse from its own `PeekMessageA` loop (`WM_MOUSEMOVE`, left
  button) and keys with `GetKeyState`: `input_filter::init(exe)` + `own_mouse`, the gun posted as
  the mouse (client pixels of the window's picture, re-posted every half second), trigger = left
  button, start = Escape (pause / back).
- **Reload.** The game reloads only by a touch of its bullet belt (the HUD's reload buttons,
  `onHudReload`; no key): the gun's reload (b2, its right button) or a shot off the screen (b6)
  clicks the belt (pointer at 0.15, 0.88 of the picture, left button down and up a frame apart,
  pointer back on the gun). Verified headless: an empty belt full again, a full one unchanged.
- **Crosshair.** The game's pointer is the system cursor (`cursor.cur`, at the real mouse): the
  executable's `gdi32!SwapBuffers` is hooked and a red cross drawn at the gun with the fixed
  pipeline (attributes, matrices, program and draw framebuffer saved and put back), the game's
  cursors (`LoadCursorFromFileA`, `LoadCursorA` in the executable) an invisible one;
  `WAL_CROSSHAIR=0` turns it off (the game's cursor back).
- Verified headless (2026-11-10): menu, PLAY clicked with the gun, boosts screen, gameplay with
  shots scoring at the aimed enemies, the crosshair exactly at the gun.

#### Bounty Hunter

GCTECH's striking game (`bh.exe`, 2002, Power Render 4 on Direct3D 8, 32-bit, UPX-packed,
640x480 on a vertical monitor): the player punches and kicks eight pads (two rows of punch
pads, two of foot pads, left and right) where the targets light up; bosses picked with the pads
(a punch moves, a foot selects). `.windowsloader`: `bh.exe`. The payload is loaded at the
entry point (`payload_loader`, `xinput1_3` native), board `bountyhunter` (`bountyhunter.rs`):

- **Packed.** UPX: the payload starts before the stub unpacks the program; the stub fills the
  import table with `GetProcAddress`, which the payload hooks (chained to the common
  DirectInput's): the serial functions are the emulation's (`serial::replacement`). Its first
  call comes once the code is unpacked: the code patches are written then.
- **Board.** COM1, 57600 bauds (`0x4129c4`), used when the operator settings
  (`BountyHunter.GC`, `DataSetting.exe`'s, read at `0x4141c0`) say so (`+0x4b0`). The dump's file
  is set for the keyboard: hidden, the game starts with its defaults (board on). Packets of 5
  bytes `xx, cmd, data, xx, sum` (`sum`: the complement of the first four's sum), read once
  per frame (`0x412ae0`, dispatched at `0x412c20`): `4` the pads (bits 0-3 the left pads, 4-7
  the right ones, top to bottom: punch high, punch low, foot high, foot low), latched until the
  frame's end; `1` a coin, `2` a service credit, `3` the setup menu. The game's `FF 06 01 n`
  / `FF 05` commands get no answer. A pad press is sent once (pads are struck; a held button
  repeated every frame: the setup menu ran several steps per press).
- **Keyboard.** The window procedure takes the development keys (`0x413c20`: numpad 8 / 9,
  5 / 6, 2 / 3, down / up the pads, `Tab` coin, `F1` setup, `F2` service, test keys):
  skipped (`0x413c32` jumps to the default procedure). DirectInput keyboard (Escape quits):
  the common fake one (`WAL_DINPUT_DISABLE`).
- **Loading.** The engine's allocations go through a leak tracker (`0x475f30` / `0x476110`): a
  list node per block, each free walking the list from its head; the boot's loading took 2.5
  minutes. Both jump to the payload's: the same blocks (zeroed, two `0xeeeeeeee` guards) from
  the game's C runtime (`0x4a4eaa` / `0x4a4b4e`), no list. Loading: about 10 s.
- **Free play.** `setup.dat` (`coins per credit, credits per coin`, read at `0x415500`, written
  by the setup menu) is copied in the game's object (`[0x4cfd50]`, `+0x1e0`): 0 coins plays
  free (any pad starts, "FREE PLAY" on the title). `WAL_FREEPLAY` writes 0 there once the
  object exists (`[0x4cfd50]` holds packed data until then: its value before the program runs is
  ignored). The dump's `setup.dat` is hidden: the game's default (1 coin, 1 credit), then the
  run folder's.
- **Display.** The dump's rotation setup (ArcCabView): dgVoodoo's `D3D8.dll` (Direct3D 11),
  ReShade 4.9's `d3d11.dll` rotating the picture into the dump's bezel (`reshade_files`,
  `d3dcompiler_47`), on DXVK: on wined3d, ReShade's OpenGL hooks looped in
  `wglCreateContext` / `wglCreateLayerContext` and the boot hung at random. Without ReShade
  (the picture sideways), wined3d is needed: DXVK's Direct3D 8 crashes in the game's state block
  calls at its device setup (`CaptureStateBlock` / `ApplyStateBlock`).

#### Friction

Venum Spyder's two-gun shooter (`Friction.exe`, 2006, Direct3D 9, 32-bit; the dump's game set to
1920x1080). `.windowsloader`: `Friction.exe`. The game loads its gun board's library
(`LoadLibraryA("vsIOBoard.dll")`, its functions by name, cdecl); the dump has a community
replacement (keyboard / mouse / XInput, DemulShooter's target, its own patches of the game):
the payload is the library instead (`payload_install: [VSIOBOARD.dll]`), board `vsioboard`
(`vsioboard.rs`):

- **Listener.** `vsIOBoard_AddListener` registers a C++ object; the board's thread calls its
  methods (thiscall) every 16 ms: `[0](event)` each gun's state (`{u32 2, u32 player, u16 1,
  u16 x, u16 y, u16 buttons}`: x 160..480 and y 120..360 the picture's width and height, mapped
  by the game through `vsIOBoard_Calibrate`; buttons 5 the trigger, 2 reload), `[1](mech)` a coin
  (1 / 2 the players' slots, 3 a service credit: one coin), `[2](1)` the service button (the
  operator console: next item; start selects), `[3](player)` a start button.
  `vsIOBoard_SetUpGun(player, auto, _, frames)`: auto fire, a held trigger reported every
  `frames` events. The other functions answer as the community library did.
- **Game patches.** The community library's `RebootDisable` / `DInputDisable` patches of the
  game: the profile's `WAL_PATCHES` (the reboot skipped, the game's DirectInput mouse and keyboard
  reading skipped); its DirectInput is the common fake one.
- **Free play.** None in the game (coins per play / continue, `HKLM\Software\VenumSpyder`, read
  at `0x44b680`, default 2): with `WAL_FREEPLAY`, the coin object (`[0x5450c4]`: coins `+0`, a
  play's price `+4`, at most 420) is kept at 99 plays (the game shows CREDITS: 99) and its spend
  (`0x44ba47`) takes nothing (`sub [esi], eax` at `0x44ba64` made `nop`s).
- **Shot = start.** A trigger press is also its player's start (`[3]`, a bit per player): a shot
  starts in the attract, joins player 2, continues. The listener's start does nothing while the
  operator console is open (`[game+0x85]`), where the guns' own trigger acts.
- **Crosshairs.** The common crosshair option (`WAL_CROSSHAIR`, on by default) draws the guns over
  each `Present` (P1 blue, P2 red, hidden after a few seconds without moving); the dump's ReShade
  (`d3d9.dll`, crosshair pictures) is hidden. Off the screen, the trigger reloads
  (`gun.offscreen: b6`).
- `WAL_VSIOBOARD_TRACE=1` logs the guns' events at each button change.

#### Wild West Shoot Out

Yingqi's two-gun western shooter (`CowBoy.exe`, Unity 5.6, Mono, 32-bit, Direct3D 11; the player
is the executable). The dump keeps BepInEx 5.4 (Unity Doorstop's `winhttp.dll`) and Argonlefou's
DemulShooter plugin (`BepInEx\plugins\WildWestShootout_BepInEx_DemulShooter_Plugin.dll` +
`.ini`): it skips the dongle check, stands in for the serial board (`LSerialPort`: its packets
made from the plugin's buttons) and the camera guns (`BaseGun.GetGunPos` / `GetGunRealPixPos`,
the crosshairs placed by `GameUIController` / `ShowGamePointController`). `.windowsloader`:
`CowBoy.exe`. No board: the payload is the plugin's client (`demul.rs`, `WAL_DEMUL_PORT=33610`):

- **Reflection packets (`WAL_DEMUL_FIELDS`, `WAL_DEMUL_PLAYERS`).** The plugin's input class is
  serialized by reflection, its public fields sorted by name, each player array as long as its
  players: here `Axis_X[2]`, `Axis_Y[2]` (`f32`), `EnableInputsHack`, `HideCrosshairs`,
  `Reload[2]`, `Trigger[2]` (22 bytes). The profile lists those fields, the payload writes each
  (the aim, the natives `trigger` / `reload` / `action` / `change`, the inputs hack on, the
  crosshairs hidden when `WAL_CROSSHAIR=0`); the plugin draws the game's own crosshairs, so the
  payload's Direct3D 11 dot stays off. Player 2's coin key from `WAL_DEMUL_KEYS_P2`
  (`key_5=key_6`: each player's key in place of the profile's).
- **Aim units (`WAL_DEMUL_RESOLUTION`).** The plugin takes Unity's `Screen` pixels (y from the
  bottom). Unity full screen at its own resolution draws the 4:3 picture centered in the
  1920x1080 window, black bars around (DXVK's swap chain stays the window's size): the aim is the
  gun's place in that picture, in the resolution's pixels. The resolution is fixed by the
  profile's Unity arguments (`-screen-width 1440 -screen-height 1080`; its registry default
  was 1024x768).
- **Inputs.** The cabinet has no start buttons: a shot starts (start = the trigger). Coins are
  the plugin's keys 5 / 6 (one coin each), its `0` opens the settings menu (test: next item),
  Enter selects (service). Reload: `b3`, or a shot off the picture / with the gun off screen.
- **Settings.** The profile installs the dump's `GameConfig.xml` (and `GameCount.xml`, credits
  from zero) at each start; the payload's new `WAL_ARCADEPC_XML` (`xmlset.rs`: `<Tag>` values of
  an XML file, `{freeplay}` / `{crosshair}` the common options) sets `Free` from `WAL_FREEPLAY`
  and `FrontSight` from `WAL_CROSSHAIR`.
- **Doorstop and the payload's hooks.** The payload's d3d9 shims hook the game modules'
  `GetProcAddress` imports (`late_hooks.rs`), Doorstop's own `winhttp.dll` (in the game's
  directory) among them; the executable's import was already Doorstop's hook, kept as the
  shims' original: Doorstop calling the real function through its import reached the shims,
  then itself, endlessly (the main thread's stack overflowed, the process died without an exit
  call, `wine-preloader` SIGSEGV in `setup_raise_exception`). An import already holding another
  DLL's hook is now left alone (never the original of the payload's).
- Verified headless (2026-11-10): attract with free play, starts by a shot (both players),
  stage map, shots scoring (headshot at the gun's exact place, both crosshairs where aimed),
  reload, coins with free play off (3 coins a game), settings menu opened and entered.

#### Mars Sortie

MRCQ's four-gun shooter ("TJZS", `Shooter.exe`, Unity 5.4, Mono `mono.dll`, 64-bit; the player
is the executable). The dump keeps BepInEx (Unity Doorstop's `winhttp.dll`) and Argonlefou's
DemulShooter plugin (`BepInEx\plugins\MarsSortie_BepInEx_DemulShooter_Plugin.dll`): it stands
in for the GMB I/O board (`MRCQ.Hardware.GetInputAxis` "x01".."y04" from its aim, `Update`'s
trigger as each station's `startGameKey` and `fire`), skips the SuperDog check and the board /
laser camera errors, and remaps the game's coin keys 1..4 to 5..8 (`SignalKey`, keeping
space and the arrows). `.windowsloader`: `Shooter.exe`. Board `marssortie` (`marssortie.rs`)
next to the plugin's client (`demul.rs`, `WAL_DEMUL_PORT=33610`):

- **Plugin packets.** The reflection layout (see Wild West Shoot Out): `Axis_X[4]`, `Axis_Y[4]`,
  `ChangeWeapon[4]`, `EnableInputsHack`, `HideCrosshairs`, `Trigger[4]`; players 2..4 their own
  coin keys (`WAL_DEMUL_KEYS_P2..P4`: 6, 7, 8). The aim goes through the game's laser locator
  (`LaserLocator`: the dump's `LaserCamera.dat` calibration, an identity map but for its
  offsets) times `Screen`: Unity at 1680x1050 (16:10, the profile's `-screen-width` /
  `-screen-height`; the cabinet's run.bat had 1280x800), `WAL_DEMUL_RESOLUTION=1680x1050`.
- **Settings (`marssortie.rs`).** Getters made constants by IL patches before their first call
  (`nop`s, `ldc.i4.<n>`, `ret`, so callers inlining them compile the patch; Unity 5.4's scripts
  run in a child domain, its static fields out of the payload's reach): free play
  (`WAL_FREEPLAY`) as the coins per game / per continue 0 (`GameDataBase.GetCoinsPerGame`,
  `GetCoinsPerGame2` and the instance getters), the crosshairs (`WAL_CROSSHAIR`) as
  `GameData.DisplayHairCross` (the game's own crosses); the players' photos are the
  game's (its webcam when present, blank photos without one: both work). The Sinden lightguns'
  cameras are hidden from its webcams (`webcam::skip_sinden`, payload-common: the DirectShow
  device list's `IEnumMoniker::Next` skips a `FriendlyName` containing "sinden"): the game takes
  the first webcam, on the box a gun's (held by the Sinden software).
- **Crosshairs of the players only.** The game shows every station's cross (`HairCrossManager`),
  and with free play (price 0) every station plays from a stage's start: `HairCrossManager.Update`
  is rewritten in place (same size) to `SetActive(ScoreManager.IsPlaying(i) &&
  PropWeaponSpawner.IsDefaultWeaponMounted(i))` each frame, the default weapon being given at a
  playing station's first shot: a cross appears from its player's first shot in a stage, none in
  the attract or the stage select. Checked against the shipped body (this build's tokens).
- **Operator menus.** Space (OK) in the attract opens them (Device, Account, Setup, License,
  Leaderboard). They are Unity UI driven by the keypad: `MRCQ.UIInputHelper` calls
  `keybd_event` (keypad 8 5 4 6 +) for the board's up / down / left / right / OK, messages the
  keyboard filter drops: that `DllImport` is the payload's (`mono_imports`), the keys posted as
  the payload's own, a key's per-frame repeats dropped.
- **Records.** `GameRecords1.dat` / `GameRecords2.dat` (credits, accounting, the price) hidden
  from the run folder: the game makes its own there (price 2 coins, 1 a continue), the dump's
  untouched.
- Verified headless (2026-11-10): attract in free play, both players started by a shot, stage
  select, shots scoring for both (30, 30, 10, 20), both crosshairs where aimed, coins counted
  per station with free play off, operator menu opened, navigated, Setup entered.

#### Family Guy Bowling Arcade

Team Play's trackball bowling game (`bowling.exe`, Unity 5.4, Mono, 64-bit). The dump is the
Linux cabinet's game folder (`bowling.x86_64`, `bowling_Data` with `Plugins/x86_64/
libteamplayio.so`, `libserialio.so`) with Unity's Windows player added, and a BepInEx loader
(`winhttp.dll`) the profile disables (`winhttp: b`): the game runs unmodified.
`.windowsloader`: `bowling.exe`. Board `teamplay` (`teamplay.rs`):

- **The I/O library.** The game's `DllImport("teamplayio")` / `("serialio")` have no Windows
  library: stand-in handles (`mono_imports::stand_in`), every function the payload's. The
  board's input state (`IO_GetInputState`: start `0x400`, left `0x200`, right `0x800`, select
  `0x1000`, adjustables `0x40`, service credit `0x8`; the game makes the edges), coins as cash in
  cents (`IO_DefineCoinMech`'s cents a coin, `IO_NewCoinCount`, `IO_CashAvailable`,
  `IO_UseCash`; the game calls `NewCoinCount` with the mech as its board), secure with no
  cabinet flags (`IO_IsSecure`), connected, versions, the time; outputs, tickets, comm links,
  the printer's serial port accepted and dropped.
- **The trackball** (`IO_SpinnerGetCurrentValue`, spinners 8 / 9 read once a frame, times 0.033
  in the game, spinner 8 negated): the mouse's wrapping counters (`mouse_wrap` on `rx` / `ry`:
  a real trackball or a mouse) since the last read, plus the left stick's tilt (up to
  `WAL_TEAMPLAY_STICK` = 45 counts a frame: the back swing wants 0.87 a frame, the throw 0.435
  for 8 frames).
- **Settings.** The profile installs the dump's `Adjustables` (and `.bak`: `key|value|` text)
  at each start, the payload sets its `freePlayOn` from `WAL_FREEPLAY`; the audits are hidden
  (made in the run folder).
- Verified headless (2026-11-10): attract, start, game type, players, bowler select by the
  trackball's x, back swing and throws (pins down), coins (25 cents each) with free play off,
  the operator menu (test) navigated by the trackball, System Information entered.

#### Pop Moto

Tecway's motorbike racing game (`fightmoto.exe`, the 3D GameStudio engine `acknex.dll`,
Direct3D 9, 32-bit; English build `fightmoto_En10_3_19-10`). `.windowsloader`: `fightmoto.exe`.
The dump carries a community `USB_board.dll` (with its source: XInput, keyboard, raw mouse), a
dgVoodoo `D3D9.dll` and ReShade's `d3d11.dll`: the profile hides both renderers (Wine's
Direct3D 9) and installs the payload as `USB_board.dll` (board `popmoto`, `popmoto.rs`), an
engine plugin the game's scripts call (cdecl, by name):

- **Inputs.** `getJoy_x(0)` the balance plate handlebar (0..255, 128 centered: the left stick,
  or full lock with left / right), `getJoy_x(1)` / `getJoy_y` the throttle lever (128 + the
  accelerator - the brake: the pedals, or b1 / b2 full), `isButtonDown` 0 start, 1 item (b3),
  2 test, `getCoinSum` 1 per coin. Tickets, counters and lamps do nothing.
- **Engine patches.** `A6_DLLAPI` (the plugin entry, called when the engine loads the
  library) does what the community library did: the compiled scripts' code (at
  `[acknex.dll+0x21ab1c]`) gets two `sete al` (+0x5283c, +0x17ef7f) replaced by `nop`s, and
  `GS_XSocket_CS.dll`'s `x_connect_to_sever` returns at once (one cabinet, no server).
- **Free play.** `config.ini` `[Config]` `gamemode` (1 free: FREE MODE on the screens) set from
  `WAL_FREEPLAY` at start, the file's other bytes kept (GBK comments, CRLF).
- Verified headless (2026-11-10): attract, coins (CREDIT counter), free mode, start, mode /
  team / mission select with the handlebar, a race (throttle to 260 km/h, steering, the item
  prompt). The boot's WMV logos show black; the test menu not reached yet. User 100%
  (2026-11-10). Pads: the triggers are the throttle lever (r2 forward, l2 back); keyboard:
  Left / Right handlebar, Up / Down throttle (ramped 0.3 s).

#### Subway Surfers Arcade

An arcade conversion of Kiloo / SYBO's Subway Surfers (`SubwaySurfers.exe`, Unity 2022.3, Mono
BleedingEdge, 64-bit; Portuguese class names). The dump carries a BepInEx loader
(`winhttp.dll`) with a community stub of the cabinet's I/O (`CabinetIOStub`: COM5 disabled):
the profile disables it (`winhttp: b`). `.windowsloader`: `SubwaySurfers.exe`. Board `subway`
(`subway.rs`), loaded at the entry point (`payload_loader`; `xinput1_4` off, `xinput1_3` the
payload), the input filter installed once `UnityPlayer.dll` is loaded:

- **Keys.** The cabinet's controls are keys read with `Input.GetKeyDown`: `Game.Update` calls
  `HandleDebugControls`, where the arrows (or the keypad's 8 / 2 / 4 / 6) are swipes
  (`CharacterState.HandleSwipe`: up jumps, down rolls, left / right change lanes; its letter
  keys are cheats, never posted); `TrocaModeloPersonagem` (character select) left / right;
  `InsiraCreditos`: `5` released (or keypad 5, the `COIN:` button) adds a credit, `1` (keypad
  1, the `Start` button) starts a run with one, Escape quits (never posted). The payload posts
  them (`input_filter::post_key`) from the virtual sticks; the left stick presses an arrow once
  per tilt past half way (again once back near the middle).
- **Hoverboard.** Only the touch screen's double tap (`Game.HandleControls`: Unity's left mouse
  button, `HandleTap`): b2 posts two taps in the picture's middle (mouse messages and raw
  reports).
- **Free play** (the common `WAL_FREEPLAY`): start posts a coin (`5` pressed and released), then
  `1`.
- **Lamps.** `ArduinoController`: COM5 at 115200 bauds, `<pin>.1x` / `<pin>.0x` writes (pin 3:
  the start lamp), nothing read. A silent device of the payload's (Mono's `SerialPort`
  `DllImport`s through `mono_imports`): the port never reaches a real serial port.
- Verified headless (2026-11-10): attract (INSERT CREDIT(S)), free play start, lane changes by
  the d-pad and the stick, jumps, rolls, the hoverboard.
- The user: 100% (2026-11-10, desktop).

### 6.3j UNIS (`crates/payload-unis`)

UNIS's PC-based arcade games. System `unis`, payload `wal_unis.dll` (64-bit; `wal_unis32.dll`
for 32-bit games) installed as `xinput1_3.dll`, which Unity loads at start: no pads, the real
keyboard and mouse filtered, the common crosshair option. The Unity games' `DllImport`s are
resolved through Mono's `GetProcAddress` (`payload-common`'s `mono_imports.rs`), the guns'
place on the picture is `payload-common`'s `gun.rs` (both shared with the arcadepc payload).

#### Dino Invasion

The "shunxiang" gun cabinet (Unity 5.4, Mono `mono.dll`, 64-bit; `WAL_UNIS_BOARD=shunxiang`,
the default), four guns. The dump is rebuilt without the repack's BepInEx, DemulShooter
plugins, launcher, logs and the root copies of the plugins; the camera tracker's `FTD2XX.dll`
(the dump's `ftd2xx64.dll`) is in `DinoInvasion_Data/Plugins`, so the real `LC64.dll` loads.

- **Board (`shunxiang.rs`).** `serial_manage_shoulie` on COM3 (19200 bauds), Mono's
  `SerialPort`. Frames `55 5A`, the coins inserted per gun (8 counter bytes), the start,
  trigger and function bits (bytes 10, 11, 12; function 0 the menu, 1 the selection), two
  zeros, the XOR of the 15 bytes before; at most every 16 ms. The PC's `55 5B` frames (the
  guns' blowers) are ignored.
- **Guns.** `LC64.dll`'s `LCGetPoint(gun, x, y, brightness)`: the gun's place on the picture,
  0..1 from the bottom left (the camera's y up). The game maps it through its calibration
  (`Assets/config/game/hwok_tmp/data1.txt`: its first four points the corners (0, 0), (1, 0),
  (0, 1), (1, 1): the aim unchanged). The game draws no gun cursor: the crosshair option on.
- **Dongles.** `DllBindFunc`: present, verified, the licence `dog_check` compares (95 215 208
  249 248 165 248 66); `Sense4`: every call succeeds.
- **The game scene's gun hardware.** The game scene reads its coins, triggers and operator
  keys from `gun_hardware_shiyu`: a separate I/O server's data (`shiyu_gun_work`, a TCP
  client of 127.0.0.1:5000, absent: the boot's "connection refused"); `gun_hardware_shunxiang`
  reads a scene's own `serial_manage_shoulie`, idle (only the boot scene's, kept across
  scenes, runs the port). Both classes' `get_coin_count`, `get_gun_status`, `get_key_func` and
  `is_read_serial_data` are replaced (`Mono::replace`) by the payload's, reading the active
  board manager (`serial_manage_shoulie.myserial_manage_shoulie`). They are replaced once the
  game's domain runs (its board manager started): replaced earlier, the code is the root
  domain's and the game compiles its own.
- **Play.** A coin joins its player (bullets), no start key.

#### After Dark

UNIS's After Dark (1.1, `after-dark`; 2.3, `after-dark-2-3`: the same `GamePlayer.exe` and
profile, another `main.vmo`): a Virtools 3.5 player linked into
`GamePlayer.exe` (32-bit, MFC 9.0: `vcrun2008`), the game in `main.vmo` and `Module/*.nmo`;
`WAL_UNIS_BOARD=afterdark`, two guns. The executable imports none of the payload's names:
`wal-loader` starts it with the payload (`xinput1_3.dll`, native first).

- **Player settings.** `GameConfig` (an ini, `[Settings]`, absent from the dumps; the profile's
  `files`): `FileName=main.vmo` (default `NtmGame.vmo`), `TexturePath=Textures`,
  `SoundPath=Sounds`, the window 1920x1080 (default 800x600, the 16:9 picture letterboxed in
  it), made full screen by `WAL_D3D9_FULLSCREEN`.
- **DirectInput.** The input manager creates it with `CoCreateInstance(CLSID_DirectInput)` and
  `Initialize(GetModuleHandle("ck2.dll"), 0x700)`: the player linked in, no `ck2.dll`, Wine
  refuses the null instance ("Initalization Error"). `WAL_DINPUT_DISABLE`'s fake DirectInput
  also answers `CoCreateInstance` (`dinput.rs`).
- **Serial gun board (COM1, 19200 bauds).** The executable's building blocks (Gun Status,
  Internal Coins) read 26-byte frames `40 80`, gun A's and gun B's buttons (bytes 2, 3: 0x01
  start, 0x02 trigger, 0x04 the motion seat's button, 0x08 the operator menu, 0x10 / 0x20 its
  + / -, 0x80 a coin, counted once per 100 ms held), X and Y of each gun (bytes 7..14, big
  endian, 0..4095 across the screen from the bottom left), `0D`, the XOR of the 25 bytes before.
  The game maps them through `coin.ini`'s calibration (`Gun1xMin` ... `Gun2yMax`, the operator
  menu's gun settings). The PC's frames (`40 80 ..` outputs, `50 90 0D`) are ignored.
- **HID board (`HID\Vid_0471&Pid_485e`).** The dongle and the guns' vibration, behind an
  encrypted feature-report protocol (the executable's driver: session key, MAC). The payload
  replaces the driver's device object instead: the boards' init (`0x415CA0`, "Init Dongle
  Failed !") puts ours in the board table (`0x8983B0`). The dongle block writes 8 bytes
  (command 0x42) and reads them back (command 0x30, sub-command 1); the I/O thread's polls
  (sub-command 2, the vibration) get 16 zero bytes.
- **2.3.** Its dump has no `D3DX9_41.dll` (1.1's has): Wine's draws its materials black,
  the profile installs Microsoft's (`d3dx9_41`).
- **Focus.** The videos (the attract's, the operator menu's) play through DirectShow, whose
  renderer creates 8x8 Direct3D devices on the desktop window: kept windowed
  (`WAL_D3D9_FULLSCREEN` leaves devices under 64x64 alone), they still take the focus, and the
  player pauses without it (`UseFocus`, default 1): `GameConfig`'s `UseFocus=0`.
- **PC keyboard.** The MFC message loop (`mfc90.dll`) is filtered too
  (`input_filter::messages`): the real F1 was MFC's help (WinHelp, the game hung).
- **Fourth stage** (`afterdark_stage4.rs`, `WAL_UNIS_AFTERDARK_STAGE4`, default on). The stage
  select (`Module/UI.nmo`) opens Arena of Terror once the first three stages are cleared in the
  game: its local boolean `开启第四关`, set by `关卡元素` from the stage flags `第一关`..`第三关`,
  reset with them to false at each game's start (`Int Data`'s Identity), tested by the cursor's
  `关卡判断`. The executable's `CreateFileA` of `Module\UI.nmo` opens a copy (`wal-UI.nmo` beside
  it) where that boolean is saved true and the reset's destination is another flag. The
  composition: `Nemo Fi` header (Adler-32 from 0 of the file, its field zeroed; packed and
  unpacked sizes), the zlib object table (id, class, data offset, GBK name per object), the
  zlib data (per object a state chunk: size, header, dwords; a boolean local's value dword 6, a
  parameter output's destinations from dword 9, object indices).
- **Operator menu.** F2; F2 again opens the selected item, + / - change its value. 2.3's menu
  (`1.vmo`) has two more settings than 1.1's, in `coin.ini`: `[CoinContinue] Coin` and
  `[VideoVolume] Volume`. The dumps' `coin.ini` (1.1's) has neither: the menu opened but took
  no button (its script stops on the empty value; it queries no key, gun or HID input). The
  payload appends the missing sections at the menu's values (5, 2) when absent; the game keeps
  the file after (`complete_coin_ini`).
- **Sound.** `CoFramework.dll` (UNIS's framework) opens its own DirectSound device
  (`DirectSoundCreate8` through `GetProcAddress`), sets its primary format to 8-bit mono and
  loops a 4 s 8-bit buffer filled with 0x00 (8-bit silence is 0x80): a constant -1.0 on its own
  stream, summed with the game's by the sound server, clipped every loud sound (1.1 and 2.3;
  2.3's louder sounds the most). The payload hooks the module's `GetProcAddress`: its device is
  ours in front of DirectSound's, its secondary buffers made with a volume control and set to
  the minimum. The profiles' volume (the common option, §6.7) is 60%: the sounds and music are
  mastered at full scale.
- **Play.** Coins, start; the game draws its own gun cursors (the crosshair option off). The
  game has no free play (its coin setting 0 does not give one): `WAL_UNIS_FREEPLAY` (default
  on, a profile option) makes a start press a coin, then the start.

#### Night Hunter: After Dark Chapter II

The "qumo" gun cabinet (Unity 5.5, Mono `mono.dll`, 64-bit, `qumo2_en.exe`;
`WAL_UNIS_BOARD=qumo`), two guns. Dino Invasion's code base: its LC64 camera and COM board are
unused, `gun_manage_qumo` reads everything from a USB HID board (`hid_for_qumo_recv`). Some dumps
carry a mod loader (`winhttp.dll`, BepInEx and a plugin putting the game on the mouse): the
profile takes Wine's `winhttp`, the game unmodified.

- **Board (`qumo.rs`).** STMicroelectronics 0483:5750, version 0200, opened by `USB_HID.dll`
  (Mono): `SetupDiGetClassDevs` / `SetupDiEnumDeviceInterfaces` /
  `SetupDiGetDeviceInterfaceDetail` (the path, UTF-16), `CreateFile` (ANSI) and
  `HidD_GetAttributes`, then a read handle and a write handle, overlapped `ReadFile` /
  `WriteFile` on its threads (`GetOverlappedResult`, the `OVERLAPPED`'s event set). Those
  `DllImport`s are the payload's: one interface, its path's handles the payload's
  (`0x5A12xxxx`), every other handle the real functions'. The game writes a report each
  update (10 ms at most: recoil, lights; accepted), the payload answers each with an input
  report (else every 50 ms): its event manager hands the game one report a frame, a faster
  board would queue them.
- **Input report.** 16 bytes after the report id 0: `AA`; byte 1 the operator's buttons (bit 2
  test, 1 service, 0 reset); byte 2 each gun's buttons (P1 / P2 bits: 7 / 6 coin, 5 / 4 big
  button, 3 / 2 trigger, also the start, 1 / 0 emergency stop); byte 3 bits 7 / 6 the change
  of weapon (the pump); per gun three bytes (P1 5..7, P2 8..10): x's low byte, y's low byte,
  x's high nibble (bits 4..7) and y's (bits 0..3). The game counts a coin on each press.
- **Guns.** 12 bits, 0..4095 from the picture's bottom left (y up), mapped by the game through
  its calibration `Assets/config/game/excel_data/jiaoqiang.csv` (the dump's: a cabinet's
  guns): the profile's file, every corner at 0 / 4095, the aim unchanged. The game draws its
  gun marks (the crosshair option off).
- **Dongle.** `Sense4` (`dog_hardware_sense_4`, verified at start): every call succeeds
  (Dino Invasion's functions).
- **Operator menu.** Test opens it, reset moves (held: repeats), service selects
  (`config_cehua_houtai`: move `FIX_BUTTON`, OK `SERVER_BUTTON`).
- **Free play.** The operator's setting (`Free_Play` in
  `Assets/config/game/excel_data/data_info.csv`, beside the bookkeeping): set at start from
  `WAL_UNIS_FREEPLAY` (default on, a profile option), the rest of the file kept.
- **Play.** The trigger starts a game, the other gun's trigger joins. Natives `trigger big
  change coin stop` (each gun), `test service reset` (the operator's).
- **3.0.2** (`night-hunter-after-dark-chapter-ii-3-0-2`). The same executable and board code,
  four missions (scenes 1, 3, 5, 6: Ice Castle, Misty Swamp, Lost Trails, Mission 4; 2.0.6
  has two: its scenes 2 and 4 are unfinished, no level data). The game plays the first
  mission without the select (`RUN_FIRST_LEVEL_NO_SELECT`, default on) and the fourth last,
  once the others are cleared (`MUST_PASS_LEVEL_4 = NO`): the profile's
  `config/config_game_run_type.txt` (the dump's) sets `RUN_FIRST_LEVEL_NO_SELECT = NO` and
  `MUST_PASS_LEVEL_4 = YES`. Its select scene (`level0`, `Panel_head/select`) shows three
  islands: the fourth's (`image_level4`, `show4`, `label4`) is inactive, over the third.
  `nighthunter_mission4.rs` (`WAL_UNIS_NIGHTHUNTER_MISSION4=1`, the profile's) gives the
  player's `CreateFileW` of `level0` a copy beside the executable: those objects active
  (a `GameObject`'s `m_IsActive` its last byte), the island at the map's bottom right (a
  `RectTransform`'s `m_AnchoredPosition` / `m_SizeDelta` at bytes 84 / 92), each original
  value checked first. After a mission the game picks the next one at random: the fourth,
  it starts it without the select.

#### Raccoon Rampage

UNIS's water-gun cabinet (Unreal Engine 4.26, 64-bit, `RSGame-Win64-Shipping.exe` 1.4, its PDB
in the dump; `WAL_UNIS_BOARD=usbplugins`), four guns. The executable imports `xinput1_3.dll`:
the payload. The game loads `Plugins/UsbPluginsDll.dll` (the I/O board over USB HID, the guns,
the dongle) and looks its `BIND_*` functions up by name (`FWindowsPlatformProcess::GetDllExport`,
the executable's `GetProcAddress`, hooked): for that DLL the payload answers the I/O, coin and
dongle functions, the others (lights, recoil, settings files) are the DLL's own.

- **I/O (`usbplugins.rs`).** `BIND_GetIoData(FCoUnisIoData&)`, 49 bytes read by
  `ABPGameState::ParseIOData`: byte 2 the operator's buttons (bit 2 test: the menu, its move up;
  1 its move down; 0 its enter), byte 5 the triggers (bit 7 - n gun n: a press starts the gun's
  game, the cabinet has no start buttons) and the water tank's sensors (bit 3 low, 2 medium,
  1 high: medium, the normal level; none = no water, coins locked), bytes 6..9 the ticket and
  card dispensers (clear), 10..25 each gun's raw x, y (big endian, unused).
- **Coins.** Counted by the DLL: `BIND_GetCoinNumByPlayer`, `BIND_DecCoins`,
  `BIND_AddKeyboardCoins` are the payload's counters (a coin press one coin). The game's pay
  setting (operator menu, Game Setting; the dump's `Setting_Pay.ini` is empty) is free play.
- **Aim.** The game maps each gun's raw x, y through the operator menu's grid calibration
  (`Setting_Hardware.ini` `Calibrate_<n>P_<cell>_LT_X`..., `ABPGameState::GunDataToScreen`) to
  its 1920x1080 picture (`FPACKET::iX`, `iY`, then `UBPGameInstance::SetMouseLocation`, the
  game's crosshairs). `GunDataToScreen` (RVA 0x9B3110) jumps to the payload's: the gun's place
  on the picture times 1920x1080; `IsGunCalibrate`'s `xor al, al` (0x9B7290) is `mov al, 1`
  (DemulShooter's Raccoon Rampage module patches the same RVAs). Both checked first: another
  version keeps the game's code.
- **Dongle.** `BIND_InitDongle`, `IsDongleExist`, `IsDongleVerify`, `UnInitDongle`: true;
  `BIND_SetLocalTime` (the menu's clock) changes nothing.

Headless checks: attract, two players joined by their triggers, aim and shots, the operator
menu (test, down, enter: Gun Test, every gun "Corrected").

#### Crazy Ride

UNIS's driving cabinet (Unreal Engine 4.19, 64-bit, `CrazyTaxiGame-Win64-Shipping.exe` 1.9, its
PDB in the dump; `WAL_UNIS_BOARD=usbplugins`, the game told by its executable): the same
`GetProcAddress` answers as Raccoon Rampage's, for `Plugins/UsbPluginsDll_R.dll`.

- **I/O.** `BIND_GetIoData`, 33 bytes (`ACTGameState::ParseIOData`): byte 2 the operator's
  buttons (as Raccoon Rampage's), byte 3 bit 0 the start button (in a race, while a mission
  runs, the nitro: `ACTVehiclePlayerController::ApplyIOData` calls `IO_TurnOnWing`, the
  super nitro, one of the charges; also on b1), 1 the seat motion's pause
  (each press toggles it), 2 the seat belt (set: fastened), 7 a coin (unused); byte 6 the
  brake, 7 the gas.
- **Wheel.** `BIND_IsExistSteeringWheel`, `BIND_OpenSteeringWheelDevice`: true;
  `BIND_GetSteeringWheelAngle()`: 255 full left .. 1 full right (the cabinet's wheel: left the
  high values), 128 the centre. The game maps it and the pedals through the operator menu's
  input calibration (`Setting_Test_Input.ini`, read by `IniFile.dll`, CRLF lines only: the LF
  file was ignored, "Steering wheel or gas pedal is not calibrated"): the profile's matches the
  payload's ranges.
- **Coins.** `BIND_GetInternalCoins()` the credits, `BIND_DecCoins(count)`,
  `BIND_AddKeyboardCoins(count)`. The dump's pay setting is free play.
- **One screen.** `Config/DefaultManage_Menu.ini` `bEnableSplitscreen=true` (`AManage_Menu::
  SetSplitscreen`): a 2520x1080 window, the main picture 76 % of it, the cabinet's 600x800
  touch screen (the ranking list) at its right. The profile's copy sets it false: a 16:9
  window, one picture.

- **Force feedback.** `BIND_SetSteeringWheelData(force, centre, shake, 0)` (the DLL's wheel
  motor): the centring force level (`iSteeringWheelForceFeedback`, 0..10, 3 by default, the
  operator menu's; 0 while the game starts), the centre (128), the shaking level
  (`iSteeringWheelShakingForce`, set while the car hits or bumps, 0 otherwise). The payload
  sends the launcher a centring spring and a vibration, a tenth of the full force per level.
- **Full screen.** The game sizes its window from the screen's height (2133x1200 on a
  1920x1200 screen: its right side cut off): a 1920x1080 desktop (`WAL_DESKTOP_SIZE`), its
  window a borderless popup (`WAL_WINDOW_POPUP`) fitted onto the screen, aspect kept.
- **Keyboard.** The arrows steer (left / right: `lx`) and drive (up the gas, down the brake),
  pushed over 500 ms (`keyboard_ramp_ms`); the operator menu opens on test only (up is the
  gas).

Headless checks: attract, start, car select, a race (gas, steering both ways), the operator
menu (Hardware Test, Input Test), the force feedback outputs.

#### Crazy Speed Arcade

UNIS's "game232" driving cabinet (`release/Client_r.exe`, Ogre 1.x, 32-bit, no symbols;
`WAL_UNIS_BOARD=game232`, `crates/payload-unis/src/game232.rs`), started with the payload by
wal-loader.

- **Board.** COM1, 19200 bauds 8N1, overlapped reads of one byte: the board streams `AA AA`,
  10 data bytes, their CRC-16/Modbus (low byte first). Data: three axes (16-bit little endian,
  0..1024, the game's value `1 - raw / 1024`: wheel, gas, brake), 20 buttons (bytes 6..8),
  byte 8 bit 7 a coin (counted on each frame it is set: one frame per press). The axes go
  through `DATABASE/RuntimeSettings/AnalogAdjust.ini`'s calibration (the profile's: 0..1).
  Buttons, through `configure/js_game232_map.dat`: 5 START/NOS (confirm, nitro), 4 VIEW (the
  camera; on the title the game card's renewal), 9 the operator's (it quits the game: not
  mapped), 0 / 1 the menus' up / down (the vertical lists: the d-pad, the keyboard's Page Up
  / Page Down, the game's own keys for them). The wheel selects in the horizontal menus (the
  d-pad's left / right turn it fully); the arrows steer and drive.
- **Display.** The game asks for a 1360x768 full screen mode (constants in its code), which
  Ogre refuses when the screen lacks it: `WAL_D3D9_WINDOWED_SIZE=1360x768` (the mode added to
  the adapter's list, the device made windowed) in a borderless window (`WAL_WINDOW_POPUP`) on
  a 1360x768 desktop (`WAL_DESKTOP_SIZE`) fitted onto the screen, aspect kept. Ogre sizes its
  view from the window's client area: a screen-sized window (`WAL_WINDOW_SIZE: screen`, the
  first setup) zoomed the 1360x768 picture, its right and bottom cut off. Ogre's render
  system (`RenderSystem_Direct3D9.dll`, loaded after the start) makes the window: the fake
  desktop's hooks reach the DLLs of the game's directory loaded later (`late_hooks.rs`), and
  the window's client area answers in desktop units (`WAL_DESKTOP_CLIENT`). No force feedback:
  the game writes its board two settings at the start and the cabinet's lamps, nothing in a
  race.
  Ogre creates the device from `RenderSystem_Direct3D9.dll`, loaded later by `OgreMain.dll`:
  the d3d9 shim's imports are now late hooks (`late_hooks.rs`), every DLL of the game's
  directory loaded later gets them.
- **Shaders.** The dump's `d3dx9_28.dll` native (Wine's compiler dropped the shaders'
  constants: "Parameter ... does not exist").
- **WMI.** `Win32_LogicalMemoryConfiguration` (absent from Wine) enumerates nothing: the
  checks after `IEnumWbemClassObject::Next` are made `jne` (no object skipped).
- **Machine id.** A licence key bound to the cabinet ("UNIDENTIFIED SYSTEM ID"): its check
  answers valid.
- **Dongle.** The game's actions (64-byte blocks) are RSA-encrypted with a public key in the
  game (OpenSSL) and decrypted by the dongle (`dic32u.dll`, the private key): without it every
  dispatched action came out garbled ("no module process this action", START spending a
  credit, the title staying). Both functions return at once: the actions in clear.
- **Offline.** The network engine's connection thread (to an internet dispatch server, every
  30 s) crashed on its queue (its global overwritten at the engine's start; the crash dialog,
  "CrazyForSpeed", took the screen): it ends at once, and `connect` to non-local addresses is
  refused. The engine's shutdown skips the queue's release (crashed at the exit). The game shows
  OFFLINE and plays Tournament / Time Attack.
- **Rear view mirror.** Its material's empty texture unit drew it white (the game adds the
  mirror's render texture as the next unit): the profile's material has none.

Headless checks: coin, credits, START, game card (timeout: no card), car and course select, a
race (gas, steering both ways, the mirror).

#### Elevator Action Invasion

UNIS / Taito's two-gun elevator cabinet (Unreal Engine 4.26, 64-bit, `ESGame-Win64-Shipping.exe`,
its PDB in the dump; `WAL_UNIS_BOARD=usbplugins`, the game told by its executable): the same
`GetProcAddress` answers as Raccoon Rampage's, for `Plugins/UsbPluginsDll.dll`, its "ESIO"
board and USB guns.

- **I/O.** `BIND_IsExistESIODevice`, `BIND_OpenESIODevice`: true; `BIND_GetESIoData`, 17 bytes
  (`ABPGameState::ParseIOData`, into `FIoData`): byte 2 bit 0 the operator's enter, 1 down,
  2 up (test: the menu), 4 start 2P, 5 start 1P; bytes 3, 4 the left and right doors' sensors
  (low nibble: the door closed, `LeftDoorVal`; bit 7 `bInfraredHit`, bit 6 `bInfraredError`:
  the doors' hand sensors, clear); bytes 5..12 each gun's raw x, y (0..4095, big endian: the
  board's guns, used when the settings' gun type is 1); byte 13 the triggers (bit 1 1P, 0 2P).
- **Guns.** `BIND_GetGunData(player, FPACKET*)` (`ABPGameState::Tick`, the gun type 0, the
  dump's): the payload writes the gun's place on the 1080x1920 portrait picture (`iX`, `iY`;
  the game clamps them to 70..1010, 70..1850 and moves its crosshair there), the trigger
  (`bLeftBtn`: its edges fire), the buttons, `iUSB` 1 (`ABPGameState::IsGun` wants it above 0:
  0 crossed the guns' icons out, a negative one skips the gun) and a calibration (`LeftTop`,
  `Middle`, `RightBotton`: the picture's corners and middle; none, "enter Setting Menu to
  calibrate!" in the attract).
- **Coins.** `BIND_GetCoinNumByPlayer`, `BIND_DecCoins`, `BIND_AddKeyboardCoins`: the payload's
  counters, as Raccoon Rampage's.
- **Doors (`eai_doors.rs`).** The cabinet's elevator doors, in front of the screen, are the
  game's: `ABPPC::SetDoorVal` → `UBPGameInstance::SetDoorData` → `BIND_SetDoorData(left, right)`
  (each door's closed part, 0 open .. 10 closed: closed for each elevator ride, opened at the
  floor). The payload moves them at the cabinet's pace (1.5 s open to closed), reports their
  positions as the sensors and draws them over the picture (`WAL_UNIS_DOORS`: the dump's
  `Attraction Doors Pack/1920x1080/Portrait` pictures, `DoorLeft.png`, `DoorRight.png` and the
  `DoorsBezel.png` frame; `payload-common`'s `images_d3d11.rs`, textured quads drawn at the
  swap chain's `Present` with its own device context state). The game's door video
  (`ShowDoorMV`, cabinets without doors, `bHasDoor` false) is not used.
- **Levels.** After the first mission (the helicopter's) the game shows its mission select.
  The game keeps no progress (no save game: the levels' passed flags are `UGameData`'s, in
  memory). A level is unlocked when every level of its list (`FSelectLevelData`) is passed
  (`UGameData::GetLevelIsUnlock`): its `je` there (RVA 0x7687CE, checked first) is a nop,
  every level unlocked.
- **Focus.** A shot beside the picture (a mouse click on the bezel or the desktop) took the
  game's focus: `WAL_DESKTOP_ACTIVE` (the window never deactivated), the bezel's strips answer
  `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`.
- **Display.** The portrait picture: `WAL_DESKTOP_SIZE=1080x1920` (the game's window that size,
  fitted onto the screen, aspect kept), the landscape set's `Bezel.png` around it
  (`WAL_BEZEL`, `WAL_BEZEL_FIT`).

Headless checks: attract (the doors closing and opening), coins, start, a game (the doors at
each ride, aim, shots, enemies hit).

#### Wicked Tuna

UNIS's four-player fishing cabinet (Unity 2018, Mono, 64-bit, `WickedTuna.exe`;
`WAL_UNIS_BOARD=wickedtuna`, `crates/payload-unis/src/wickedtuna.rs`): four rods (a reel's
handle, the rod turned left and right and raised, a hook button) on one serial board. Unity
tries `xinput1_4.dll` first: disabled, the payload (`xinput1_3.dll`) is loaded at start. The
dump's mod loader (`winhttp.dll`, BepInEx and its input plugin) is not used: Wine's `winhttp`.

- **Board.** `IOBoardControler` scans COM 2..10 (`iobaordCom.dll`'s `exIO_Initiate(port,
  115200)`), then reads 48 bytes (`exIO_ReadData`) after each command (`exIO_SendCmd`, 26
  bytes: lights, the reels' and rods' resistance, shakers, tickets: ignored). The payload
  answers those `DllImport`s (`mono_imports.rs`). Data: four 12-byte records, byte 0 `A1` +
  the player, byte 2 bits 0..2 the reel's position (each change one step of the line: the
  game's `GetRollingRope`), 3 the hook button (cast, confirm; menus move with the reel), 4 a
  coin (counted on each read it is set: one read per press), 5 / 6 / 7 the operator's repair,
  service, test; bytes 3, 4 the rod's direction, 5, 6 its angle (12 bits each, high nibble
  first), 7..9 the tickets left, 11 the complement of the sum of bytes 0..10.
- **Rods on light guns.** The game maps direction and angle through the operator menu's
  per-player ranges (`logs/Data.dat` `AdLeft` / `AdRight`, `AdMin` / `AdMax`; the dump's:
  player 1 a cabinet's, 2..4 min = max), then turns the rod and drops the hook at
  `FishPointRange - dir × 2 FishPointRange` (`PlayerCtrl::StartFlyHook`), and asks for the rod
  raised (≥ 0.7) or lowered during a fight. The payload: the gun's x the direction (the
  board's from the right: 0 the rod turned right; checked headless, the rod's tip follows the
  aim), its y the angle (aimed up: raised), the trigger the hook button, a held button the
  reel (one step per read, the board read every other frame). `WAL_UNIS_TUNA_AIM=column` (the
  default): each gun's x across its player's quarter of the picture (the four players' columns,
  left to right); `screen`: the whole picture (pads, a mouse). The profile's `logs/Data.dat`
  (installed at each start, as the dump's otherwise: free play, English) has every range
  0..4095, its `logs/Data2.dat` the bookkeeping cleared.
- **Sound.** Bass streams opened with a speaker flag per player (`GF_SoundManager::
  InitSound4`: 1 front left, 2 front right, 3 rear left, 4 rear right, music on all): on a
  stereo device BASS refuses the rear ones (`BASS_ERROR_SPEAKER`, 42: players 3 and 4
  silent). The payload's `BASS_StreamCreateFile` (the same `DllImport` hook) opens a refused
  stream again on the front speaker of its side.
- **Online.** `UnisINTR.dll` (UNIS's network service) is off in the game's code
  (`isUseUnisINTR = false`).

Headless checks: coin, join on free play, mode and character select, the fish radar, casts,
fights (the line reeled in, the rod raised and lowered), fish landed (a shark), the rod's
direction both ways, player 3's sounds on the front speakers.

#### Fantasy Soccer

UNIS's four-player football cabinet (Unreal Engine 4, 64-bit, `FB-Win64-Shipping.exe` 2.4,
no PDB; `WAL_UNIS_BOARD=usbplugins`, the game told by its executable): each player has a stick,
a red and a blue button and a real ball kicked against a sensor. The game loads
`Plugins/UsbPluginsDll_R.dll` as Crazy Ride does.

- **Packed executable.** The executable's code is unpacked at run time (sections without
  names): it reaches the DLL's functions past the hooked `GetProcAddress`. Once the DLL is loaded, each export the payload answers is patched to jump to
  the payload's function (`mov rax, f; jmp rax`; every export is 16 bytes or more). The board's
  layout was read from the unpacked image (dumped from memory by the payload during the work):
  the game state's tick reads `BIND_GetIoData` into its 0x511 and parses it.
- **I/O.** 33 bytes. Byte 3: the operator's (bit 7 test: opens the menu, moves down; 6 up,
  the up arrow and the d-pad's up; 5 enter). Byte 4 bit 3 - n: a button of player n's (not on the operator menu's input test).
  Bytes 5, 6: the ticket dispensers (a nibble each player, clear). Byte 11 + n: player n's ball
  sensor, the kick's strength (the game takes it when it rises from 0: one read per kick).
  Byte 15 bits 7 - 2n: the red button (accelerate; any button starts), 6 - 2n: the blue one.
  Bytes 16 + 2n, 17 + 2n: the stick's x and y, 255 full left / up .. 0, 128 the centre (dead
  zone 20). The operator menu's input test (Main Menu, Input Test) shows each part.
- **Calibration.** `Setting_Test_Input.ini` (`IoDataList_iMin_X_%dP`... the sticks' and the
  balls' ranges, `bIsVerify_Rocker` / `bIsVerify_Football`, CRLF lines): without it every slot
  warns "The joystick or the ball or the camera is not verified". The profile's has every
  player's; `Setting_Webcam.ini`'s `bEnableWebcam=false` (the players' photos: no camera).
- **Coins.** `BIND_GetInternalCoins(player)`: the player's coins since the start (the game adds
  each increase to the player's credits; three a game).
- **One screen.** `AManage_Menu`'s `bEnableSplitscreen` (on: a 3840x1080 window, the cabinet's
  second screen's video beside the game, "Video File Missing") is set off in the user's saved
  `Manage_Menu.ini` before the game reads it (`WAL_UNIS_SPLITSCREEN=1`: both screens). The
  engine's `-ini:` command line override did not take.

- **Full screen.** The game sizes its window from the screen's height (cut on 16:10): a
  1920x1080 desktop fitted, its window a borderless popup. The executable's import table is
  filled after the payload's start (packed): `WAL_PACKED_IMPORTS=user32.dll`, its image's
  slots of the hooked functions replaced as they appear (`iat.rs` rescans the image for 20 s).
  The engine's `-fullscreen` did nothing: the game sets its windowed resolution itself
  (`r.SetRes %dx%d%s`, its width the height × 16/9).
- **Keyboard.** The arrows move the player (the stick); the up arrow is also the menu's up.

Headless checks: coins (3/3), two players joined, mode select, a match (moves, accelerate,
kicks), the operator menu and its input test (buttons, calibration).

#### Mighty Moto

UNIS's motorbike cabinet (Unity 2.x, 32-bit, `motobikeV1.0.9.exe`, 2009;
`WAL_UNIS_BOARD=mightymoto`, `crates/payload-unis/src/mightymoto.rs`), started with the payload by
wal-loader (no XInput import; `xinput1_3: "n,b"`, else Wine's builtin is loaded).

- **Keys.** The cabinet's controls are keys (its board a key encoder): the scripts read Unity's
  `Input` (`headScript`: `1` start, `2` the item, `3` a coin, `4` the operator menu, the
  `Horizontal` axis the handlebar, the `d` / `up` buttons the gas, `u` the brake; `myCar`: `s`
  back, space the handbrake; `houtai`, the operator menu: `j` / `l` move, `d` enters). The real
  keyboard is filtered; the payload posts these keys to the game's window as its own key
  messages (`input_filter::post_key`, held for the key state polls) and gives the fake
  DirectInput (`WAL_DINPUT_DISABLE=1`) the same keyboard. The stick's x past a third of its
  travel is the arrows, the gas and brake pedals (`accel`, `brake`) the keys too.
- **Settings.** `SaveData.xml` beside the executable (free play on in the dump), written back by
  the game: the profile installs a copy of the dump's.

- **Keyboard.** The arrows steer and drive (up the gas, down the brake), Page Up / Page Down
  move in the operator menu. No force feedback: the game drives nothing (no board output; its
  network code is SmartFox, the linked cabinets' races).

Headless checks: attract, start, track select, a race (gas, steering both ways, overtakes).


#### Big Foot Crush

Unity 2022.3, Mono, 32-bit (`WAL_UNIS_BOARD=bigfootcrush`, `crates/payload-unis/src/bigfootcrush.rs`),
started with the payload by `wal-loader`. The Windows build of an Android cabinet's game: the board
bridge (`Framework_Call_JNI`) is empty, nothing comes from the MCU.

- **Settings.** The dump lacks the APK's `StreamingAssets/Setting`; the game reads
  `C:\mnt\sdcard\UNIS_GameData\Setting` (`c_folders`, the profile's `files`): `Config.ini`,
  `PlayerSetting.ini`, `Menu0/ConfigReadOnly.ini` and `Menu0/Framework_Menu_Main.ini` are made for
  this cabinet. `GlobalSetting.GetMenuSetting` throws on a key without a menu row, so every key
  the game asks is a row (hidden when it does not apply: tickets, the cabinets' link). An empty
  `BootLoader/BinName.ini` passes the boot's comparison with the missing APK file.
- **Keys.** `W` the gas, `A` / `D` the wheel (`CarAll`), `1` a coin, `M` test, the arrows the
  menu's values (`Framework_Keyboard_Test`), posted from player 1's stick. The wheel is `A` / `D`
  out of the menu: the left arrow is the operator's free coin there. Unity 2022's `KeyCode` is the
  place on a US keyboard of the key a virtual key has in the host's layout (on AZERTY `VK_W` reads
  as `Z`, whatever the message's scan code): each is posted as the virtual key at that place
  (`MapVirtualKey(scan, MAPVK_VSC_TO_VK)`). Headless runs have a US layout:
  `XKB_DEFAULT_LAYOUT=fr` tests another.
- **Operator menu.** Its keys go through the link between a cabinet's two PCs: `GetData` /
  `SetData` of `Framework_UART_Manager` replaced to answer it (`GetFromOther2` = 1, `GetFromOther4`
  echoes `SendToOther4`), the MCU's polling coroutine ended (its IO ERROR). Leaving the menu, an
  Android player restarts; `CallBackState.OnMenuEvent`'s IL is patched to load the game's scene as
  the editor does. `Mono::replace` has a 32-bit jump for this game.
- **Picture.** Portrait (the logo, truck select and counters are laid out on 9:16): a 1080x1920
  fake desktop, fitted on the screen, black around.
- **Attract video.** AVPro Video has only its Android backend here (no native plugin): its
  `DisplayUGUI` stayed grey. `payload-common`'s `avpro.rs` replaces `DisplayUGUI.get_mainTexture`:
  a Unity `VideoPlayer` added to the `MediaPlayer`'s object plays its `m_VideoPath` from the
  streaming assets (`m_Loop`, no sound, `APIOnly`), its texture is the display's (black until the
  first frame; `LateUpdate` sees the change). The movie is MPEG-4 Part 2: the runner's winedmo
  needs GE patches 0113-0115 (GE-Proton11-12 runner, §5.3).
  The payload attaches to Mono only once `Assembly-CSharp` is loaded (attaching while Mono
  started crashed the game).
- **Not in the dump.** The maps' `SLFramework/Inis` tools (`IniManager`'s initializer also fails on `AndroidJavaObject`: the
  update check and these tools are skipped, harmlessly).

### 6.3k Adrenaline Amusements (`crates/payload-adrenaline`)

Adrenaline's PC-based arcade games. System `adrenaline` (Batocera's ES `adrenaline`), payload
`wal_adrenaline.dll` (32-bit) or `wal_adrenaline64.dll` (64-bit games) installed as
`xinput1_3.dll`: no pads, the real keyboard and mouse filtered (the games' own coin, operator
and quit keys). The Unity games' `DllImport`s are resolved through Mono's `GetProcAddress`
(`payload-common`'s `mono_imports.rs`). Both games have the Matrix dongle (`matrix.rs`).

#### Skylanders Cloud Patrol

Unity 4.2 (Mono `mono.dll`, Direct3D 9, 32-bit), a touch screen game (the mobile game's arcade
port). The dump is the cabinet's `DataPC` folder (`Skylanders.bat` updates it with
`Skylanders.jar`, then runs `Skylanders.exe`): `skylanders-cloud-patrol.windowsloader` points at
`Skylanders.exe`. Unity loads `xinput1_3.dll` after its Direct3D device: `payload_loader: true`
loads it at the entry point, so the crosshair overlay hooks the device.

- **Touch frame (`pqmt.rs`).** `PqmtScreenGestures` is the client of a PQ Labs frame's server
  (`PQMTClient.dll`, TCP 127.0.0.1:21555): `ConnectServer`, `SendRequest`,
  `GetServerResolution` (callback), the point frame callback (`SetOnReceivePointFrame`, cdecl:
  `TouchPoint { point_event 0 down / 1 move / 2 up, id, x, y, dx, dy }`; a finger missing from a
  frame is lifted). Each player's finger (id = player) is down where its gun points while
  `touch` (the trigger) is held, frames every 16 ms. The game maps a point to Unity's screen as
  (`x`, `resolution.y - y`) times desktop / resolution in integers: the resolution reported is
  0.6 times the screen's (the factor 1), the points in client pixels. The callbacks come from a
  payload thread attached to the game's Mono domain (`mono_thread_attach`; unattached, Unity 4's
  Mono crashes in the callback's wrapper), started at the game's first
  `SetOnReceivePointFrame`: a thread started before `mono.dll` loads has no slot in its static
  TLS (crash in `mono_thread_attach`).
- **Kaboom board (`kaboom.rs`).** `KaboomComDll.dll` (cdecl, `KABOOM_DLL_ERROR` byte 0 = no
  error): `InitLib(port)` on the first port, two coin counters (`GetCurrentCoinCount`, reset by
  `ResetCoinCount` after crediting: P1 on counter 1, the others on 2), the keypad's four press
  counters (`GetKeypadCount`: any change toggles the operator menu: our `operator`), the ticket
  dispenser (`Set/GetTicketFeederTicketToFeedCount`, `GetTicketFeederCount` fed / empty,
  `ResetTicketFeeder`): a ticket fed every 100 ms, never empty.
- **Dongle.** `matrix32.dll` (stdcall, the online device id only): `Init_MatrixAPI`,
  `Dongle_Count` 1, `Dongle_ReadSerNr` a serial, `Release_MatrixAPI`.
- **Play.** 4 coins a game (operator setting), then touches: element and character select,
  shooting. Natives `touch coin` (each player), `operator`. The common crosshair option (d3d9).

#### Infinity Blade FX

Unreal Engine 3 (Direct3D 9 / 10, 32-bit, a mixed-mode executable run by Wine Mono), the
touch screen sword game: swipes swing the sword, taps block and dodge. The dump's root holds
`infinity-blade-fx.windowsloader` (`Binaries\Win32\SwordGame.exe`), which imports the payload
(`xinput1_3.dll`): no Mono `DllImport`s, the hardware is hooked in the executable's imports.

- **Touch frame.** `PQMTClient.dll` imported (`ConnectServer`, `SendRequest`, `SendThreshold`,
  `SetOnReceivePointFrame`, `SetOnGetDeviceInfo`, `DisconnectServer`): `pqmt.rs`'s client
  through IAT hooks, the callback called from the payload's thread without a Mono attach, the
  points in client pixels (the game never asks the resolution). One player: P1's mouse or gun.
- **Gun swipes.** `WAL_GUN_SWIPE=<gain>` (profile option, default 1): while the trigger is held
  the finger moves `gain` times as far as the gun from where it was pulled. The game's swipe
  zones need 200 px at 900 px/s (`SwordGame.ini`): at gain 5 a flick of 6% of the screen's
  width hits (headless: the boss from 180 to 100 health), at gain 1 the same flicks do nothing.
- **Shield and dodges.** Fixed fingers, wherever the gun points (`pqmt.rs` `FIXED`, ids the
  player + 4 / 8 / 12): native `shield` (profile: `b2`, right click / the gun's second button)
  held on the block zone (`SwordShieldBlock`, x 0.41-0.59, y 0.8-1.0: the shield stays up, its
  count drops per blocked hit), `dodge_left` / `dodge_right` (`left` / `right`: the arrow keys,
  the gun's d-pad) on the dodge arrows (`SwordDodgeLeft` / `Right`, the bottom corners).
- **Kaboom board (`kaboom.rs`).** The board's library is built into the executable
  (`CKaboomCom`, `\\.\COMn`): packets `F10E` request / `B00F` reply, a 16-bit sum of the
  length, command and data bytes, u16 length and command; commands 6 keypad (18 bytes, the
  menu key's count at byte 14), 8 / 9 tickets to feed, 10 fed + empty, 11 feeder reset, 12 / 13
  the coin counters. Answered on any COM port (`serial.rs` `COM*`).
- **Dongle.** `matrix32.dll` imported by ordinal (#40 `Init_MatrixAPI`, #55 `Dongle_Count`,
  #61 `Dongle_ReadSerNr`): `matrix.rs`. The dump's `matrix32.dll` and `dinput8.dll` are a
  third-party loader shim (it asks for another loader's DLL): Wine's `dinput8`, our imports.
- **Engine settings.** The profile's `SwordEngine.ini` (`files`): `AllowJoystickInput=0` (with
  no pad the engine's joystick loop writes through a null device at the title: crash at
  `SwordGame.exe+0x18e488b`) and `AllowD3D10=False` (Direct3D 9: the crosshair, screenshots).
  Full screen from `-ResX= -ResY= -fullscreen` (the Resolution option).
- **Play (headless).** 4 coins a credit, touch: title, single player, the fight.

#### Fruit Ninja FX 2

Halfbrick's own engine ("Mortar", native, 32-bit, Direct3D 9), the touch screen fruit slicing
game. The dump's `DataPC\Fruit Ninja FX 2 Arcade.exe` is an AutoHotkey script running
`FruitNinja_2010_ATeamBuild_patch.exe` (not wrapped by the dongle's envelope):
`fruit-ninja-fx2.windowsloader` points at the latter. It imports no `xinput1_3.dll`:
`payload_loader: true`. Saves (`ArcadeCoins.json`, operator settings...) in `..\` (the dump's
root).

- **Touch frame.** `PQMTClient.dll` imported, as Infinity Blade FX: `pqmt.rs` through IAT
  hooks. The game asks the resolution but takes the points in client pixels (native mode).
  Several players' fingers at once (one per gun).
- **Gun slice.** A gun's shot is a point, the fruit is cut by swipes: `WAL_GUN_SLICE=<length>`
  (profile option, default 0.25 of the picture's height) makes each trigger pull a slice
  (`pqmt.rs` `Slicer`): down at the aim, out half the length, back through the aim to the
  other side, back to the aim, 8 frames of 16 ms, run to its end even if the trigger is
  released; held, the finger then follows the gun. Its direction is the gun's move over the
  last 6 frames, else alternate 30° diagonals. Menu buttons (powers, START's melon, the
  operator menu's arrows) get their finger down and up on the aim. Headless: random shots over
  a 60 s round cut 42 fruit.
- **Kaboom board.** Built in, on `\\.\COMn` (COM1-19): the game opens the first port answering
  command 24 (the board's version, two words: `kaboom.rs` answers 2, 2), then polls 6 (keypad:
  the operator menu), 12 / 13 (coins), 11 (feeder). 4 coins a game (operator setting).
- **Dongle.** `matrix32.dll` by ordinal: Infinity Blade's #40 #41 #55 #61 and #36
  `GetVersionAPI`, #50 `Dongle_Find`, #52 `Dongle_Version`.
- **Figurine portal (`fruitninja.rs`).** `NfcReader.dll` (cdecl, C++ names) opens COM19 down
  to COM1 itself (the machine's real ports): its imports answered "no reader" (`NfcOpen` 0).
  The power is picked on the screen instead.
- **Heap slack.** The game's bilinear image scaler (`+0x5b0a10`, at load) reads the next source
  row at the last one, a row past the buffer: on Wine a large block can end at a page's end
  (access violation at start, 2 runs in 7). The executable's `HeapAlloc` / `HeapReAlloc`
  blocks of 64 KB and more get 64 KB more (8 runs in 8 clean).
- **ReShade.** The dump's `d3d9.dll` + `ReShade.ini` (its shaders not in the dump) hidden:
  Wine's Direct3D 9, the common crosshair.
- **Fruit Ninja FX (the first one).** Its original `FruitNinja.exe` is wrapped by the Matrix
  envelope. REA analysis recovered this build's table-derived XTEA key without a dongle:
  its optional dongle key transformation is disabled. `FruitNinja.decrypted.exe` restores
  the three protected sections, imports and original entry point. Reproduction and
  reusable investigation notes: [FRUIT-NINJA-FX-DECRYPTION.md](FRUIT-NINJA-FX-DECRYPTION.md),
  `tools/scripts/fruit-ninja-fx-decrypt.py` (exact input hash enforced).
  `fruit-ninja-fx.windowsloader` points at the decrypted executable; the profile is FX 2's
  without its ReShade `hide` (none in this dump). Same hardware path: `PQMTClient.dll`
  imported (8 functions), the built-in Kaboom board on `\\.\COMn` (commands 13, 6, 11 then 12
  / 6 polled: coins, keypad, feeder). No `matrix32.dll` and no `NfcReader.dll` import (the
  dongle was the envelope's only), so no ordinal hooks, no figurine portal, no heap slack
  (not needed: clean starts). `Sleep(15000)` at start, after the window is created (the
  cabinet's boot wait, `+0x7bd89`): `WAL_PATCHES` makes it `Sleep(0)` (else a white window and
  GNOME's "not responding"). Version 1.4.0.0. Headless: 4 coins a credit, the Single
  Player melon sliced, a 60 s round (34 fruit from random shots), bonus screen, 7 tickets
  fed; F1 operator menu.

#### Jetpack Joyride Arcade

Halfbrick's own engine (native, 32-bit, Direct3D 9). On the cabinet `C:\Joyride\Joyride.bat`
updates `C:\Joyride\DataPC` and runs `mgjp fullscreen` there; `mgjp.exe` is wrapped by the
Matrix dongle's envelope (its code decrypted with the dongle over HID). The dump's
`mgjp_standalone.exe` is the unwrapped executable (dongle check removed, imports rebuilt):
`jetpack-joyride-arcade.windowsloader` points at it, `fullscreen` its argument. It imports no
`xinput1_3.dll`: `payload_loader: true` (and `xinput1_3: n,b`, else Wine's builtin is loaded
under the payload's path).

- **Folders.** The game reads its data from `..\DataPC\` and writes its saves (credits,
  tickets, operator settings: `ArcadeCoins.json`...) in `..\`. The profile's `c_folders` map
  `C:\Joyride` to the dump's `Saves` folder and `C:\Joyride\DataPC` to the dump;
  `WAL_ADRENALINE_CWD=C:\Joyride\DataPC` is set by the payload before the game's code runs.
- **Dongle.** `matrix32.dll` imported by name (`GetVersionAPI`, `Dongle_Find`, `Dongle_Version`
  too): `matrix.rs` `hook_names` (the unwrapped executable no longer checks it).
- **Board (`kaboom.rs`).** `KaboomCOMBoardManager` loads `KaboomComDll.dll` with
  `LoadLibrary` and resolves its 25 functions with `GetProcAddress` (any missing: no board):
  the executable's import hooked (`mono_imports::hook_game`). `InitLib(port, keypad, coins,
  tickets, x)` takes cdecl callbacks, called from the board thread: keypad `(keys)` on a change
  (operator 1, down 2, select 4, up 8), coins `(counter 1, counter 2)` running totals (the
  game credits their increase; `ResetCoinCount` zeroes them). The unwrapped executable had the
  `InitLib` call replaced by `xor eax, eax` and 6 nops (no board opened, no coins): the profile's
  `WAL_PATCHES` puts `mov ecx, [esi+0x10c]; call ecx` back (RVA `0xE552D`). Tickets:
  `SetTicketFeederTicketToFeedCount` / `GetTicketFeederCount` (a ticket every 100 ms); the lamps
  (`SetOutput`, `ClearOutput`, `ClearExtendedOutput`) are ignored.
- **Button.** The jetpack button is on the keypad: any of the down, select and up lines flies
  (button codes 1-3 pressed, 5-7 released), the operator line opens the operator menu from the
  attract and title states (in the menu: down / up / select). The game's DirectInput keyboard
  (it counts the keys held) sees nothing (`WAL_DINPUT_DISABLE`).
- **Play (headless).** 4 coins a game (operator setting), the button: the 45 s run, then the
  tickets (20 fed). Natives `button coin` (P1), `operator up down select`.

#### Rabbids Hollywood

Unity 2017.2 (Mono, Direct3D 11, 64-bit), four guns: Sixense STEM packs (electromagnetic
trackers, position and rotation) around a STEM base. `rabbids-hollywood.windowsloader` points at
`Game.exe`. Some dumps carry a Unity Doorstop proxy `winhttp.dll` loading BepInEx and a mouse
plugin: the profile sets Wine's own `winhttp` (no plugins, the hardware emulated instead). The
game draws its own crosshairs.

- **Trackers (`sixense.rs`).** `sxCore.dll` (cdecl): device 0 the base (`device_type` 14),
  devices 1-4 the guns (`device_type` 4, `device_type_extended` = the gun's number).
  `GunDetection` waits for the enabled guns plus the base
  (`sxCoreGetNumConnectedTrackedDevices`), flashes devices whose firmware differs (base MCU
  01.10, packs MCU CA.18 / DSP DA.17: reported as such), waits for the guns' auto-sync
  (`tracker_id` 7), maps them to the players. `sxCoreGetData` every frame: status, buttons
  (`START`: the trigger), position (mm, Unity flips z), rotation quaternion (Unity flips z and w).
- **Aim.** `InputCollisionMesh` casts a ray from the gun plus the receiver's place
  (`m_ReceiverOffsetY - m_SizeY / 2`, `m_ReceiverOffsetZ`) along the gun's forward axis onto the
  screen plane (`m_SizeX` x `m_SizeY` m, `ControllerBaseConfig.json`), to the viewport. Each gun
  stays at the receiver (position 0: independent of the scene's tracker scale, never "docked")
  and turns towards the point where the player's gun aims.
- **Gun mode.** `NewInputManager.ChooseInput` (its first frame) takes the guns only if
  "GunReady" came first, else the mouse for good; the dump's assembly sets "gun not found" in
  its `Awake` (mouse at once). A second after `GunDetection`'s setup (`sxCoreSoftReset`), from
  `sxCoreGetData` (main thread), the payload clears `m_GunNotFound`, sets `m_GunReady` and calls
  `ChooseInput` (Mono's embedding API) while `m_InputMode` is the mouse.
- **Kaboom Adrio board (`adrio.rs`).** `KaboomAdrioFxPlusFourFeeders64.dll` (cdecl,
  `KABOOM_DLL_ERROR`): `InitLib(port 2..19, keypad, coin, ticket, player button, spinner
  callbacks)`, `GetTargetVersion` 9 / revision 1102 (another revision: error popup), feeders'
  counts 0 after the reset. Callbacks: keypad `un16AllKeys` (operator 1, down 2, select 4, up
  8), coins (16-byte structure by value: its address; four doors of two counters, running
  totals), player buttons (`un16AllButton`: P n's start at bit 2n, button 2 at 2n + 1). They are
  called from `GetTicketFeederCountAdrio5`, which the game's board thread polls after every
  frame (a managed thread). Tickets: `SetTicketFeederCountToFeedAdrio5({ feeder, count })`
  targets, a ticket fed every 100 ms, never empty.
- **Dongle.** `matrix64.dll`: found as above; the save files' AES key is
  `Dongle_DecryptData` of fixed blocks (the dongle's key) checked against an MD5. Our XTEA
  fails the check: the game picks a random key each run, saves are not read back (operator
  settings at their defaults: 4 credits a game, four guns).
- **Play.** Coins, start, the guns shoot the posters (level choice) and the Rabbids. Natives
  `trigger start coin` (each player), `operator up down select`.

#### Tomb Raider

Unity 2017.2 (Mono, Direct3D 11, 64-bit), Rabbids Hollywood's hardware: four Sixense STEM guns
(`sixense.rs`, the same aim), the `KaboomAdrioFxPlusFourFeeders64.dll` board (`adrio.rs`,
version 9 / revision 1102) and the Matrix dongle. `tomb-raider.windowsloader` points at
`Game.exe`. The dump carries a Unity Doorstop proxy `winhttp.dll`: Wine's own in the profile.

- **Trigger and reload.** The trigger is the board's player button 2 (`un16AllButton` bit
  2n + 1, `InputManager.OnKeyEvent`); the tracker's `START` button reloads. Rabbids reads
  either for its trigger: the `trigger` native sets both there, only button 2 here (the payload
  tells the games apart by Rabbids' `NewInputManager` class); the new `reload` native sets
  `START`.
- **Gun mode.** `InputManager` starts with neither "gun not found" nor "gun ready" and takes
  the guns at `GunDetection`'s "gun ready" by itself (no `sxCoreSoftReset` in this game, no
  Mono patch).
- **Display options.** Resolution (`WAL_RESOLUTION`, default 1920x1080) goes into Unity's
  `-screen-width`/`-screen-height` through the profile's `{WAL_RESOLUTION.width}` and
  `{WAL_RESOLUTION.height}` arguments (any profile argument can use `{NAME}` of its
  environment, `.width`/`.height` the sides of a `WxH` value). The game is GPU-bound on an
  integrated Radeon 860M: 35-42 fps at 1920x1080, 47-57 at 1280x720 (it aims at 60).
  `WAL_TOMBRAIDER_POSTPROCESS=0` (the default; `tombraider.rs`) calls the game's own
  `GameplayCamera.EnablePostProcess(false)` every second (cinematics turn it on again): no
  post-processing stack nor volumetric fog, 40-47 fps at 1920x1080.
- **Saves.** The operator settings (free play...) are AES files whose key is the dongle's
  decryption of `SBK.Encryption.DataTransform.KEY` / `IV`, checked against the MD5s
  `m_KeyHash` / `m_VIHash` (a mismatch: a random key each run, the saves never read back, as
  Rabbids Hollywood). At the first `Dongle_DecryptData` (`matrix.rs`, the game's key thread)
  the payload sets those hashes to the MD5s of our decryption: the same key every run.
- **Play.** Coins, start (the game mode and chapter are chosen by timeout or a shot), then
  shots; an empty clip shows RELOAD. Natives `trigger reload start coin` (each player),
  `operator up down select`; the game draws its own crosshairs.

#### Rampage

Unity 2017.2 (Mono, Direct3D 11, 64-bit): three players, a joystick and an attack button each.
`rampage.windowsloader` points at `Game.exe`. The dump's `sxCore.dll` is unused (no Sixense
calls).

- **Board.** The same `KaboomAdrioFxPlusFourFeeders64.dll` as Rabbids Hollywood (`adrio.rs`,
  version 9 / revision 1102, callbacks), but its board thread calls no function every frame:
  the callbacks come from a payload thread started at `InitLib` (Mono loaded) attached to the
  game's Mono domain, every 10 ms. Player n's joystick is on the player buttons' bits 4n (down
  1, up 2, left 4, right 8); the attack buttons are the keypad's lines (P1 down, P2 select, P3
  up; operator 1), the operator menu's keys too. Coins: doors 1-3.
- **Play.** Coins, attack to join, then the monsters. Natives `stick_up stick_down stick_left
  stick_right attack coin` (each player), `operator`; the profile maps them (`native_map`: no
  start nor select, they would be another player's attack).

#### Hot Wheels: Kings of the Road

Unity 2018.3 (Mono, Direct3D 11, 64-bit): six seats, a steering wheel (with its motor), a gas
pedal and a coin door each. `hot-wheels-kings-of-the-road.windowsloader` points at `Game.exe`.
Some dumps carry a Unity Doorstop proxy `winhttp.dll` (a third-party controls patch for
another board emulation): the profile sets Wine's own `winhttp`. The game photographs its
players with a camera (none here: a grey disc).

- **Boards (`hotwheels.rs`).** `KaboomAdrioHotWheels64.dll` (cdecl, the board's id first):
  three boards, two players each. `InitLib(board, port, keyboard, coin, ticket, steering)` on
  the first port whose `GetTargetVersion` is 9 (revision 1143, else a firmware update) and
  whose `GetPlayerId` is under 8 (its first player's global id: 2 * board). The callbacks are
  static and shared, each with a global player id: keypad `(player, 0, keys)` (operator 1,
  down 2, select 4, up 8, accel 16), coins `(first player, 0, total 1, total 2)` (running
  totals), steering both players' `(player, init in progress, init completed, calibrated (as
  BOOLs), max left, centre, max right, wheel, pedal)`. No game call comes every frame: a
  payload thread (started at the first `InitLib`) attached to the game's Mono domain calls
  them every 10 ms. `GetSteeringStatus` reports a calibrated wheel.
- **Wheel and pedal.** Wheel 0..254 around 127. The pedal reads high released, low pressed
  (the game's default calibration 131 to 55; under 110 pressed, 25 and under a stuck pedal):
  140 released to 50 full, from `accel` (or the `gas` native). The launcher's four players are
  seats 1-4; the operator's keypad is player 1's.
- **Play.** Coins, the pedal to accept, a double press to boost. Natives `gas coin` (each
  player, the wheel on `lx` and the pedal on `accel`), `operator up down select`.
- **Force feedback.** The wheel's motor is a position servo (`SetSteeringParameters(board,
  wheel, target, power 0..100)`, `DisengageSteeringWheel`): the game rests it at the centre
  (power 69), shakes it between both ends for bumps (walls, hazards, coins, cars) and sways it
  in attract mode. On the player's wheel: a centre target as the centring spring, any other as
  a constant force towards it (the distance from the wheel times 4, at most the power);
  `WAL_HOTWHEELS_FFB` scales it (percent). The bumps' power is their own times the online remote
  settings' `steeringBumpMultipler`, 0 offline (no bumps): the payload sets the game's default
  0.9 then (Mono's embedding API). FFBArcadePlugin has no module for the game.

#### Need for Speed Heat Takedown

Unity 2021.2 (Mono, Direct3D 11, 64-bit, Sarbakan): one seat, a steering wheel with its motor,
gas and brake pedals, a nitro button. `need-for-speed-heat-takedown.windowsloader` points at
`Game.exe`. Some dumps carry a Unity Doorstop proxy `winhttp.dll` (a third-party controls
patch): the profile sets Wine's own `winhttp`. No Kaboom board: the game's `ControllersManager`
waits for the steering board, the LED strips and spotlight boards (none here: their managers
give up and retry every few seconds) and, on a motion cabinet (machine name `*MOTION*`), the
pneumatic seat.

- **Port (`steering.rs`).** `SteeringManager` lists the ports (`SerialPort.GetPortNames`: the
  registry's `HARDWARE\DEVICEMAP\SERIALCOMM`, Mono's `RegOpenKeyExW`, `RegQueryInfoKeyW`,
  `RegEnumValueW`, `RegQueryValueExW` `DllImport`s: a fake key listing COM3 only), skips COM1
  and opens each with `SerialPort` (115200 bauds; `WinSerialStream`'s kernel32 `DllImport`s,
  ANSI `CreateFile`, are `serial.rs`'s) until one answers `GET_TARGET_INFO` with product 1;
  firmware 5.35 (else `PicProgrammer.exe` flashes it). The LED strips and spotlight managers
  then try COM3 too: refused (access denied) while it is open, as a real port's exclusive open,
  else their probes would read the board's stream.
- **Frames.** `FE src cmd len(u16) payload.. sum`, the xor of `FE` and every byte after it 0;
  the board escapes a `FE` byte as `FE FE`, the PC only its sum. Board to PC: every poll (the
  game's thread, 15 ms) `EVENT_STEERING_POSITION` (14: wheel -1..=1 positive right, gas and
  brake 0..=1, buttons 1 nitro 2 brake), on a change `EVENT_BUTTON` (10: card 0, keys operator
  1, down 2, select 4, up 8) and `EVENT_COIN` (11: player 0, the coin counter; the game credits
  its increase); `GET_INIT_STATUS` (30) no error. The PC's other commands are acknowledged.
- **Play.** Coins, the gas pedal to join (no start button: the lobby waits for the cabinets
  linked, then the race with AI cars). Natives `gas brake nitro coin operator up down select`;
  the wheel on `lx`, the pedals on `accel` and `brake`. The operator menu (test) is driven by
  the wheel (previous / next) and the gas pedal (select).
- **Force feedback.** The board's firmware makes the forces: the game sends `SET_FORCE_FACTOR`
  (20: 1, out of track 0.5, hit obstacle 0.5, differential 1) at the race's start,
  `GET_CAR_INFO` (19, each physics step: speed 0..=1, hit props (destructables, landings: ±11),
  side hit (the car's lateral velocity change, -50..=50), vibration frequency 30 and intensity
  (drift, off road: up to 0.22), off road, vibration on) and turns the motor on and off
  (`ENABLE_WHEEL` 16, `DISABLE_WHEEL` 17). Played on player 1's wheel: a centring spring of
  25% plus 50% times the speed (times the force factor), the side hit / 20 as a constant force
  (held, fading in 120 ms; positive pushes right), the vibration intensity times 4 and the
  props' hits (|props| / 11, fading in 250 ms) as a vibration; `WAL_NFS_FFB` scales them
  (percent; its presence enables the board). The forces are traced in the payload's log (a
  line a second at most).
- **Render scale (`hdrp.rs`).** GPU-bound on small GPUs (HDRP, Radeon 860M at 1920x1080: 27
  frames per second in a race). `WAL_HDRP_RENDER_SCALE` (default 0.67) forces HDRP's dynamic
  resolution: the pipeline asset's `m_RenderPipelineSettings.dynamicResolutionSettings` (found
  from `RenderPipelineManager.s_currentPipeline`'s `m_Asset`, a Mono-attached payload thread,
  again whenever its values come back) set enabled, software, forced at the scale, the FSR 1
  upscaler (`EdgeAdaptiveScalingUpres`); the game's cameras do not allow dynamic resolution:
  `DynamicResolutionHandler.SetCurrentCameraRequest`'s IL patched to store true, at the game's
  first `DllImport` (before HDRP's first frame compiles it). The HUD stays at full resolution.
  0.8: 38-48 frames per second, 0.67: about 50.

#### Hot Wheels: Victory Lap

Unity 6 (6000.0.40, IL2CPP, Direct3D 11, HDRP, 64-bit, Sarbakan): one seat, Need for Speed Heat
Takedown's steering board (wheel, gas and brake, the boost on the nitro button, keypad, coin
door), LED strips and a pneumatic seat. `hot-wheels-victory-lap.windowsloader` points at
`Hotwheels.exe`; the dump's Unity Doorstop proxy `winhttp.dll` (a third-party patch): Wine's
own `winhttp`. The profile's `WAL_NFS_FFB` enables NFS's board (`steering.rs`, COM3 listed).

- **Ports (`rs232.rs`).** The game's `SBK.IOBoardLib.SerialProtocol` (the steering board, the
  LED controllers, the seat) opens its ports with `RS_232_DLL.dll` (Teunis van Beelen's RS-232
  library, cdecl, ports by index: `RS232_GetPortIndexByName("COM3")` 2), not Mono's
  `SerialPort`: its `DllImport`s are the payload's, on `serial.rs`'s port (`CreateFileA`
  through `steering.rs`: exclusive). The LED manager probes COM3 first (`GET_TARGET_INFO`,
  product 1: "Not a LED controller on COM3", closed), then the steering manager keeps it. The
  game writes a frame in pieces (`FE`, header and payload, sum): gathered into one packet.
  `RS232_PollComport` never waits and, after a poll that read data, reads nothing for 10 ms:
  the board's stream is endless (a frame at each empty poll), and the LED probe's read loop,
  reading while bytes come, never ended (the game stuck before its first scene).
- **Play.** As NFS: coins, the gas pedal joins and selects (car, model), the wheel moves; races
  with AI cars ("waiting for other player" times out). The operator menu (test): down next
  tab, select. `GET_CAR_INFO` comes each physics step: NFS's forces.
- **Webcams (`victorylap.rs`).** The cabinet's camera takes the player's picture before the
  race (`SBK.FaceDetection.FDManager`, OpenCV) when the operator's "Take Picture" is on (its
  default). `OperatorUtil.get_TakePicture` is replaced (IL2CPP's API, at the dongle's first
  call): false, no capture (the boxes have no camera; the menu still shows the setting on).

#### Spider-Man Homecoming

Unity 5.5 (Mono `mono.dll`, Direct3D 11, 32-bit), a portrait cabinet: two players, one button
each (the Spider-Sense reflex game). `spider-man-homecoming.windowsloader` points at `Game.exe`.

- **Portrait screen.** `ApplicationManager.Start` sets `Screen.SetResolution(1080, 1920, true)`:
  Unity 5.5 changes the display mode (`ChangeDisplaySettingsA`) to 1080x1920, none of a
  landscape screen's (a black screen from the start on NVIDIA's drivers, native Windows too).
  `WAL_DESKTOP_SIZE: 1080x1920` (`desktop.rs`): a mode change to the desktop's size is done
  without changing anything, `GetDeviceCaps` gives its size, the window is fitted onto the
  screen. Unity reads the screen's size before it loads `xinput1_3.dll`: `payload_loader: true`;
  `-screen-width 1080 -screen-height 1920` (else Unity's size saved in the registry).
- **Kaboom Adrio two-feeder board (`adrio2.rs`).** `KaboomAdrioFxPlusTwoFeeders.dll` (cdecl):
  `InitLib(port 2..19, keyboard, coin, object detected, object not detected, wheel spinner)`.
  The keyboard callback gets the keys held (operator 1, down 2, up 4, select 8), the coin
  callback the two counters' running totals. No game call comes every frame: the callbacks
  come from a payload thread started at `InitLib` (Mono loaded) attached to the game's Mono
  domain. The players' buttons are the keypad's up (P1) and down (P2), which also move in the
  operator menu; coins: P1 on counter 1, P2 on counter 2. Tickets:
  `SetTicketFeederTicketToFeedCount(1, 2)`, `GetTicketFeederCount` fed / empty (a ticket every
  100 ms, never empty), `ResetTicketFeeder`.
- **Older four-feeder library (`adrio.rs`).** The game's `KaboomAdrioFxPlusFourFeeders` (not
  its scene's board) polls `GetReadInputAdrio5` (active low; coins at bit 4 + 2 * door) and
  reads a door's counter on a coin press, adding one: emulated too.
- **Play.** 4 coins a player, the player's button to join, then the button during the
  Spider-Sense. Natives `button coin` (each player), `operator select`.

#### Drakon: Realm Keepers

Sarbakan's Unity 2021.3 game (IL2CPP: `GameAssembly.dll`, Direct3D 11, 64-bit; the dump is the
development build 227996): two turrets on a dragon ride. `drakon-realm-keepers.windowsloader`
points at `Game.exe`. The game's code is compiled: the payload (`drakon.rs`) finds its classes
and methods by name through IL2CPP's embedding API (`payload-common`'s `il2cpp.rs`:
`il2cpp_class_from_name`, `il2cpp_class_get_method_from_name`, a method's code the first field
of its `MethodInfo`), once IL2CPP runs: at the Matrix dongle's first function resolved.

- **DllImports.** Unity 2021's IL2CPP resolves them in `baselib.dll`
  (`Baselib_DynamicLibrary_GetFunction`): its `GetProcAddress` import is hooked too
  (`mono_imports.rs`). The Matrix dongle (`matrix64.dll`) is `matrix.rs`; the save files are
  plain JSON (`AppData/LocalLow/Sarbakan/09038_Adrenaline_Skyride_Turret/savegame`).
- **Serial ports.** The turrets' controllers (`SBK.TurretControllerLib`: yaw / pitch, two
  triggers, LEDs, shaker) and the pneumatic seats talk through `RS_232_DLL.dll` on the ports
  of `HARDWARE\DEVICEMAP\SERIALCOMM`: hidden (`lib.rs`), the game opens none (and
  `RS232_OpenComport` fails). The development build plays without them, in mouse mode. No
  Kaboom board either (the build's `KaboomBase` has no board left).
- **Guns.** `InputsManager.Update` (each player's mouse, every frame) is replaced: the payload
  calls `SendButtonEvent(player, trigger)` and `SendPositionEvent(player, viewport)` for each
  player of `playersData`, the gun's place on the game window from -1 to 1 (y up), as a turret's
  `OnReceiveTurretPosition`; off the screen, the last place. The game draws each turret's
  reticle.
- **Keys.** The cabinet's inputs are keys of the scene's `SBK.InputMapping.KeyboardPressInput`s
  (Unity `KeyCode`s): coin doors `1` `2` (`CoinInput`; Alt with one: its credits cleared),
  the players' buttons `q` `w` (`InactivePlayerState.OnPlayerStartButtonPressed`: join,
  continue; the screen says "press trigger": the trigger presses it too), the operator's keypad `o`
  (operator menu), keypad `1` `2` `3` (up, select, down). `UnityEngine.Input.GetKeyInt`,
  `GetKeyDownInt` and `GetKeyUpInt` are the payload's: the virtual sticks' keys, a press seen
  once a frame (`Time.frameCount`); the game's other keys (development cheats, Escape) never
  down.
- **Menus.** The level choice's turrets (`WindowTurret`) shoot at the turrets' buttons message
  (`EventTurretButtonsState`) or, in the development build, at
  `Input.GetMouseButtonDown(player)`: `GetMouseButton`, `GetMouseButtonDown`, `GetMouseButtonUp`
  are the payload's too, mouse button n player n's trigger.
- **Picture.** GPU-bound on an integrated Radeon 860M at 1920x1080: 10-15 fps at a level's
  start (URP, 4x MSAA, shadows to 800 m; 60 in the menus). The URP asset of the current quality
  (`UniversalRenderPipeline.asset`), checked every second from `InputsManager.Update`:
  `WAL_DRAKON_MSAA` (GUI option, default 1) its `msaaSampleCount`, `WAL_DRAKON_RENDER_SCALE`
  (GUI option, default 0.8) its `renderScale` with `upscalingFilter` FSR. Measured there: 4x
  MSAA at scale 0.8 10-15 fps, at 0.5 46-58; no MSAA 44-60 at scale 1, 52-60 at 0.8. The payload
  logs the frame rate every 10 s (`Time.frameCount`).
- **WiFi.** The operator menu's WiFi tab opens `wlanapi.dll` (`NativeWifi.WlanClient`):
  Wine's `WlanOpenHandle` fails (the development console shown over the menu); the payload's
  has no adapter.
- **Play.** A coin a player (operator setting), the trigger joins, the level chosen by the
  reticle or the timer. The operator menu is played with the guns (target and shoot).
  Natives `trigger start coin` (each player), `operator up down select`.

### 6.3k Sega ALLS APM3 (`crates/payload-apm3`)

APM3 ("ALL.Net P-ras MULTI Version 3") is the ALLS platform for digitally distributed games
(Under Night In-Birth Exe:Late[cl-r]). The game imports `apm_x86.dll`, the client library of the
cabinet's system (`amdaemon`: keychip, ALL.Net, the Type 4 I/O board, Aime reader, credits,
backup), which the dumps do not include. segatools' `apm3hook` (public domain) emulates that
whole stack under the real amdaemon; here the payload replaces the library instead (system
`apm3`, installed as `apm_x86.dll`), like `NxL2Core.dll` for NESiCAxLive 2:

* **Exports.** All 106 (`apm_x86.def`: MSVC stdcall names, `_Input_isOn@4`; the Rust functions
  are `extern "system"`, ld links the decorated names, `--enable-stdcall-fixup`). Every function
  logs its first call. Strings are UTF-16 (the game converts them with code page 932); the
  keychip / board ids are 0x38-byte buffers the game copies.
* **Inputs.** `Input_isOn(id)` / `isOnNow` / `isOffNow` / `isFlipNow`: ids from the library's
  name table: 0 test, 1 service, 2-5 up down left right, 6 start, 7-14 `push01`-`push08` (the
  8-button Vewlix layout, segatools' apm3io: top row 1-4, bottom row 5-8), 15 home (back to the
  system menu: unmapped). Edges are between two `Core_execute` calls (once per frame). One player
  per cabinet: the game's `CPadInput_APM` reads pad 0 only; versus is cabinet to cabinet. Under
  Night: A B C D = push01 push02 push03 push05 (profile: b4 = push05, b5 = push04).
* **Credits.** The `coin` input adds a credit and calls the game's coin hook
  (`Credit_setCoinInHook`); `Sequence_beginPlay` / `continuePlay` pay 1. `Credit_toString` is
  the number only (the game prints `CREDIT(S) %s`, `FREE PLAY` itself). Free play by default (`WAL_APM3_FREEPLAY=0`: coins).
* **Test menu.** The game reads the test switch itself and calls `Core_exitGame(1)`: the payload
  requests a relaunch `exe=TestModeApp.exe` (`WAL_APM3_TEST`, the dump's `gametest.bat`), whose
  `Core_exitGame` relaunches the game. The process is ended 3 s after `Core_exitGame` (the
  system closes it on a cabinet). Settings: the dump's `testmodesavedata.ini`.
* **Network.** `WAL_APM3_ONLINE=1` (Under Night's profile default): `AllnetAuth_isGood` and
  `Aime_isDBAlive` true; otherwise offline (the game's banner: offline, no Aime card, single
  play only). The game embeds SEGA's ABAAS matching client (`am::abaas`, libcurl HTTP + STUN/TURN
  relay, server from `AllnetAuth_getAbaasLinkServerName`): at boot it waits 1800 frames (30 s,
  countdown `0x9bc8f4` set at `0x52edb7`) for the link before the attract mode; without a link
  server (`WAL_APM3_ABAAS_LINK`) the profile's `WAL_APM3_NO_LINK_PATCHES` make it 1 frame (game
  only, never the test program). Network versus needs an ABAAS server: none is public (ARTEMiS
  and minime have no such title, segatools only redirects its DNS); not written yet. Found with
  `WAL_MEMDUMP` dumps every 2 s: a frame counter stopping at 1800 when the attract started.
* **Blade Strangers (APM3)** (`himekaku.exe`, OpenGL, `apm_x86.dll` delay loaded: the payload
  starts at the game's first APM call). What it needs beyond Under Night:
  - the e-money UI window (`emoney.rs`): the boot waits until a top-level window titled
    `emoneyUI` exists (`EnumWindows` + `GetWindowTextW` every 10 ms; the cabinet's
    `emoneyUI.exe`): the payload makes a hidden one (`WAL_APM3_EMONEY_WINDOW=0`: none);
  - the matching group a letter (`ApmSystemSetting_getMatchingGroup`, default `A`,
    `WAL_APM3_MATCHING_GROUP`): the title indexes a picture with `group - 'A'` (0 crashed it:
    animation 0xffbf);
  - the backup (`backup.rs`): `Backup_setupRecords` `{data, size}` pairs (pointer-sized), two
    small records (8 and 3 bytes) the test menu writes (its settings) and the game copies at boot;
    the game imports no save function (the system saves the memory itself). `backup/<index>.bin`
    loaded at setup (status 0; another size than the file's: their common part), status 1
    without a file (the game's defaults: status 0 with memory never written, the game never starts
    a play); a record whose memory changed is written about every 2 s from `Core_execute` and at
    `Core_exitGame`.
  - the test program `gametest.exe` (`WAL_APM3_TEST`; .NET AnyCPU, WPF on Wine Mono): a 64-bit
    process P/Invoking `apm.dll`: the payload's x86_64 build is installed as `apm.dll` too (the
    same exports, undecorated; also the main payload of 64-bit APM3 games).
  - inputs (`0x6a1bf0`): push01-04 bits 0x8-0x40, push05 0x1 | 0x2, push06 0x4 | 0x80000000,
    push07 / 08 0x80 / 0x100, start 0x800; they are read only ~16 s after the boot (the logos).
  - online (`WAL_APM3_ONLINE=1`, the profile's default: Aime cards): without an ABAAS link the
    studio logo scene waits for its connection (the `je` at `0x4f9c24`) and the network state
    (`0x4aa3d0`: 0 with ALL.Net, the Aime DB, the game server's online flag and the link ready;
    1 without the link: the game starts without asking for a card) needs the link (`0x4aa40b`):
    both NOPed by the profile's `WAL_APM3_NO_LINK_PATCHES`. Then START reads the card
    (`LGameMode_aime_cardRead`, "use the Aime card? yes / no") and the game asks the ABAAS game
    server for the player.
  - the ABAAS game server (`gs.rs`, `am::abaas::GsMain`): POSTs to
    `http://<AllnetAuth_getAbaasGsServerName>/api/<api>` (the name a host, port 80 added: the
    payload's `127.0.0.1`, its connect hook moves port 80 to the title server's, which routes
    `/api/`). A body: a 16-byte header, also the IV (`01 01 01 00`, the game id, 8 random
    bytes), then zlib-compressed JSON, AES-256-CBC encrypted with the title key the game gives
    the client (traced from `BCryptGenerateSymmetricKey`, `WAL_APM3_APMS_TRACE=1`). The response
    header: `01 01`, a status (u16 BE, 0 = success), the JSON's CRC32 and length (BE), 4 bytes.
    `server/alive`: `{"control":{"online":1}}` (the online flag, `0x6a3b2e`). `user/load`
    (`{"user":{"user_id":N,"savedata":{"keys":[..]}}}`, keys with `*` / `?` patterns:
    `league*`, `Ch??rank`): the answer must hold `user` (missing: a crash at `0x68d494`, the
    reader at `0x6a3dca`); `{"user":{"user_id":N}}` is a new player; a known one gets its name and
    the requested saved values. `user/save` (the end of a game: `name` and `savedata`, a flat
    object of strings: league, per-character records, play counts): stored as
    `apms/gs-<aime id>.json`. `user/heartbeat` and the rest: `{}`.
  - no network banner (pink "Aime play and national / in-store matching unavailable"; online, the
    access code hint): the profile's `WAL_PATCHES` (`WAL_PATCHES_EXE=himekaku.exe`, not the test
    program) make the banner's switch on the network state (`0x4aa3d0`) jump to the code hiding
    both its texts (`0x49943e`).
* **Aime card** (`aime.rs`). The `card` input (a toggle: on / off the reader) is the card;
  `WAL_APM3_CARD_AUTO=1` (Under Night's profile): always on, the input takes it off. The
  game's `AMU::PlayState` (`0x43ebf0`, states through the byte table `0x43f6f8`): `Aime_start`
  → `Aime_isBusy` true while no card (`Aime_cancel`: START at the "Data Certification" prompt,
  no card) → a card: not busy, `Aime_hasResult`, `Aime_getAccessCode` (`AccessCode`: 10 BCD
  bytes, then the 20 digits as UTF-16, 0x34 bytes), `Aime_getAimeId` → `AppData::startPlay`
  (aime id; -1 = guest, no server). `WAL_APM3_ACCESS_CODE` (default `50000000000000000001`),
  `WAL_APM3_AIME_ID` (default: the code's last 9 digits).
* **Title server** (`apms.rs`, `cts.rs`). The players' data go through SEGA's `libcts`
  (`am::cts::AppData`): HTTP POSTs to `http://<ip>:80/apms-api/<name>` (StartGame, EndGame,
  ContinueGame, UpdateUser, Heartbeat, GetRanking, Get/SetCollection, Alive, CreateAccessCode,
  ConvertCtsId, SetCabinetProperty), `<ip>` from `DnsQuery_A("<game id>.apm.sys-all.net")`
  (game id `SDCM`; `.test-apm.sys-all.net` when `AllnetAuth_isDevelop`). The payload answers that
  name with 127.0.0.1, moves the game's port 80 connections (WS2_32 `connect`, ordinal 4) to its
  server on the launcher's port + 100 (33800 by default; port 80 needs root on the host). The setup step at `0x43e748` tests a flag
  (`+0x2d0`) nothing sets here: `WAL_APM3_ONLINE_PATCHES` NOPs its `je`. Bodies: a 16-byte header
  that is also the AES-128-CBC IV (request `02 01 03 28`, game id, CRC32 of the data, random;
  response `02 01`, u16 status 0 = success, CRC32, data length, any), then the zlib-compressed
  MessagePack data, encrypted (key `8a8df80d…` from `BCryptGenerateSymmetricKey`, logged with
  `WAL_APM3_APMS_TRACE=1`; PKCS#7). Positional arrays: requests `[[keychip, game id, version, 1,
  time], arguments...]`. Answers (decoders: `PlayerData` vtable `0x7531d0` method 2 by request
  type, `GameData` `0x753188`):
  - StartGame `[[aime id, ""], [first slot, 16]]` → `[[aime id, ""], [name, u16, save], [8
    strings, 8 ints]]`; `save` (a MessagePack str holding bytes) is the game's 0x4000-byte block
    (`0x4bbd90` copies it; bytes 4-7 zero: a new player, `0x4bb930`; else checked: `0xaa` magic +
    checksum, `0x4bbca0`); an empty block crashes (`getSlotData` subscript).
  - EndGame / ContinueGame / UpdateUser `[[aime id, ""], [name, u16, save], slots, play logs]`:
    `save` holds only the changed slots (libcts compares the 0x400-byte slots with the block
    StartGame answered), `u16` the first one (none changed: empty, nothing saved). Merged into
    the stored block of `apms/<aime id>.dat` (a launcher data dir of the game folder), returned
    by the next StartGame padded to the requested slots (16: 0x4000). A replaced block lost the
    unchanged last slots: Guilty Gear then reset the player.
  - GetRanking `[kind, [ranking ids], filters]` → `[[[] per id]]` (fewer crashes the attract);
    GetCollection → `[]`; the others: the header alone.
* **ABAAS game server, all games** (`gs.rs`; Blade Strangers above). The title key is learnt at
  run time: the game's `BCryptGenerateSymmetricKey` 256-bit secret (`WAL_APM3_GS_KEY` hex
  instead); a client in its own DLL (Otoshu DX's Unity plugin `abaasgs.dll`,
  `WAL_APM3_GS_MODULE`) gets its `connect` (ordinal 4) and key imports hooked as soon as it is
  loaded (polled every 2 ms). Every answer carries `control.online` (Senxin Aleste throws
  without it): 1 for `server/alive` (the server), 0 elsewhere (the player's: 1 = still playing on
  another cabinet, "the last play did not end normally"). `user/load` for a new player answers
  `"name":""` and an empty `savedata` object when keys were asked (Senxin's reader requires both;
  the games then use their default name), and `user.rankings` (the requested ranking ids) gets the
  player's score, rank and the ranking's total. `user/save`'s `rankings` (`ranking_id`, `score`,
  `sub_score`, `optional_data`) keep each player's best in `apms/gs-ranking-<id>.txt`;
  `ranking/list` (`rankings`: id, count) answers `{"rankings":[{"ranking_id", "total",
  "records":[{"rank", "user_id", "name", "score", "sub_score", "optional_data"}]}]}` (Otoshu's
  `JTest7` / `JTest5` / `JTest6` classes).
* **Exports' `BOOL`s.** The exported functions answer 32-bit 0 / 1 (not a Rust `bool`, which
  only sets `al`): 64-bit C# games P/Invoke them and read the whole `eax` (Otoshu DX).
* **Aime result after cancel.** The last card read stays readable (`Aime_getAimeId`,
  `Aime_getAccessCode`) until the next `Aime_start`: Otoshu DX cancels the reader, then asks
  the id (`g::GetAimeIDAndLoad`; a failure there never loads the player).
* **Senxin Aleste** (`senxin_aleste_apm3.exe -a`, 32-bit Direct3D 9, M2 ShotTriggers): works as
  the generic APM3 game; test mode = the same program with `-t` (`WAL_APM3_TEST` plus
  `WAL_APM3_TEST_ARGS`, the test program recognized by its arguments). Aime cards on the ABAAS
  game server (records, settings, rankings). Japanese only (`config.txt`'s `LANGUAGE` ignored).
* **Otoshu DX** (`otsdx.exe`, Unity 2018.3 IL2CPP, 64-bit: the payload's `apm.dll`, P/Invoked
  by `GameAssembly.dll`). A touch screen game: `WAL_APM3_TOUCH=1` (`touch.rs`) makes player 1's
  gun (the mouse, or the pointer `lx` / `ly`) the screen and its `touch` input (`b1`) the
  touch, posted as the window's left mouse button (Unity's touch screen is its mouse), the real
  mouse and keyboard filtered (`input_filter`, the Unity player's cursor polls). A red dot where it touches (the game shows no cursor): the common crosshair option (`WAL_CROSSHAIR`, on in the profile), drawn over the Direct3D 11 picture (`pointer_d3d11`). Test mode:
  `otsdx.exe -t` (`gl::Start` sets its test flag when the command line has an argument). Found
  without a decompiler (Hopper ran out of memory): the IL2CPP metadata (v24.1) types, fields and
  method pointers (`CodeRegistration`'s `methodPointers`, found by its count) read by a script,
  then objdump; `WAL_APM3_TRACE_PINVOKE=1` logs the P/Invoke functions the game resolves,
  `WAL_APM3_PEEK=<module>+<rva>,<offset>...` logs bytes of a pointer chain when they change.
* **More ABAAS game server answers.** The answer of a play's start (`user/load` with
  `control.start_game` 1) carries `seq_id` and `token` strings (a counter: Puyo Puyo e-Sports
  reads `seq_id`, the games send them back; `WAL_APM3_GS_SEQ=0`: none, Nosferatu Lilinor refuses
  the player's data with one, "the play data could not be read"); a
  player answer always has `name` (empty for a new player: Puyo's keyless `start_game` load
  needs it). `data/load` / `data/save` (`free_buckets`: the cabinet's settings, Pengo's
  `conf_*`, Puyo's `rule_*`...) in `apms/gs-data.json`, the requested key patterns answered.
* **Same-program test modes.** A test mode that is the game's own program without arguments
  (Puyo Puyo e-Sports, Koihime Enbu) is started with a marker argument the game ignores
  (`WAL_APM3_TEST_ARGS: "-wal-test"`): the payload recognizes it and `Sequence_isTest` answers
  true there.
* **Kasiori** (Unity, 64-bit, Mono): loads the system library by its path, `X:/lib/apm.dll` (the
  cabinet's system drive; without it the APM3 code is off: no credits). The profile installs the
  payload there too (`payload_install: [apm.dll, X/lib/apm.dll]`) and maps X: to that run
  directory folder (`run_drives`, a launcher key: drive letters to run directory folders, with
  the payloads). Test mode `TestMode`.
* **Nosferatu Lilinor** (Unity, 64-bit; test `-test`), **Milkchan** (32-bit, `gametest.exe` .NET
  test program), **Koihime Enbu RyoRaiRai Version 3** (32-bit, `abaasgs.dll` beside the game):
  the generic APM3 game. **Puyo Puyo e-Sports** (64-bit): a new player accepts the terms (the
  dialog's default is "no"); its built-in link (abaaslink ee20f2d) is never set up: the profile's
  `WAL_APM3_NO_LINK_PATCHES` make the icon green and skip the versus box's link tests once the
  Aime server answers. **Pengo! Online** (64-bit, `pen5_test.exe` test program): its menus
  are touched (`WAL_APM3_TOUCH=1`, native game: the posted mouse messages; `b1` both push01 and
  the touch). Its link client is built in (abaaslink 0f03646): without a link server its "link ready" flag
  (`0x1402d0c6c`) stays 0 and the status (the "Aime / versus unavailable" banner, the network
  icon) counts it; the profile's `WAL_APM3_NO_LINK_PATCHES` make the flag's five reads of the
  status code answer 1 (ALL.Net and the Aime server decide); the red touch dot on (Direct3D 11). **Goonya Fighter** (Unity, 64-bit): the game in `game\`, its test program in
  `gametest\` (`WAL_APM3_TEST: ..\gametest\gametest.exe`, the payload in both folders by
  `payload_install`); a Unity game's main thread ends before its process: with a relaunch asked
  the launcher ends the process and relaunches (no crash stop).
* **The link DLL** (`link.rs`): games with the ABAAS link client in `abaaslink.dll` (Kasiori,
  Goonya Fighter, Koihime Enbu) show "in-store matching unavailable" and a network icon not
  green without a link server; `WAL_APM3_LINK_READY` (default on, unless `WAL_APM3_ABAAS_LINK`)
  patches its `linkIsAvailable` / `linkIsChecked` exports to answer 1 once it is loaded (x86:
  `_name@4`, stdcall): no warning, the games still play against the CPU.
* **Dengeki Bunko: Fighting Climax (APM3)** (`DengekiFCI_APM3.exe`, 32-bit Direct3D 9; id
  `dengeki-bunko-fighting-climax-apm3`, the RingEdge version has the plain one): `gametest.exe`
  (.NET) test program. Its network state (`0x4642d0`) is online only with the built-in link
  ready: the profile's `WAL_APM3_ONLINE_PATCHES` make it online (its early exit's default and
  its result). It reads `Aime_getConfirm` after the card: the payload answers AimeDB (2; NoneDB,
  0, was "the Aime card could not be read"). No player data server (the card's player is
  "NO NAME"); a new card agrees to the Aime terms (up for YES, START).
* **Rolling Gunner** (`game01.exe`, 32-bit SDL2, Senxin Aleste's engine): test mode `-t`; its
  sounds read from `C:\SDGW\sound` (the dump's `game.bat` copies `resource\sound` there): the
  profile's `c_folders` link it. Full screen with `-f` (the dump's `fullscreen.bat`; windowed
  without), the test mode too (`-t -f`). The Aime prompt's default is "don't use" (left then
  button 1).
* **Cotton Rock 'n' Roll** (`game.exe`, 32-bit OpenGL, game id SDEM; the ABAAS game server
  client built in): no patch. Test program `gametest.exe` (.NET WPF, 32-bit). The Aime prompt's
  default is neither button (left then button 1 plays with the card); button 1 held on the title
  shows the card's access code.
* **Dead or Alive 6** (`App\muscle_apm3.exe`, 64-bit Direct3D 11, game id SDFM; the ABAAS game
  server and link clients built in; test program `gametest.exe`, .NET WPF). The game reads the
  switches as an Xbox pad's bits (0x140120140) and maps those to its actions by a pad config
  (default at 0x1418c6d00: X Hold, Y Punch, B Kick, LT Special, A H+P, LB P+K, RT H+K, RB
  H+P+K); the profile's `native_map` puts H P K S on b1-b4, the shortcuts on b5-b8. The card
  player is saved only when `user/load` answers `control.online` 1 (`WAL_APM3_GS_ONLINE`, as
  parsed at 0x1401402e3): with 0 the game sends no `user/heartbeat` and no `user/save`. Without a
  link server the network state (0x141731b50: 4 no ALL.Net, 1-2 no Aime / game server, 3 no
  link, 0 all up) is 3: a "network play unavailable" banner and a grey icon;
  `WAL_APM3_NO_LINK_PATCHES` turn that 3 into 0, the default mode becomes in-store versus, the
  CPU fights the player while the game waits for a challenger.
* **Umihara Kawase Fresh! for AC** (`App\umifresh.exe`, 32-bit OpenGL, game id SDGU, the
  profile's `WAL_APM3_GAME_ID`; the ABAAS game server client built in): no patch. Test program
  `gametest.exe` (.NET WPF). The dump's own `apm_x86.dll` (with `apm3_config.exe`,
  `apm3_input.bin`) is replaced by the payload. On the stage list START opens the controls
  settings, button 1 decides. Jump (push05, b5 by default: Z) on b1 (left Ctrl, pad A), b1's
  push01 on b5 (the profile's `native_map`, user's choice).
* **Tapping Skill Test** (`GL_pc.exe`: the dump is the FJ11 update `SDFJ_FJ11_..._1_FJ10`
  flattened, the FJ10 dump's `GS3.PAR` plus `GS3_V10100.PAR`; 64-bit Direct3D 11, game id SDFJ). A touch screen game
  (tap, swipe, pinch): its window procedure takes `WM_POINTER*` (`GetPointerInfo`) and the plain
  mouse messages alike (`WAL_APM3_TOUCH`), and the mouse wheel as the two finger pinch
  (`WM_MOUSEWHEEL` at the cursor: the `pinch_in` / `pinch_out` natives post wheel notches,
  `input_filter::post_wheel`, its mark in `wParam`'s low word, the high word being the delta;
  the launcher's mouse wheel sources `wheel_up` / `wheel_down`, a notch held 64 ms, on them).
  The game zooms the word under the cursor by 15% a notch (its box hit-tested, 0x14015b7e0).
  The game and its test program `GL_pc_test.exe` never call the switch API: they read the I/O
  board as winmm joystick 0 (`joyGetPosEx` buttons, `WAL_APM3_JOYSTICK=1`, `joystick.rs`: the
  IO4's two button words, test bit 9, service 6, home 28, start 7, up 5 down 4 left 3 right 2,
  push01..08 bits 1, 0, 15..10; segatools' `apm3hook/io4.c`). No ABAAS link: the network status
  functions' reads of the link-ready flags patched (`WAL_APM3_NO_LINK_PATCHES`, no "Aime /
  versus unavailable" banner).
* **Virtua Fighter 3tb Online** (`App\runtime\media\arashi.exe`, 64-bit Direct3D 11, game id
  SDJQ from the dump's `app.json`; the Model 3 game runs in `pre3\pre3-pxd-w64-Retail.dll`): no
  patch. Test mode = the same program (`gametest.bat`): `-wal-test`. The ABAAS game server holds
  the player (`misc_*`, `chara*` keys); a new card agrees to the play terms (default "no": left
  then button 1). Buttons push01-04 = Guard, Punch, Kick, Escape. National / in-store versus
  greyed (no link server): "one player" mode plays the CPU. The network icon (`0x141257470`, its
  state: 1 without ALL.Net, 2 = yellow "!" unless the Aime server, the cabinet flag (`+0xa8`) and
  the link (system object slot `0x2e0`, the link's `+0x1c99`) all answer, 3 = green): the
  profile's `WAL_APM3_NO_LINK_PATCHES` make only the icon's link test answer 1; patching slot
  `0x2e0` itself also enabled the versus menus (a national match waiting for a server). Found by
  a crash probe on the `AllnetAuth_isGood` thunk (`0x1412641a0`): its caller is the icon code.
* **Virtua Fighter eSports** (`eve.exe`, 64-bit Direct3D 11): RGG's Dragon Engine front end (menus,
  Aime screens, fights' presentation) around the VF5 Final Showdown core it loads from `vf5fs\`
  (`vf5fs-pxd-w64-Retail_APM3.dll` + `vf5fs_data.par`). eve.exe itself has no network code: the
  ABAAS game server client is in that core DLL (`WAL_APM3_GS_MODULE`), without it Aime play is
  greyed. A new card agrees to the play terms (default "no": left then button 1). The core draws
  its entry / Aime menus only through a host callback (frame context object `+0x1fe10`, size
  `0x20270`) that eve.exe fills; `vf5fs\vfes.exe`, the core's own loader, leaves it null, so run
  alone it shows the attract and the test menu but a black screen after START. Test mode = that
  loader (`gametest.bat`: `pushd ...\vf5fs`, `vfes.exe`): `-wal-test`, run from `vf5fs\`
  (`WAL_APM3_TEST_CWD`, the launcher's relaunch `exe=<file> cwd=<dir>`; from the game folder it
  boots eve's content instead), our `apm.dll` installed there too. National / in-store / password
  versus unavailable (no link server). The network icon: the core builds a status word for eve.exe
  each frame (`0x180219180`: 1 ALL.Net, 2 Aime server, 4 the link object's alive flag, 8 its
  connection); `WAL_APM3_NO_LINK_PATCHES` (`<module>!` patches of the core DLL) set the link bits:
  green icon, no "versus unavailable" banner, a MATCHING SELECT screen whose versus modes stay
  greyed ("play alone"). Found with `WAL_APM3_TRACE_CALLERS=1` (each new caller of a system
  function logged, `module+offset`): the callers of `AllnetAuth_isGood` / `Aime_isDBAlive`.
* **Guilty Gear Xrd REV 2 (APM3)** (`Binaries\Win32\GuiltyGearXrd.exe`, 32-bit Unreal Engine 3,
  Direct3D 9): the dump's `game.bat` arguments (`-FULLSCREEN -ResX=1920 -ResY=1080 -APM3 ...`);
  test mode = the same program with `-TESTMODE` (`gametest.bat`). Its ABAAS link client is built
  in: without a link server it retried and then ended the online mode ("communication error"), so
  `WAL_APM3_NO_LINK_PATCHES` do not start it (`0x1412736`) and make the boot's "checking the
  connection with the server" wait (3600 frames, `0x12fcfdc`) 1 frame; `WAL_APM3_ONLINE_PATCHES`
  make the network state (`0x140eac0`) and the link's "alive" (`0x1493820`) online. It resolves the
  title server with `gethostbyname` (WS2_32 ordinal 52, `sdcp.apm.sys-all.net`): `apms.rs` hooks it
  as `DnsQuery_A`. It asks 32 save slots (`StartGame`'s `[0, 32]`): with Under Night's 16 KB block
  its end of play compared the slots past it ("invalid vector<T> subscript", "user data
  send/receive failed (-1)", no `EndGame`): the block is now the requested slots x 0x400 bytes.
  `WAL_CRASH_REPORTS` (payload-common, default 8) raises the exception reports a run logs (this
  game's boot uses the 8).
* **Guilty Gear -Strive-** (`WindowsNoEditor\RED\Binaries\Win64\GGST-Win64-Shipping.exe`, 64-bit
  Unreal Engine 4, Direct3D 11; the PDB in the dump gives every symbol and struct): `apm.dll`
  next to the exe (`payload_install`), the dump's `game.bat` argument; test mode = the same
  program with `-testmode`. `-language=` takes the exe's names (`JPN`, `INT`, `FRA`...; anything
  else, the dump's `ja` included, is Japanese): `INT` shows the arcade menus' raw text keys
  (`*Title_PressButton_AC*`), only the EULA has English text, so Japanese stays. No ABAAS server:
  `isABaaSLinkAvailable_apm3` answers true (no "matching unavailable" notice; the link client
  itself must still start, else the game reports Aime unusable), the client's server name
  (`sdem.test-abs-link.sys-all.net`, a real host) is patched to loopback. LOCAL then plays alone
  against a still P2; no network versus. Selecting a character yourself matters: a character
  select that times out plays the fights by itself.
  - The player's data is not on SEGA's title server: Arc System Works' own `TLGameServerAPI`
    (`tl.rs`): WinINet POSTs of `data=<hex MessagePack>` to
    `https://arcade-ggst-game.guiltygear.com/api/<name>`, the URL patched to
    `http://127.0.0.1/api/` (`WAL_APM3_ONLINE_PATCHES`). WinINet is not the game's `connect`
    import: `apms.rs` hooks `InternetConnectW` to move port 80 to the title server, which routes
    a `data=` body to `tl.rs`. Request `[[user id, session, language, version, platform], param]`,
    response `[[session, result, date, appVer, dataVer, serverVer], param]`, positional; shorter
    arrays are accepted, so `[0]` answers news, rankings, items, VIP and results.
  - `user/login` (the Aime id; `WAL_APM3_PLAYER_NAME`, default `PLAYER`); `statistics/ac_get` /
    `ac_set`: JSON text per data type, character and matching value,
    `apms/tl-<user id>-<type>-<chara>-<value>.bin`; `tus/read` / `tus/write`: the compressed save
    (`0xd3b58` bytes unpacked), `apms/tl-<user id>-tus-<slot>.bin` (`apms` next to the exe, the
    program's folder). A known player without a save must get `tus/read` status 1: status 0
    without data is a fatal `BulkData compressed header read error. This package may be corrupt!`
    (`uei::archive::Decompress`, from `UpdateState_SaveDataWait`), which looks like a damaged pak
    and is not. The saves go at game over ("uploading data", about 2 s), the data is read back
    after the Aime prompt ("communicating with the server", about 15 s, one request at a time).
* **Shanghai IV Ryuumon Hiyaku** (Unity, 64-bit, Mono): as Kasiori (`X:/lib/apm.dll` by its path,
  `run_drives`; test mode `TestMode`; `-popupwindow`). Its `apmgamepad.dll` (the cabinet's gamepad
  library, DirectInput / XInput) is never called by the game. The Aime prompt's default is "play
  without linking" (left then button 1 for the card).
* **BlazBlue: Cross Tag Battle** (`Game.exe`, 32-bit Direct3D 9, Arc System Works' AALib; test
  mode `Game.exe -t`, the dump's `gametest.bat`). Its boot wrote `<app root>bblog<date>.dat` into
  a 40-byte stack buffer: the run directory's long path (`C:\wal\apm3\...`) overran it (a
  `__report_gsfailure` fast fail, exit `c0000409`, found by making its `int 0x29` an access
  violation for the crash reporter). The cabinet's root is `W:\` (`cd /d W:\` in `game.bat`):
  `WAL_APM3_APP_ROOT` (payload: `System_getAppRootPath`'s answer) and `run_drives: {w: .}` (`.`,
  the run directory itself). Its network state (`0x485d52`) is "unavailable" (banner
  `NESYS_INFO_NoUseOnlineBattle`) unless the built-in link (abaaslink ee20f2d) is set up and alive:
  the profile's `WAL_APM3_ONLINE_PATCHES` skip that test.
  - The player's data is on Arc System Works' own server (`bb.rs`), WinINet (`InternetConnectA`:
    `apms.rs` moves port 80 of 127.0.0.1 to the title server, which routes `/api/v1/`), the base
    `https://api.bbtag.blazblue.jp/api/v1` patched to `http://127.0.0.1/api/v1`. After a card:
    `GET /prod/150/config.json` (`base_url`), `/IsUserCreated` (`result`, a JSON bool),
    `/session` (`sid`, sent back as `X-SID`), `/GetUserId` (`user_id`), `/GetUserData`: the
    player's 0x314c-byte block read raw (magic `0xfc`, byte sum at +1, word sum at +2 of the bytes
    from +4; anything else is a new player: zeros, the last player's block still in the buffer);
    `/SetMyData` (octet-stream) stores it in `apms/bb-<card>.bin`; `/LogEvent` / `/EndPlay`:
    `{}`. The block's lock: `+0x32c` > 0 less than 900 s after the play time at `+0x324` is "this
    Aime is in use on another cabinet" (`0x5f3fd7`), set at the play's start and 0 at its end: a
    play ended by closing the game locked the card for 15 minutes, so `/GetUserData` answers the
    block unlocked (checksums updated).
  - Play: the card's player (left for YES, button A) then buys points with credits (100 points =
    1 credit; free play gives the credit; a card or not); a national "winner stays" room (50
    points) is a 300 s practice while waiting for a challenger: no network versus (no ABAAS
    server), the play ends when the points run out. Its opponent's state (START's SPARRING SETUP
    menu, `SPA_EnemyStatus`: 0 DUMMY, standing still, 1 Lv.1, 2 Lv.2; the player's `+0x680`) is
    set to DUMMY when a room starts (`0x5f6b94`, every room): the profile makes it 1, a CPU. English: only 144
    of the 664 arcade texts are in `localizeAC/eng_idlist.txt`, Japanese stays.

### 6.3l Sega ALLS with amdaemon (`crates/payload-alls`)

System `alls`: the ALLS games that run beside the cabinet's own `amdaemon.exe` (keychip,
ALL.Net, the "Type 4" USB I/O board, credits) instead of linking APM3 (§6.3k). First game:
Mario & Sonic at the Tokyo 2020 Olympic Games Arcade (SDFV, 64-bit, portrait). segatools
(public domain, `games/tokyohook` + `common/platform`, `common/board`) was the reference.

- Launch: the profile's `background` list starts `amdaemon.exe -c <config files>` (game.bat's
  list plus the profile's `config_wal.json`: video setup off, DHCP), `background_wait_ms`
  later the game; both through `wal-loader64` with `wal_alls.dll` (`payload_loader: true`).
  The game appends to amdaemon's payload log (`WAL_LOG_APPEND`).
- amdaemon (`daemon.rs`): `HKLM\SYSTEM\SEGA\SystemProperty` values written at start
  (`registry.rs`: keychip ids, region, subnet; mounts E: AMFS, Y: app data; `amVideo\name` =
  the payload, which exports `amDllVideo*`); `\??\FddDriver` keychip ioctls (play counters,
  trace log, billing CA/public key: segatools' dummy PKI in `pki.rs`); `\\.\sghwmonitor`,
  `sghwreset`; the IO4 HID board (`io4.rs`: SetupDi/HidD hooks, overlapped 64-byte input reports
  completed by a thread); `sysfile.rs` sets the DIP switches and free play in `E:\sysfile.dat`.
- Network (`netenv.rs`): one adapter at .205 of the keychip's subnet, router/DNS at .1, pings
  answered; DNS: `tenporouter.loc`, `bbrouter.loc` and the ALL.Net hosts resolve to the router,
  `mobirouter.loc` / `dslrouter.loc` do not (else the DNS check waits over a minute; a
  loopback address is refused as ALL.Net's). Connections to port 80 of the router go to the
  payload's server (`allnet.rs`, 127.0.0.1:`WAL_PORT`+102: PowerOn / DownloadOrder, DFI = zlib +
  base64, ARTEMiS formats). PowerOn is refused by default (offline); `WAL_ALLS_ALLNET=1`
  accepts it (no title server: the game's ABAAS GS `/api/...` is not emulated yet).
- Clock (`clock.rs`, both programs): Japan's time zone, clock setting refused quietly.
- Keychip billing type 1 (`nearfull` high word; type 0 gives error 8113 "MODE"). Offline the
  RTC is never synced with ALL.Net and billing stops the game with error 8114 "ALL.Net System
  error (RTC)": the profile patches amRtc's sync test (`WAL_ALLS_DAEMON_PATCHES`
  `0x28f100:B801000000C3`). Found with crash probes (`8B0C2500000000`) on the sites setting
  error 0x1fb2 / 0x1fb1.
- The game (`app.exe`): `-windowed` (its full screen draws the portrait picture for the real
  screen's size, off center), a 1080x1920 desktop fitted on the screen with the dump's
  `Bezel.png`; its window a borderless popup, not managed by the window manager (GNOME placed
  it below its top bar: a black band on top, the picture cut at the bottom). Wine's X11 driver
  read the `Managed` option before the payload set it (app.exe's DLLs start it before its entry
  point): `wal-loader` now sets it for the game's executable before starting it
  (`WAL_DESKTOP_UNMANAGED=1`); real keyboard/pads filtered; the LED board 837-15093-04 on COM1 (`led.rs`). The
  power-on test screens are laid out for 1920 wide: clipped on the left in portrait (the
  game's own layout).
- Controls (tokyo board): b1/left blue, b2/up/start yellow, b3/right red, b4/b5 the left/right
  foot (run: alternate), b6 jump (every jump beam, both feet off); test = test menu, service =
  its cursor.
- Debug: amdaemon's error numbers are in `am_resources/error.english.dll`; a landscape
  `WAL_DESKTOP_SIZE: 1920x1080` layer shows the power-on test screens whole.
- House of the Dead: Scarlet Dawn (SDET, UE 4.18, `WAL_ALLS_BOARD: hotd`): an older (2018)
  amdaemon. Its dump has no ICF: `icf.rs` writes `E:\ICF1`/`ICF2` (system + game entries;
  AES-128-CBC, key/IV built by amdaemon from three obfuscated tables at 0x140268d80; header
  CRC-32 of the bytes after it, then the XOR of the entries' CRC-32s), else the game id stays
  unset ("Game ID is invalid", "Disconnect dongle"). Platform `ACA1` (ACA0, model 0: the dongle
  is refused). `config_wal.json` sets only the video off (this amdaemon requires
  `network.property.save` with a network section).
  - I/O board: per player start (bit 7), the gun's front (4) and side (1) buttons.
  - Gun sensor board on COM1 (`gunsensor.rs`): one byte `08` per frame, answered with both
    guns' 10-bit x/y (y from the bottom), triggers and off-screen flags, the complement of the
    sum; `04` reset, `S P2\r\n` / `S Fn\r\n` settings answered `75 8A`. The LED controller
    (COM22) is absent. The aim's update (0x1401ded89) is hooked: identity calibrations for
    both guns (the factory's targets are inset from the edges) and the screen size: the menus'
    canvas (1920 wide) or the 3D view's in game (DemulShooter's method).
  - Graphics (`hotd.rs`, `unreal_ini.rs`): `[SystemSettings]` console variables in the game's
    `Saved\Config\WindowsNoEditor\Engine.ini` (render scale, sharpening, shadows, AA, AO,
    SSR, motion blur, DoF, bloom; `WAL_HOTD_EXCLUSIVE=1`: exclusive full screen at
    `WAL_RESOLUTION`, Wine's FSR). Radeon 860M, 1920x1080 in game: 13 fps with every effect,
    shadows off 26, everything off 43.
  - Gore (`gore.rs`, `WAL_HOTD_GORE`, default on): Michel Denizob's gore restoration (MIT,
    github.com/MichelDenizob/hotdsd_gorepatch) ported: the 5 edited assets (3 `bGoreFlag`
    flips, `BP_EnemyNormal`'s burned body bound, visible, hidden at death) rebuilt at the
    game's start from the untouched pak into `Content\Paks\Hodzero-WindowsNoEditor_Gore_P.pak`
    (stored entries, the base pak's mount point; `_P`: read before the base pak), removed when
    the option is off. The assets' SHA-1s match the original tool's; Unreal registers the pak
    at the first gameplay load.
  - `--headless` runs: the background amdaemon.exe is not inside gamescope (it opens the
    desktop's display); a Wine call crossing the two displays (ClipCursor's grab, a display
    mode change) ends the game with an X BadWindow error (one display outside headless runs).
    The launcher now starts the `background` programs inside the game's headless gamescope
    (a shell starts them, waits `background_wait_ms`, then runs the game).
- Sega World Drivers Championship (SDDS, UE 4.17, `WAL_ALLS_BOARD: swdc`, amdaemon Nov 2018):
  StartApp.bat's main cabinet (`config_LanServer.json`, `-launch=Cabinet`), no ICF in the dump
  (`icf.rs`). segatools' `swdchook`/`board/ffb.c` (GPL-3, read for the protocols only) helped.
  - amdaemon: `config_wal.json` sets the video off, the e-money terminal off and the Aime
    firmware's path from the game's folder (amdaemon runs from `Binaries\Win64`, not
    `App\Todoroki`: else error 6506 "firmware update failed"). The Aime reader (837-15396) on
    COM3 (`aime.rs`, `WAL_ALLS_AIME_PORT`): namcoes3's `sgreader.rs` moved to payload-common;
    player 1's `card` touches it for 3 s. Error 8114 (RTC): `amRtcIsServerSync` at 0x271a70
    patched (`mov eax,1; ret`).
  - I/O board: wheel adc 0, gas 1, brake 2; player 1 word: up 5, down 4, left 3, right 2,
    start 7, view change 1; player 2 word: the wheel's blue 15, green 14, red 13, yellow 12,
    paddles left 1 / right 0 (amdaemon's `config.json`).
  - The game's boards: COM1 the force feedback board (`ffb.rs`: 4-byte `cmd p1 p2 xor&0x7f`
    commands, never answered; `80` motor on/off, `84` constant force, `85` vibration, `86`
    damper, sent as the launcher's FFB outputs), COM2 the 15093 LED board (`led.rs`), COM20
    the dashboard (`dashboard.rs`, the `BP_DashBoard` Blueprint through Unreal's SerialPort
    plugin, first of COM20-29: `pr\r\n` answered `ps` + ten `0` flags, else error 3003
    "indicator LED board").
  - Main cabinet: the game reads `Y:\SDDS\LanServer.dat` too (the payload creates it; else it
    waits for a main cabinet). Its `MainDetector` broadcasts to 192.168.149.255:40257 and must
    hear itself, else the game reports 8404 "LAN install server not found" (the
    `BP_InitializeTestController` Blueprint's, not amdaemon's; a "reset in test mode" error
    amdaemon keeps): `udp.rs` sends the subnet's datagrams to lo (broadcast 127.255.255.255),
    received ones come from the adapter's address.
  - Offline: the attract shows the offline banner; guest play (Aime prompt: START, then left +
    START on "Yes"), team, round, race. Debug: amdaemon's ALL.Net library prints its log on
    stderr with its level globals set (`WAL_ALLS_DAEMON_PATCHES` `0x8a2480:05000000,
    0x8a2490:FFFFFFFF`).
  - Online (default, `WAL_ALLS_ALLNET=1`): ALL.Net accepted, its `uri` the game server's bare
    host name (`WAL_ALLS_TITLE_URI`: the game posts to `http://<uri>:80/api/...`); `aimedb.rs`
    answers amdaemon's AimeDB (TCP 22345, AES-128-ECB `Copyright(C)SEGA`, ARTEMiS formats; the
    Aime id 1000000000 + the access code's last 9 digits: a user id under 9 characters ends the
    play); `gs.rs` runs the game's ABAAS game server in its process (payload-common
    `abaas_gs.rs`, shared with apm3; title key from `BCryptGenerateSymmetricKey`; players in
    `Y:\gs\gs-<aime id>.json`, saves merged: the game's two end-of-play saves). The title data
    its servers held (`free_buckets`, `title_data` in `gs.rs`): JSON strings holding JSON,
    numbers as strings, lists plain (the game turns `[` `]` into Unreal text lists), learnt from
    the game's `UpdatePropertyFromJson` messages: `season_setting` (`NetSeasonSettingData`;
    without it the power-on test crashes), `season_recommend_setting` (`update_time`: the time
    sale period in minutes, a division by zero at the top menu without it), `event_setting1`
    (event base) / `2` (event dates) / `5` (point rules), the penalty limits. Profile patches:
    the main cabinet's LAN announcement every second instead of 30 s (0x43bebc), the matching
    server's connection test reported good (0x303190: `tdrserver.exe` is not in the dumps).
    Verified: power-on test all GOOD, card registration (name entry), a race, the save, the
    returning player's licence.
  - Automatic card (default, `WAL_ALLS_AUTO_CARD=1`, `aime.rs`): after START, sgreader puts the
    card on the reader at amdaemon's polls until it is read (amdaemon polls only while the game
    asks for a card), not again for 3 minutes; START on the attract goes straight to the
    licence.
  - English (`english.rs`, `WAL_SWDC_ENGLISH`, default on): the community "SWDC ENGLISH v2.1"
    release folder copied into the game's folder (any name, found by its `manifest.json`):
    applied at start like its installer (originals checked by SHA-256, moved to
    `Content\WAL-English-backup`, the 26 files copied: `Todoroki-English_P.pak`, voice banks,
    the movie), put back when off.
- Initial D THE ARCADE Season 3 (SDGT 2.30, UE 4.24, `WAL_ALLS_BOARD: idac`, amdaemon 2023):
  the dump's `App\DAC` exe, amdaemon and `IndRun.dll` are the decrypted ones (`*.original` and
  the top `DAC\` folder: still encrypted, `.text` 0x1000 bytes). segatools' `idachook` and
  `platform` (public domain, TeamTofuShop / Dniel97 forks) were the reference; a working
  install's game log (another loader, NVIDIA) was the comparison.
  - Black attract = the cabinets' nightly maintenance hours, Japan 02:00-07:00 (every test ran
    in them, the payload's clock being Japan's): the game holds its logo sequence for the reboot.
    `WAL_ALLS_TIMEWARP=1` (`clock.rs`, segatools' `timewarp`): each day's 24 hours shown as
    Japan's 07:00 to 23:00 (CHUNITHM PLUS closes before midnight: "本日のプレイ受付は終了しました"),
    the hours never reached; ALL.Net's and the game server's times from the same warped clock
    (`clock::unix_now`), else the game's auto reboot check fires at the attract.
  - Cabinet: DIP 0x07 (main, SWDC cabinet: Aime 837-15396 on COM3, single seat), the single
    seat's address 192.168.158.5 with its router .254 (`WAL_ALLS_HOST`, `WAL_ALLS_ROUTER`, the
    game's own iphlpapi too: `WAL_ALLS_GAME_ADAPTER`), platform ACA4 named "ALLS UX"
    (`WAL_ALLS_PLATFORM_NAME`). Error 3002 "Single Seat Setting Failed" (BP_BootCheck: a single
    seat not the LAN server): `UXeLanWrap::FIsCabinetSever` patched true (0x71bcc0). 8114 (RTC):
    `amRtcIsServerSync` 0x288350. amdaemon reaches ALL.Net through ws2's `connect` imported by
    name (`allnet.rs` hooks it by name too); system reboot (`ExitWindowsEx`) blocked, optical
    drives hidden (`QueryDosDeviceW`).
  - The game (`indrun.rs`, segatools' `indrun.c`, in the exe and `IndRun.dll` when loaded): no
    process list, computer `ACAE01A99999999`, user `AppUser`, Windows 10 1809, no mouse buttons,
    current directory `X`. Boards: COM1 the force feedback board (SWDC's `ffb.rs`), COM2 the
    837-15070 LED board (`led.rs`: ack on/off, board info `15070-04`, EEPROM in
    `Y:\idac\led15070.eeprom`; unanswered, the game polled it and GNOME showed "not
    responding"). I/O: SWDC's wheel and pedals, the 6-speed H shifter's gate on player 2's word
    (b6/b7 sequential, kp1-kp6 an H shifter's positions; on the keyboard the numpad's 1-6, keys 1
    and 5 kept for start and coin). Option data `C:\Mount\Option` (profile `c_folders`). No
    `-UserDir` on the command line: the game then posts to `/Get`. A borderless 16:9 window
    centered on the screen (`WAL_WINDOW_SIZE: picture`): on a 16:10 screen the interface's right
    side (the start prompt) was cut.
  - Game server (`idac.rs` + `idac/`: ARTEMiS Dniel97 `titles/idac` season 2/3 and
    Bayshore_sdgt_legacy reimplemented): ALL.Net's host field is the server URL
    (`WAL_ALLS_TITLE_HOST`, as ARTEMiS answers), JSON POSTs to `<host>initiald/<Category>/<Endpoint>`
    (requests without the URL's folders routed by their endpoint name); players in
    `Y:\idac\<aime id>.json`. ALL.Net accounting off (`config_wal.json`: a fresh cabinet's
    attract showed "It's being processed now", the `Reporting` message, and refused START: no
    billing server on 8443). Alive/Get's `force_reboot_time` a day ago (a later one reboots the
    game). Automatic card 3 s after start or the gas pedal (the game starts on either;
    `WAL_ALLS_AUTO_CARD_DELAY_MS`: a card read on the attract is dropped). Verified headless: new card (terms, registration, name, car), returning
    card ("play with this data"), Story race, result saved (story, mileage, cash), continue.
- Initial D THE ARCADE Season 5 (SDGT 3.10, season 3's update): season 3's profile, the same
  amdaemon (its RTC patch), the same game server. The dump's exe and `IndRun.dll` are decrypted
  memory dumps (Scylla's `.SCY` import section; `.data` and code clean).
  - Not needed any more: the `FIsCabinetSever` patch (the single seat is the LAN server by
    itself, "S" beside the network icon; error 3002 does not appear).
  - The first race crashed ("Fatal error!", an access violation in `IndRun.dll`, the race
    engine) and, past that, the player's car drove itself (steering ignored, the pedals barely
    felt; the input test was fine). The race settings the game hands IndRun carry 3 check values
    (+0x22c/+0x230/+0x234, copied to IndRun's +0x4bc/+0x4c0/+0x4c4): the constants
    0xf5fcfbfa / 0x79 / 0x59 when a flag of the race's manager is set (+0x81, read by
    0x37fa60), else values from a data object, unset here (-1, 0xff, 0x30). IndRun flags the
    player's car with wrong ones (driven by IndRun) and its race start state (2 -> 3) falls back
    each frame: the cars added again into a 4-entry list counted on (8), the next step read
    garbage pointers. Patch: both flag tests at 0x6e23af / 0x6e241d made true (mov al,1).
    Found by a caller-logging hook on IndRun's race start, then a DR0 watchpoint on the field
    (written by IndRun's settings copy at +0x65fe8), back to the game's writer.
  - `IndRun.dll`'s date limit: it compares the local date, packed as
    `(year-2022)<<16 | (month+4)<<8 | day+5`, to 0x4091f (2026-06-26, a year after its
    build). Past it, a check value of its race settings (+0x1014) is spoiled, the check at
    +0x7c8b0 answers 0x8c instead of 0 and every car's physics step adds a penalty term.
    Patch: its `ja` at `IndRun.dll+0x74c98` removed.
  - IndRun steps its race once per frame (`cUE4Interface::OnTick` ignores the frame time):
    the cabinet's 60 fps assumed. Below it the race runs in slow motion while the menus keep
    real time (season 3 too): a Radeon 860M laptop reached 33-36 fps in races, the same at
    1280x720.
  - IndRun's other cabinet checks (computer and user names starting with `A`, the current
    directory starting with `X`, no mouse buttons, the Windows build, crack marker files under
    `Y:\SDGT\`) are passed by `indrun.rs` or do not matter here.
  - Game server: three new endpoints; `User/CheckNgWords` (the name entry's word check: without
    `ng_flag` the name screen waits for its timeout) answered `ng_flag: 0`;
    `User/UpdateExpansionModeResult` and `Team/CustomizeTeamItem` answer the generic OK.
  - The attract reads a card too ("login preview": the player's data shown, then back to the
    attract): the automatic card after a START pressed before the attract is up is spent there
    (not again for 3 minutes). START on the card prompt chooses guest play.
  - Verified headless: new card (terms, registration, name, maker and car), returning card,
    story race driven by the inputs (gas, brake), result saved (UpdateStoryResult).
- Mario & Sonic at the Rio 2016 Olympic Games (SDCH, Nu 1.1 `AAV1`, 64-bit D3D11, portrait;
  amdaemon Dec 2015): a Nu game with amdaemon, profile in `systemprofiles/seganu/` with
  `system: alls`. The Nu's amdaemon is the ALLS one (keychip driver, registry values, ICF,
  ALL.Net) but reads the JVS I/O board through the AMEX board: `WAL_ALLS_JVS=1` starts
  payload-common's `amex.rs` (Sonic Dash Extreme's AMEX devices, §6.3m: SetupDi classes, JVS,
  DS, GPIO DIP switches, EEPROM / SRAM in `Y:\amex`) in amdaemon instead of `io4.rs`.
  - `config_wal.json`: video off, DHCP; this amdaemon needs `"save"` beside `"dhcp"` (else
    `No "save" key.` at start); LAN install off (cabinet 1's config makes it the store's
    install server). Error 8114 (RTC):
    `amRtcIsServerSync` at 0xed890 patched (the same function as SWDC's, found by its bytes).
  - ALL.Net accepted (`WAL_ALLS_ALLNET`); the Aime reader on COM1 as the cabinet's 837-15286
    (`WAL_AIME_READER=15286`: firmware 0x92, the dump's file; as a 837-15396 amdaemon tries a
    firmware update, then never polls it), `WAL_ALLS_AUTO_CARD` (option, default on).
  - GAME SERVER: Sega's CTS (`cts.rs`, `WAL_ALLS_CTS=1`): libcts 2.51 posts to
    `/apms-api/<request>` on ALL.Net's host; bodies: a 16-byte header (the IV), then
    MessagePack, zlib, AES-128-CBC with a fixed key (found by logging CryptImportKey). Answers
    need the message's CRC-32 in the header (else result -201 and the request retried: the
    ranking polled 20 times a second, StartGame "failed to get data"). StartGame returns the
    card's player (name, 2 save slots of 1 KB; a new card: no name, blank slots: the game asks
    for the name and resets the save); EndGame / ContinueGame / UpdateUser carry the name and
    the changed slot, kept in `Y:\cts\`. Rankings answered empty (EndGame's results not used).
    Found with a breakpoint probe on libcts' result conversion (0x49f090) and its own error
    log (its level checks patched, stderr printed through a `vfprintf_s` hook).
  - Open: CONNECT MAIN CABINET (the game's own check: cabinet 1 searched with UDP broadcasts on
    port 20199; without a cabinet id: error 3002 "Cabinet ID 1 Not Found") waits about 30 s,
    then BAD, the game then plays. The broadcasts are heard (`WAL_ALLS_UDP=1`, `udp.rs`); not
    changed by the game's `Config.csv` (`BOOT_CHECK_NETWORK`) or `NetConfig.csv`
    (`NET_USE_LAN`), nor by amdaemon's LAN install.
  - Language: the region (`WAL_ALLS_REGION` option): 1 Japan (default), 4 export: English text,
    but the export game has no Aime screens, no mode select, no game server (amdaemon's Aime
    can be forced on, unused). The English texts (`TxtMessage/data_us`) over the Japanese ones
    change little: the menus are images (`flash/data_jp`, `_us` variants only for the export
    screens).
  - The 15093 LED board on COM12 (`WAL_ALLS_LED_PORT`; else error 3001 "LED Board Not Found").
  - Controls (the game's input test): one player, two levers, each read by 8 photo sensors
    around it (`LLEV-PHO-1..8` clockwise from up: JVS right, left, up, down, buttons 1-4; the
    right lever on player 2's switches), a button on each lever (start), a foot sensor on each
    side (button 5) and six jump sensors (buttons 6-8 of both players). JVS layout `levers`
    (`jvs_board.rs`): the left lever from player 1's directions, the right lever from player
    2's directions or player 1's right stick. Run: the feet in turn (b3 / b4); start dash: both
    levers up; jump (b5): all six jump sensors.

- Initial D ARCADE STAGE Zero (SDDF 1.30, Nu 1.1 `AAV2`, 64-bit D3D11; `initial-d-arcade-stage-zero`,
  `WAL_ALLS_BOARD=idz`): segatools' idzhook (public domain) reimplemented. amdaemon reads the JVS
  I/O board on the AMEX board (`WAL_ALLS_JVS`): wheel / gas / brake on analog channels 0-2, the
  6-speed H shifter's gate on player 2's directions (new JVS layout `h-shifter`, payload-common's
  `hshifter.rs`, shared with IDAC's io4: b7 / b6 shift up / down, kp1-kp6 the positions); the
  game: FFB board on COM1 (SWDC's), 837-15070 LED board on COM11, Aime 837-15286 on COM10
  (amdaemon), `_stat64i32` passed for the unpacked programs (`stat.rs`, idzhook's `crt.c`).
  - amRtcIsServerSync at 0x2479a0 (found by its "amRtcIsServerSync" error string): error 8114.
  - `initialD.ini` says 2560x1440: read as an edited copy in `Y:\` (`inifile.rs`,
    `WAL_ALLS_INI_FILE`: 1920x1080 full screen), else the picture is cut.
  - The store server: cabinet 1 (DIP 1) starts `ServerBoxD8_Nu_x64.exe` itself (CreateProcessA);
    without it error 8306 ("server machine") forever. `child.rs` (`WAL_ALLS_CHILDREN`) starts it
    through wal-loader with the payload: its full screen status window hidden (it covered the
    game: "PC MODE DUMMY" is printed unconditionally, not a state), the subnet's datagrams
    (`udp::init_with_echo`: the game finds it by a broadcast to UDP 51457), TCP to the cabinet's
    own address (.205:51783) sent to the loopback (`allnet.rs`), its accepted peers shown as the
    cabinet's address.
  - Game server (`idz.rs` + `idz/{msg,store}.rs`, by a subagent): ARTEMiS's `titles/idz`
    (handshake, AES, codes per version; WTFPL) completed with the dump's minime v005 layouts for
    1.30 (create / load / save profile, cars, stocker, settings, teams, time attack, top ten):
    TCP router:10000 (ALL.Net host `{router}:10000`, `WAL_ALLS_TITLE_HOST`), RSA-encrypted
    session key then AES-128-ECB, 0x30-byte header + fixed-size message per code; players in
    `Y:\idz\<aime>.json`. UDP echo ports 10001/10003/10009/10010/11 (minime's ping; 10009 the
    game's too), news text on router:80 (`gs::init_plain`).
  - Aime only after ALL.Net accounting checked in: with accounting off (IDAC's config) the attract
    says "national battle mode, Aime unavailable" and a registered card's game stops at the race
    loading with error 8304 (communication failed, GAME). `WAL_ALLS_BILLING=1` serves
    `billing.rs` in amdaemon (TLS 1.2, suite 002f).
  - Names: `WAL_ANSI_CODEPAGE=932` (the name entry's kana were saved as "?").
  - English on the Japanese keychip (`WAL_ALLS_IDZ_ENGLISH=1`, option Language, `idzenglish.rs`):
    the export region (4) has English texts but no Aime / game server. The game's opens under
    `data\` are redirected: `TxtMessage\data_jp\txbn` to the export texts, the menus' picture
    pairs `X.tcd` / `Xtex.pac` to `X_ex.tcd` / `X_extex.pac`, and the community "D0 English patch
    0.85" (PockyWitch, Alexvgz...: made for the export version) in the dump's `english\` folder
    over them. The TXBN text files (`idzenglish/txbn.rs`: entries of 23 u32, line table, UTF-16
    pool; all 105 files round-trip) are rewritten at start into `Y:\idz_english\txbn` with the
    2100 texts the export / patch left in Japanese translated (`idzenglish/strings_[a-d].rs`,
    1824 strings: avatar parts, frames, stickers, titles, result lines, tips...). The font of the
    dynamic texts (the cars' tuning parts, team names) has no half width Latin: the tuning files
    are converted to full width (`Y:\idz_english\info_csv`, Shift-JIS and UTF-8), the server's
    auto team names sent as full width "Speed Stars 001" (`idz/msg.rs`). User: OK (2026-11-10).
    For Zero Ver.2: the tables apply to every identical Japanese text; the start log counts the
    texts left ("idz english: N text files, X translated, Y still in Japanese"), add their
    translations as a new `strings_e.rs` in `strings.rs`'s TABLES.
  - Its full screen mode exits (status 1) on a 1920x1200 desktop: windowed 1920x1080 in a
    borderless 16:9 window (`WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE=picture`).
  - User: 100% (2026-11-10). Verified headless: steering adjust, attract, guest story race to the result, new card
    (registration, name, auto team, car, createProfile / saveProfile3, story race, results,
    time attack saved), returning card (loadProfile3 and the others), test menu NETWORK TEST all
    GOOD. Open: in-store battle (single cabinet), events / gacha / rankings empty (as ARTEMiS).
- Initial D ARCADE STAGE Zero Ver.2 (SDDF 2.30, `initial-d-arcade-stage-zero-ver-2`): version
  1.30's profile and board (segatools' idzhook covers both versions: same devices, AAV2; Ver.2
  takes Aime MIFARE cards only), its dump with `package\` at the root, amRtcIsServerSync at
  0x265630 (its amdaemon, found by the error string). Game server for protocols 2.10 / 2.30
  (`idz/msg/v2.rs`, profiles in `Y:\idz2`): minime v016's 2.10 layouts (github.com/djhackersdev/
  minime) with ARTEMiS's 2.30 codes / sizes, plus what 2.30 changed, found on the game:
  - loadConfigA must answer server version 230 (minime's 210: no loadConfigB, offline, Aime
    refused with error 6503); loadGeneralReward answered generic (minime's 0x009d answer: the
    login retried 3 times then "[accessStep] FAILED" in the game's IniD0_AppError.log).
  - The profile = 2.10's with 6 more story chapters (rows of 0x7a): save request 0x1190 (rows
    from 0x548, 2.10's fields 0x2d8 further: the car found there), load answer 0x1640 (rows at
    0x534, 2.10's fields 0x2dc further: the Aime id read at 0x9b8, the team id at 0xf3c, found by
    a probe answer (`WAL_ALLS_IDZ_PROBE=1`: each empty slot holds its own offset); the car right
    after the car count (0x1568, the game's answer parser at 0x1402d0b61; a car 4 bytes off read
    as code 0xffff and crashed). `WAL_ALLS_IDZ_RAW=1`: the requests' bytes in the log and
    `Y:\idz_raw_<code>.bin`.
  - Framing: 2.30's unknown request lengths taken whole after 50 ms of quiet; the request sizes
    are logged. Ver.2's ServerBox opens userdb connections the server cannot decrypt ("bad
    magic", answered 0x00 as ARTEMiS does): the game plays without it.
  - The start button held off in amdaemon's JVS board until the game has its configuration
    (`idz::hold_start`, the event `WAL_IDZ_SERVER_READY` set at loadConfigB): it asks for it in
    steps 30 s apart from its attract (about 90 s), pauses them once a player is in its entry,
    and a card read before fails (error 6503, "[accessStep] FAILED").
  - English (`WAL_ALLS_IDZ_ENGLISH`, `WAL_ALLS_IDZ_ENGLISH_DIR=..\english`; no patch for Ver.2):
    its export texts and pictures, the 6564 texts left in Japanese translated (1.30's tables for
    3072, `strings_e-i.rs` for the 3234 others: titles, stamps, missions, story dialogue,
    advice), the cars' tuning texts translated (`strings_j.rs`) and in full width (the files
    UTF-8 or Shift-JIS whatever their name, via code page 932). The pictures the export left in
    Japanese (headings, banners) stay so.
  - User: 100% (2026-11-10), English included. Verified headless: attract, guest story race, new
    Aime player (registration, name, auto team, car, createProfile / saveProfile, Mako & Sayuki
    tutorial race, result), returning player (licence card with name / team / car, story
    race).
- CHUNITHM STAR (SDBT 1.20.00, Nu 1.1 `AAV1`, 32-bit Direct3D 9; `wal_alls32.dll`): amLib and
  the ALL.Net library are linked in `chuniApp.exe`, no amdaemon. `WAL_ALLS_BOARD=chuni`
  (`lib.rs` `chuni_process`): the game's devices first (`chuni.rs`), then the system's
  (`daemon.rs`: registry, keychip, AMEX board with `WAL_ALLS_JVS=1`, network, ALL.Net, AimeDB) in
  the game's own process. segatools' `chunihook` (public domain) was the reference.
  - Ground slider on COM1 (837-15330, `chuni.rs`): frames `FF cmd len data sum` (escape `FD`),
    reset `10`, board info `F0`, auto scan `03` / `04` (32 pressures streamed through the serial
    layer's idle callback, every 2 ms at most, numbered from the right: key 16's two sensors
    first), lights `02` not answered. Air sensors: player 1 and 2's JVS switches 5-7 (beam 0
    player 1's btn5, 1 player 2's btn5, 2 player 1's btn6...), on while clear
    (`jvs_board::set_switches_hook`); a raised hand breaks them from the lowest up, one per read.
  - Controls (player 1): left stick = keys 1-8 by its direction (left, up, right), right stick =
    keys 9-16; d-pad left/down/up/right and b1/b5/b2/b6 (X, A, Y, B) = keys two by two from the
    left; b3/b4/b7/b8 (shoulders) = air. Keyboard (the profiles' `input.keyboard`, segatools'
    layout): S D F G H J K L = keys two by two from the left, Space = air (players 2 and 3's
    default keys freed). Touchpad / touchscreen (`input.touch: true`, the launcher's `touch.rs`:
    the kernel's multi-touch slots, the device grabbed, `Message::Touch` with up to 10 points
    0..65535): each finger the key under it (16 across), a finger in the top third raises the
    hand.
  - JVS (837-14572 as segatools' io3): 24 outputs, 8 analog channels, 4 rotary channels
    (`WAL_TYPEX_JVS_ROTARY`), else error 6402. The game sends `14 11 12 13` in one packet: the
    features' end code is one byte when commands follow (`jvs_board.rs`).
  - LED boards 837-15093-06 on COM10 / COM11 (`led::init_pair`, `WAL_ALLS_LED_BOARD` =
    `15093-06,6710,adf7`: number, chip, firmware sum).
  - Aime reader (837-15286) on COM12 in `aimeReaderHost.exe` (`background`, its options through
    `WAL_EXE_ENV`): it reaches AimeDB itself (`aime.naominet.jp`: `netenv::init_game`,
    `allnet::init_redirect`, the game's AimeDB server). A new card: the game asks to register it
    (yes = left), then the new player's trial / name entry.
  - Backup (AMEX EEPROM `Y:\amex`): the settings record (0x10F0 / copy 0x1558, `TBDS`, CRC-32)
    made the store's distribution server (+0x80 = 0) without cabinet group (+0x81 = 0): the
    default client waits for a server cabinet on the LAN ("配信サーバーを探しています"). Set at
    start and in the game's writes (`amex::set_eeprom_write_filter`); a blank backup gets the
    game's id record (0x1F0 / 0x9F0) and default settings first. Free play: amLib's credit record
    (0x40 / 0x840, +0x0A; `WAL_ALLS_FREEPLAY`, else `WAL_FREEPLAY`, default on).
  - Network: `netenv.rs`'s structures in their 32-bit layouts (`IP_ADAPTER_ADDRESSES`,
    `IP_ADAPTER_INFO`, `DNS_RECORD`) and `GetIfTable`, else "IP ADDRESS CHECKING" forever.
    The title server (`chuni/server.rs`, ARTEMiS's `titles/chuni` base / STAR handlers, WTFPL):
    `WAL_ALLS_TITLE_URI=http://{router}/SDBT/120/`, zlib JSON to `ChuniServlet/<Api>` answered in
    ALL.Net's server (`allnet::set_title`; the router's port 80 and WinHTTP to the router), the
    players in `Y:\chuni\<Aime id>.json` (the game's records merged by their ids), events and
    charge items listed from the data (`event*/Event.xml`, `chargeItem*/ChargeItem.xml` of
    `data\A000` and `C:\mount\Option\*`).
  - Billing (`billing.rs`): the game's OpenSSL 1.0.2h checks in at `ib.naominet.jp:8443` (the
    router's port 8443 to 127.0.0.1:`WAL_PORT`+105) offering RSA key exchange only: a TLS 1.2
    server of `TLS_RSA_WITH_AES_128/256_CBC_SHA` (Montgomery RSA, AES-CBC, HMAC-SHA1, PRF SHA-256;
    TLS 1.0 / 1.1 for older clients: CHUNITHM PLUS's OpenSSL offers 1.0 only, PRF MD5 / SHA-1,
    1.0's chained CBC IVs)
    with segatools' dummy PKI (`server.pem` for `ib.naominet.jp`, signed by the CA the keychip
    gives); answers as ARTEMiS's billing servlet (play limit and near-full signed with
    `billing.key`, RSA PKCS#1 v1.5 SHA-1). `OPENSSL_ia32cap=~0x20000000` (segatools'
    `openssl.c`). Error 8114 "ALL.Net System error (RTC)" was amRtc's `LookupPrivilegeValue`
    (no `SeTimeZonePrivilege` in Wine, 1313): `clock.rs` grants the time privileges; the time
    warp also hooks the C runtime's clock (msvcr110: the RTC check compares both).
  - CHUNITHM STAR PLUS (SDBT 1.25.00, `chunithm-star-plus`): the same profile with the title
    uri `/SDBT/125/` (the server's data version from the uri's version: `1.25.00`).
  - CHUNITHM PLUS (SDBT 1.06.00, `chunithm-plus`): the same profile with the title uri
    `/SDBT/106/`; billing over TLS 1.0, the time warp's day ending at Japan's 23:00.
  - CHUNITHM PARADISE LOST (SDBT 1.50.00, `chunithm-paradise-lost`): runs with SEGAguide's
    unpacked `chuniApp.exe` and `aimeReaderHost.exe` (Dniel97/SEGAguide `SDBT/150.zip`, cloned
    at `~/code/SEGAguide`; its `amfs/ICF1`, `ICF2` too) in place of the dump's protected ones
    (kept as `.orig`). SEGAguide's executable has its title encryption already off; the profile
    clears WinHTTP's secure flag (`WAL_PATCHES` 0xD04D09, SEGAguide's "No TLS": else its title
    requests are HTTPS). Its options re-ship the earlier ones' events (3065 `Event.xml`, 325
    events): the title server lists each id once (every copy listed, the game failed to parse
    the answer, "expected object or array", and stayed on 筐体情報 アップロード中). The game logs
    "No such node (apiName)" after the client upserts (harmless). Headless-tested: boot checks,
    title server, card login (registration prompt), tutorial with the slider.
    The dump's protected executables, for the record: a stub at +0x196c, sections decrypted at
    start into `.tls`, VMware / VirtualPC probes, trap flag checks; under Wine the protector
    reports to `\\.\mailslot\ErrLog-Pec7679` (date, code, path, an encrypted block) and calls
    `ExitProcess(0)` (`chuniApp.exe` `023-01F-000`, `aimeReaderHost.exe` `042-023-004`). Its
    checks, found by hooking its status callback (+0x22257c0: `status = code << 4 | kind`, kind 5
    a failure, the caller's frames logged, the image dumped at the failure):
    - 0x01F, the parent: a parent holding the mutex `HtpecIt.dll::<name>` passes (the name from
      the parent's id inverted, xored with the low half of its creation time, 0x12345678 when 0:
      8 base 13 digits from the lowest, each a hex digit from a 4 x 13 table, row = digits left
      & 3), else the parent's image must be in `<windows>\system32\` (the cabinet's `game.bat`,
      `cmd.exe`), compared with `QueryDosDeviceW`'s device path. Wine's
      `NtQueryInformationProcess(ProcessImageFileName)` answers `\??\C:\...` where Windows
      answers `\Device\HarddiskVolume1\...`: a `cmd.exe` parent fails too. A loader holding
      the mutex passed (tested).
    - 0x017, the boot options: `HKLM\SYSTEM\CurrentControlSet\Control` `SystemStartOptions`
      must be read (none in Wine) and hold no `TESTSIGNING` / `DISABLE_INTEGRITY_CHECKS`; set, it
      passed (tested).
    - 0x019, its kernel driver: `\\.\Htsysm7679` (`Global\HtsysmXX.sys::`) opened, then queried
      with `DeviceIoControl` 0xaa012040 / 0xaa0120c0 / 0xaa0120c4 (hardware values), the
      protector never installing it itself: the blocker (a hardware lock; emulating it would
      need the driver's answers on a real Nu). The community runs patched / unpacked
      `chuniApp.exe` builds.
    Those experiments were reverted (none of them is in the code).
  - Debug: `WAL_CHUNI_DEBUG=1` (STAR 1.20 only: the ALL.Net library's log, its level, categories
    and callback globals set; EEPROM writes; the TLS client hello), `WAL_ALLS_AMLOG=0x7e53d0` with
    `WAL_ALLS_AMLOG_LEVELS` (amLib's log, payload-common's `amlog`), `WAL_ALLS_AIMEDB_DEBUG=1`.
  - Graphics: DXVK's SM3 compiler (dxbc-spirv) asserts on one of the game's pixel shaders (no
    `oC0`: `sm3_io_map.cpp` `getColorValue`), the game aborts at boot: the profile uses
    wined3d (`dxvk: false`).

- CHUNITHM SUPERSTAR (SDGS 1.00.00, international, ALLS `ACA1`; `chunithm-superstar`): WORK IN
  PROGRESS, not running. amdaemon (64-bit, `App\amdaemon`, game.bat's `pushd amdaemon`) runs the
  IO4 board (`WAL_ALLS_BOARD=chusan`: the air sensors on players 1 / 2 bits 13, 12, 11, `chuni::hand`)
  and the Aime reader on COM4; the game (32-bit) the slider on COM1 and the LED boards on COM2 /
  COM3 (DIP switches 07: delivery server, 60 FPS, CVT; segatools' chusanhook); its title
  connections (sockets, WinHTTP to the router) go to amdaemon's ALL.Net server, where the title
  and billing servers run (`allnet::init_title_redirect`, `chuni::init_daemon`). The launcher
  starts a background program of the other architecture with that architecture's loader and
  payload (`wal-loader64.exe`, `wal_payload64.dll`) from its own folder.
  - Blocker: both executables are protected. `chuniApp.exe`: a stub (entry 0x40196c ->
    0x401000: `pop eax; pushal; call 0x401030; popal; test eax,eax; ... push eax; ret`) maps the
    file and decrypts a second stage into `.tls` (RVA 0x200be00, obfuscated protector code),
    which unpacks the program later; its anti-VM check (`0F 3F 07 0B`, Virtual PC) is caught by
    its own SEH frame. `amdaemon.exe` (64-bit): its sections encrypted in the file, the protector
    in `.pdata`'s range (RVA 0xa5xxxx), single-step (trap flag) checks with a handler at
    0x140a5fdd0, then direct system calls (its own copies of the Windows stubs: `test byte
    [7FFE0308h],1; jne; syscall; ret`): under Wine they return garbage, execution lands in the
    next stub (illegal instruction at 0x140a5d488), Wine's unwinder reads -1 (ntdll+0x10efb):
    amdaemon dies at once, also without the payload. The exception directory in memory is empty
    then (the protector's temporary copy freed). Needs either Wine handling of foreign direct
    syscalls (syscall user dispatch, Windows call numbers) or patching every stub; segatools users
    run unpacked executables (none in SEGAguide's packages for SDGS).
  - Tools made for it: payload-common's `packed.rs` (`WAL_PACKED_JUMP=0x100d`: the stub's last
    jump caught, the payload started after it; `WAL_PACKED_DUMP`: the image written then),
    `crash.rs` (`WAL_CRASH_DUMP`: the image at the first illegal instruction or access violation,
    the faulting code bytes logged), `WAL_PACKED_IMPORTS` (existing: hooks as hot patches).
  - CHUNITHM SUPERSTAR PLUS (SDGS 1.05.00): the same protected `chuniApp.exe`; its dump has no
    `App\amdaemon` folder.

### 6.3m Sega Nu (`crates/payload-seganu`)

System `seganu`: Nu / Nu SX games linking Sega's amLib, whose "amy" layer talks to the AMEX
board's drivers instead of RingEdge's `mx*` devices (segatools' `common/amex` and
`common/platform`, public domain, were the reference; Project DIVA Arcade is its Nu example).
First game: Sonic Dash Extreme (SDBX, Nu SX `AAW0`, 2015, 32-bit Direct3D 9, portrait). The
payload is loaded by `wal-loader` (no driver DLL imported).

- Devices (`devices.rs`, IAT hooks: `CreateFileA/W`, `DeviceIoControl`, `CloseHandle`,
  `SetupDiGetClassDevsW` / `EnumDeviceInterfaces` / `GetDeviceInterfaceDetailW`): one device
  per AMEX interface class, its path ours. DS (`279a9f67`: the 32-byte board record, CRC-32,
  region, serial), EEPROM (`b7970f0c`, 8 KB) and SRAM (`741b5fca`, 512 KB) as files of the save
  folder (`sv/EEPROM.bin`, `sv/SRAM.bin`, the game's own overlapped reads and writes; version
  and `IOCTL_DISK_GET_DRIVE_GEOMETRY` answered), GPIO (`e9a26688`: DIP switches
  `WAL_SEGANU_DIPSW`, push switches test / service, LEDs), JVS (`db6bbb45`: payload-common's
  board on port `amex_jvs`, `CTL_CODE(0x8000, 0x802)` one packet each, `0x803` the sense line
  3 / 2, the bus reset completed with `STATUS_NO_DATA_DETECTED`). `\??\FddDriver`: the keychip
  (play counters, trace log, as alls' amdaemon). `\\.\sghwmonitor`: the CPU temperature.
- `HKLM\SYSTEM\SEGA\SystemProperty` written at start: keychip (`WAL_SEGANU_GAME_ID`,
  `WAL_SEGANU_PLATFORM_ID`, `WAL_SEGANU_REGION`, the machine's /24 as its network), `Static`
  (platform id, CPU temperature limits), `Master`, `mount` (Y: application data).
- Logs: `WAL_SEGANU_AMLOG` / `_LEVELS` (amLib's log function and module levels, as RingEdge's),
  `WAL_SEGANU_GAMELOG` (the game's printf-like log: Sonic Dash Extreme `0x2e2b20`, its
  managers' init only). Sonic Dash Extreme's amLib log: `0x1ddb60`, levels `0x76fa78,
  0x76fd08,0x770120,0x782ef0,0x7830ec,0x783128,0x783134,0x783bac,0x783bb0,0x788ac4,0x788ac8,
  0x78aac8,0x78af0c`.
- Sonic Dash Extreme: runs from the dump root (`WAL_SEGANU_CWD=..`, `game.bat`), Y: a dump
  folder. Its DirectInput blocked at boot on a synchronous read of Wine's virtual HID mouse:
  `WAL_DINPUT_DISABLE`. JVS: 24 general purpose outputs (none: error 6402 "JVS I/O board does
  not fulfill the game spec"); output 8 (byte 1, 0x01) is the ticket dispenser's motor, its
  notch sensor player 1's btn4 (byte 2, 0x40), its empty sensor btn2 (byte 1, 0x01; found in
  `TestMode_x86.exe`'s dispenser test: the switch words byte swapped per player, current at
  +0x964, rising edges at +0x968): `WAL_TYPEX_JVS_TICKETS=8,btn4` (payload-common's board:
  notch pulses at 10 tickets/s while the motor runs). Without pulses: TICKET JAM, kept in the
  backup (the next boots resume the payout and stop on it) until the RELEASE button. COM2 is
  the cabinet's lighting controller (a command per game state): absent. Display: its
  1080x1920 windowed device in a 9:16 borderless window (`WAL_WINDOW_SIZE=aspect`) with a
  1080x1920 back buffer (`WAL_D3D9_WINDOWED_SIZE`: sized from the window its 2D screens were
  cut) and `WAL_D3D9_KEEP_WINDOW` (the window, not shown yet at the device's creation, stays
  where it is instead of over the whole screen), in the dump's `bezel.png`
  (`WAL_BEZEL_FIT=1`, a 3:4 opening).
- Test menu: on TEST the game creates `C:\Users\AppUser\gametest` and exits (`game.bat` then
  runs `TestMode_x86.exe` and deletes the file). The marker goes to the save folder and its
  creation asks the launcher to relaunch with `exe=TestMode_x86.exe` (`WAL_SEGANU_TEST_EXE`);
  in the test menu the payload removes it and asks for the game's relaunch, so EXIT starts the
  game again (the cabinet's `game.bat` loop). The test menu accepts only a board with 4 analog
  channels at least ("JVSNode[0] does not satisfies function", then NO JVS NODE in its input
  test): `WAL_TYPEX_JVS_ANALOG_INPUTS=,,,` (declared, fixed). Its input test names the
  switches: the DASH button is START, btn1 the ticket RELEASE button (profile: `b1` start,
  `b2` release). Its amLib log: `0x2d760`, levels `0x770cc 0x770d0 0x77700 0x77708 0x8ad60
  0x8ad70 0x8ad78` (through `WAL_EXE_ENV`: the level list takes spaces there).
- Free play (`WAL_SEGANU_FREEPLAY`, else the common `WAL_FREEPLAY`; `seganu` is in the GUI's
  free play systems): amLib's credit record (backup record 1: EEPROM 0x40, copy 0x840, 0x50
  bytes, CRC-32 of the bytes after it) has the free play flag at +0x0A (the ALLS credit
  block's place; +0x08 = 1 gives error 1000 "Wrong Coin Setting"), set at each start. A blank
  backup makes the game copy the record from its default game data, used for that session:
  `WAL_SEGANU_FREEPLAY_DEFAULT=0x5a51da` sets the flag there too (amLib's own default,
  `0x5d2d10`, is not the one it uses). Verified: FREE PLAY from a blank backup, START alone
  starts a game; off: coins and credits again.

- Luigi's Mansion Arcade (SDCF, Nu `AAV0`, Capcom 2015, 64-bit Direct3D 11, two vacuum guns):
  the payload's 64-bit build (`wal_seganu64.dll`, started by `wal-loader64`: profile
  `payload_loader`), the game run from `exe\x64` (its data `..\..\DataGlobal`). The executable
  is packed by Capcom's protector (`.capcom0` / `.capcom1`): it decrypts the game's sections and
  fills its imports, then checks its environment and ends the process under Wine (exit code
  `0xFD605820`, also without the payload; the process list, the kernel modules and the parent's
  name made like Windows' did not pass it). Its first `NtQuerySystemInformation(
  SystemProcessInformation)` comes after the decryption: `unpack.rs` (`WAL_SEGANU_UNPACK=
  0x30fb70`) hooks ntdll's system call stub and, at that call, writes `WAL_SEGANU_UNPACK_PATCHES`
  (the five functions the protector replaced by jumps into its virtual machine, the calls to the
  first one, the data path string `..\..\DataGlobal\EN\Data\` it decrypts after its checks;
  found by comparing the decrypted memory with an unpacked executable) and calls the entry point
  on the protector's stack. Its exception tables stay encrypted (an exception would end the
  game). I/O board on COM1 (`lmaio.rs`, `WAL_SEGANU_LMAIO`, 115200 bauds, the firmware
  `LMAIO.bin` `game.bat` flashes): ASCII packets every 28 ms, the game's `/s CO: SW: SO: MO: ST:
  /e` (`CO:T,<n>`: the coins taken, repeated until the board reports none), the board's lines up
  to an ETX: `G1:<a1>,<b1>,<a2>,<b2>` the guns (a: x reading, 0x1000 start, 0x2000 action,
  0x4000 trigger, 0x8000 off the screen; b: y reading from the bottom, 0x8000 valid), `CO:` the
  coins, `SW:` three switch words (first: 0x4 test, 0x2 service), `ST:` the vacuum guns' board
  status (not 0: error 3002 "communication error (Poltergust board)"). The game maps the readings
  with the guns' calibration in its board object (`[[exe + 0x4f6c98]]` +0x5c..+0x68, x = (a -
  256 - min) / (max - min) * 1920, y = (1535 - b - 256 - min) / (max - min) * 1080), set to
  0..1024 at each answer (`WAL_SEGANU_LMAIO_OBJECT`): the readings span the picture. Start is the
  gun word's 0x1000 (a rising edge, the join check), the trigger also presses it. COM10 / COM2
  (its LED board, 45 RGB lamps, written only) is absent. Free play: the game writes amLib's credit
  record from its own coin settings (EEPROM record 0x1200, free play at +0x18):
  `WAL_SEGANU_FREEPLAY_BYTE=0x1218`, and in a blank backup the records of a set up one first
  (`WAL_SEGANU_FREEPLAY_RECORD`: its id record at 0x1f0 / 0x9f0, its default coin settings; else
  its first boot sets the backup up, free play off).

### 6.4 Batocera integration

A configgen generator (`windows-arcade-loader`, kept in the rgs Batocera tree, not in this
repository) runs `arcade-launcher run <rom> --root <emulator dir> --profile <es options>`:

* Emulator directory `/userdata/system/rgs/emulators/windows-arcade-loader`:
  `arcade-launcher`, `payloads/`, `systemprofiles/`, `launcher.yaml` (machine layer:
  `runners_dir: /userdata/system/wine/custom`, `runner: GE-Proton11-10-x86_64`,
  `payloads_dir: payloads`; template in `tools/batocera/launcher.yaml`). The Wine prefixes are
  created there. `tools/batocera/install.sh [DEST]` copies a build (and the profiles) into it.
* The GE-Proton runner (with the runner hotfixes) is copied to
  `/userdata/system/wine/custom/GE-Proton11-10-x86_64`: Batocera's own Wine builds lack them.
* Roms: the dump directory's `<gameid>.windowsloader` file (ES extension `.windowsloader`), or
  the dump as a `.squashfs` image: configgen mounts it with a writable overlay
  (`writesToRom`, kept in `/userdata/saves/<system>/<image>`) and hands the mounted directory.
* ES options (`es_features_windowsarcadeloader.cfg`): rotation (`reshade`), renderer (`dxvk`),
  mouse as gun (`input.guns_mouse`), keyboard (`input.keyboard_enabled`), keep aspect ratio
  (`env.WAL_KEEP_ASPECT_RATIO`), crosshairs (`env.WAL_CROSSHAIR`, the common crosshair option),
  free play (`env.WAL_FREEPLAY`, §6.6).
  Only the options set in
  ES are written to the `--profile` layer (merged last), so a game's own value stays otherwise.
* Hotkey exit runs `pkill -TERM -x arcade-launcher` (the launcher stops the game on SIGTERM).

### 6.5 Common crosshair option (`WAL_CROSSHAIR`)

One option for every game able to show the players' crosshairs: `env.WAL_CROSSHAIR` (`1`/`0`).
A game profile setting it declares the game able to, with its default; the payloads read it
through `wal_payload_common::crosshairs(default)`. The GUI's Options tab gets a "Common" group
with a Crosshairs checkbox for these games (`Profile::add_common_options`, unless the profile
declares the key itself, e.g. as "Touch pointer"); Batocera's CROSSHAIRS (ES) writes it in the
`--profile` layer. Games: Point Blank X (the game's own crosshairs, default on), Gashaaaan,
Gashaaaan Refill! (konami `crosshair.rs` overlay, on), Elevator Action Death Parade (the dump's
`P1.png`/`P2.png`, off), and the touch pointers of Winning Eleven 2012, Yu-Gi-Oh! DT6, Steel
Chronicle (on). A new game: set `WAL_CROSSHAIR` in its profile's `env` and read it with
`crosshairs()` in the payload.

### 6.6 Common free play option (`WAL_FREEPLAY`)

`WAL_FREEPLAY` (`1` / `0`; `defaults.yaml`: `1`) makes the games start without coins where the
system's payload can set it: `wal_payload_common::free_play(<system variable>)` answers the
system's own variable when set (`WAL_APM3_FREEPLAY`, `WAL_KONAMI_FREEPLAY`,
`WAL_SEGASHELL_FREEPLAY`, `WAL_EUROPAR_FREEPLAY`, `WAL_UNIS_FREEPLAY`, `WAL_NAMCOES3_FREEPLAY`: a
profile's own option wins), else `WAL_FREEPLAY`, else none (the game's setting kept). Used by
apm3 (`Credit_isFreePlay`), Konami (ARK settings, Bishi Bashi's system), Sega Shell
(`Credit/Freeplay`), Europa-R (`FreePlay`), UNIS (Qumo's `Free_Play`, After Dark) and the WMMT5
terminal. The GUI shows it in the Common options of these systems' games
(`config::FREEPLAY_SYSTEMS`); Batocera's FREE PLAY (`wal_freeplay`) writes it.

### 6.7 Common volume option (`WAL_VOLUME`)

`WAL_VOLUME` (0 to 100 percent; `defaults.yaml`: `100`) is the game's sound level, for every
game: payload-common's `volume.rs` sets the process's default audio session volume (mmdevapi's
`IAudioSessionManager::GetSimpleAudioVolume(GUID_NULL)` -> `ISimpleAudioVolume::SetMasterVolume`,
from a thread at the payload's start; under 100 only). Wine applies it to every stream of the
session (DirectSound and the DirectShow renderer on it, XAudio2, WASAPI) before the sound
leaves the process: the stream's volume in the sound server stays at 100%. A profile sets its
default (After Dark: 60); the GUI shows it in every game's Common options
(`config::VOLUME_ENV`).

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
| `point-blank-x` | Point Blank X | namcoes3 | works (headless tests: coins, start, aim and shots, operator menu), 2 guns | `PBX100-2-NA-MPR0-A63.exe` (Unity 5.1, Mono, §6.3c): `Plugins/bnusio.dll` replaced (USIO switches and coins, gun board), loaded by wal-loader at the entry point; profile `Setting.ini` (`CONTROLLER 0`); real keyboard/mouse filtered; cabinet serial in the registry; the game's own crosshairs shown; BepInEx (`winhttp.dll`) hidden |
| `mach-storm` | Mach Storm | namcoes3 | works (user: 100%) | `ACE7_WIN_10.exe` (Direct3D 9, 32-bit): `bngrw.dll` payload loaded by wal-loader at the entry point, selected by the build; JVS on COM3 (flight layout: control lever, throttle lever, machine gun, missile, third button), device errors and extra serial device checks patched, dome lookup texture replaced by a flat screen map, saves next to the executable, borderless window fitted on the screen, XInput and real keyboard/mouse filtered |
| `lost-land-adventure` | Lost Land Adventure | namcoes3 | works (user: 100%), 2 guns; test menu behind the game: alt-tab | `RSLauncher.exe` + UE3 game (64-bit, §6.3c): the payload replaces both `hasp_windows_x64_106993.dll` (dongle serial for both checks), JVS on COM3, Banapass reader on COM4 (no card), NEC projector on COM1, the gun camera on COM2 with the crosshair mapping inverted, the dumps' flat screen patch, English patch through JPN, F:/G: save drives |
| `time-crisis-5` | Time Crisis 5 | namcoes3 | works (headless tests: coins, pedals, aim and shots, tutorial, operator menu), 1 gun | `RSLauncher.exe` + UE3 game (64-bit, §6.3c): the payload replaces both `hasp_windows_x64_105293.dll` (HASP and USB key serials), Banapass reader on COM4 (no card), NEC projector on COM1, the USIO board on USB (WinUSB), the gun board on COM1 with the calibration inverted, borderless full-screen game window, F:/G: save drives |
| `tekken-7` | Tekken 7 | namcoes3 | works (user: 100%, online with Banapass cards), 1 player per cabinet | UE4 v1.12 (64-bit, §6.3c): the payload as the game's `xinput1_3.dll`, boot patches, the Namco JVS board on COM3, in-process AMCUS, Banapass reader on COM4, built-in graphene game server, saves in the dump's `Saved`, 1920x1080; no cabinet link |
| `tekken-7-fated-retribution` | Tekken 7 Fated Retribution | namcoes3 | works (user: 100%, online with Banapass cards), 1 player per cabinet | as `tekken-7` (§6.3c): `xinput1_3.dll` payload, the Fated Retribution boot patches, Namco JVS on COM3, newer IAuth, Banapass, graphene server, saves in the dump's `Saved` |
| `tekken-7-fated-retribution-round-2` | Tekken 7 Fated Retribution Round 2 | namcoes3 | works (user: 100%, online with Banapass cards, insignia rate boost), 1 player per cabinet | as `tekken-7-fated-retribution` (§6.3c): runs the dump's `.exe.orig`, Round 2 boot patches, game server port redirect, insignias |
| `pokken-tournament` | Pokkén Tournament | namcoes3 | user 100% (24.05 with Banapassport cards, 26.02 offline), 1 player per cabinet, English text | updates 24.05 and 26.02 (64-bit, §6.3c): the payload as the game's `xinput1_3.dll` (the cabinet controller as XInput pad 0, its USB id), Namco JVS on COM3 (`wajvio.dll`), dongle / cabinet serial, ping and subnet patches, Japanese locale, cards (AMCUS, COM4 reader, jackal server), GE patches 0110 / 0111 (movies), saves in the dump's `Win32\save_data`; no cabinet link |
| `jojos-bizarre-adventure-last-survivor` | JoJo's Bizarre Adventure - Last Survivor | namcoes3 | works offline (headless tests: system check, coins, start, PP purchase, mode select, operator menu), touch with the mouse, English text; no game server: national / store battles off | update 28.02 (64-bit UE4, §6.3c): the payload as the game's `xinput1_3.dll`, cabinet serial and AMActivator, USIO board with sticks, Aime reader (COM1) and display (COM2) of `libamw.dll`, tablet touch panel, store network (netsh, adapters, ping), English `Game.locres` |
| `star-wars-battle-pod` | Star Wars Battle Pod - The Force Awakens | namcoes3 | boots (2026-10-06), not user-verified | 64-bit payload as the launcher's and the game's `hasp_windows_x64_100610.dll` (by ordinal, §6.3c): HASP serial, NA-JV board on COM3 (flight stick, throttle, trigger/weapon/view), USB key enumeration, projector check skipped, flat screen |
| `wangan-midnight-maximum-tune-5` | Wangan Midnight Maximum Tune 5 | namcoes3 | works (user: game works; headless: cards, new player, car, race, save) | `hasp_windows_x64_109906.dll` payload (§6.3c), JVS on COM3, emulated terminal, Banapass cards on COM4, built-in game server (wm5srv) |
| `wangan-midnight-maximum-tune-5dx` | Wangan Midnight Maximum Tune 5DX | namcoes3 | works (user), full screen; cards and game server | as `wangan-midnight-maximum-tune-5` with the 5DX version table and server schema |
| `wangan-midnight-maximum-tune-5dxp` | Wangan Midnight Maximum Tune 5DX+ | namcoes3 | works (user), English; cards and game server | as 5DX: the 5DX+ version table and server schema |
| `wangan-midnight-maximum-tune-6` | Wangan Midnight Maximum Tune 6 | namcoes3 | works (user: 100%), cards and game server | as 5DX+: the WMMT6 version table, `bngrw.dll` card hooks, Wm6 server schema |
| `wangan-midnight-maximum-tune-6r` | Wangan Midnight Maximum Tune 6R | namcoes3 | works (user: 100%) | as WMMT6: the 6R version table, newer IAuth, nbamUsbFinder serial |
| `wangan-midnight-maximum-tune-6rr` | Wangan Midnight Maximum Tune 6RR | namcoes3 | works (user: 100%) | as 6R: the 6RR dongle serial, cabinet link group 0, Wm6RR server schema |
| `farcry-paradise-lost` | Far Cry Paradise Lost | globalvr | works (user: 100%) | dump built from the game's install disc (unshield), original executable: HASP4 dongle answered in memory (`hasp.rs`), W: drive, USBIO board and gun watchdog patches (`WAL_PATCHES`), operator keys only (`keyboard.rs`); the game's own `msvcr71` / `msvcp71` (Wine's clamps the level dependency CRCs: missing models, statue enemies), initial `systemcfg.lua` |
| `aliens-extermination` | Aliens Extermination | globalvr | works (user: 100%), 2 guns (trigger also starts), movies | wal-loader; payload `aliens.rs` (`WAL_GLOBALVR_ALIENS`): gun reader replaced (trigger, special, flamethrower, X/Y; reload = shot off screen), panel keys' `GetAsyncKeyState` answered from the sticks (start, test F1/F2, select, volume), coins into the board's coin queue; Creative OpenAL from the dump's DATA (`files: {dump}/...`); DivX 5 movies not decoded, their error boxes patched out, 1920x1080 game in a gamescope screen scaled to the real one (`-S fit`: smaller screens showed it cut off) |
| `the-swarm` | The Swarm | globalvr | works (user: 100%), 2 guns (fire, grenade, shield); stage changes: the game started again with the next map (Relaunch message), works (user: 100%); new high scores not saved | PC port dump: `the_swarm_controls.exe` run directly with its adapter's arguments; the payload is its .NET input adapter (`WAL_GLOBALVR_SWARM`: shared memory `Local\SwarmPcInputV1`, console and front end commands; `pc_start_mode.lua` a profile file rewritten before each run, players' state from memory, end of stage from `game.log`; imports the dump left unresolved go to msvcr90, a Lua error no longer crashes the game) |
| `need-for-speed` | Need for Speed | globalvr | works (user: 100%) | the cabinet frontend `UniverShell2.exe` starting Need for Speed Hot Pursuit 2 for each race, both on the payload as their `GVRInputRaw.dll` (`gvrinput.rs`); native Indeo 5 codec for the frontend's movies; 4:3 window stretched or letterboxed |
| `need-for-speed-gt` | Need for Speed GT | globalvr | works (user: 100%) | as `need-for-speed` (`NFSHP2.exe -nofrontend`): `GVRInputRaw.dll` replacement (wheel, pedals, buttons, coins, force feedback), the EA install keys; `c_folders` (C:\Gvr, C:\GvrRoot); native Indeo 5 |
| `need-for-speed-underground` | Need for Speed Underground | globalvr | works (user: 100%) | .NET 1.1 frontend `UniverShell2.exe` + `UndergroundGVR.exe` on the SQLite engine added to the dump; the dotnet11 prefix (shared with Carbon); newer `GVRInputRaw` exports, H shifter, the race's DirectInput 8 keyboard action map; frontend window fit (`frontend_fit.rs`); quartz VMR-9 runner patch |
| `need-for-speed-carbon` | Need for Speed Carbon | globalvr | works (user: 100%); force feedback not tested on a wheel | the Anark frontend `AMPlayer.exe` (`shell.am`, GVR plug-ins) starting `CarbonReleaseorig.exe -race` for each race; `GvrIO.dll` "Zeus" messages (`gvrio.rs`: buttons, axes, H shifter, dongle, force feedback); the dotnet11 prefix |
| `nascar-racing` | NASCAR Racing | globalvr | works (user: 100%) | 1.1, built from the game disc; the Carbon platform (Anark frontend, `NASCAR_GVR.exe`): frontend events declared (message 0x0D), menu selection following the wheel (`WAL_GLOBALVR_SELECTION=position`), Client 3.0 script hooks, a cabinet alone on its link (`netcomm.rs`, `linking.rs`), HASP check patched, frontend in a screen-sized popup |
| `nascar-team-racing` | NASCAR Team Racing | globalvr | works (user: 100%) | 2.0.2, as `nascar-racing` (Client 4.0) |
| `blazing-angels` | Blazing Angels: Squadrons of WWII | globalvr | works (user: 100%) | `MainR.exe`: the GVRIO board as the fake DirectInput joystick (its vendor id, `WAL_DINPUT_WHEEL_VIDPID`), flight joystick bindings; `blazing.rs`: coin and volume monitors, coin bank, dongle record, player profile; real keyboard hidden |
| `beach-head-2000` | Beach Head 2000 | globalvr | works (user: 100%) | the cabinet frontend `UniverShell.exe` (Internet Explorer with the shell's HTML pages and `ShellControl.ocx`) starting `Beach16.exe` for each play (`beachhead.rs`): pages fixed for Wine's browser, the ActiveMovie control (`amovie.rs`), buttons on joystick 0 and the gun on the system mouse, coins in the registry (`beachhead_io.rs`), shell state saved at exit (`beachhead_save.rs`); native Indeo 5; dgVoodoo |
| `beach-head-2002` | Beach Head 2002 | globalvr | works (user: 100%) | the multigame shell (`MULTIGAME_Shell`) starting `BH2.exe`: the shell's license check answered, the older `GVRInputRaw.dll` (ordinal exports, `turret.rs`: turret motion over time, the shell's stepped motion), the difficulty page one row per step; `mfc42`, the cabinet's fonts; dgVoodoo |
| `beach-head-2003-desert-war` | Beach Head 2003 Desert War | globalvr | works (user: 100%), machine gun, cannon, missiles | as `beach-head-2002` (`desertwar.exe`), dump built from the cabinet CHD; Wine's DirectDraw |
| `twisted-nitro-stunt-racing` | Twisted: Nitro Stunt Racing | globalvr | works (user: 100%) | 1.5, dump built from the game disc (unshield) and the recovery disc's system image (motion card library); the Carbon platform (Anark frontend, `NSR_S1.exe` per race); race dongle API answered (`WAL_PATCHES`), Windows XP version for the race, OpenAL from the disc's installer, cabinet database built from its schema |
| `redline-rampage` | Redline Rampage | globalvr | works (user: 100%) | 1.0, dump built from the game disc (unshield); Twisted's frontend platform with Gamepires' Gas Guzzlers as the race: its HASP HL-enveloped `GasGuzzlers.exe` replaced by `wal-gasguzzlers.exe` (loader `gasguzzlers.rs`: boots the Prana engine and the game plug-in through their exports, the frontend's race written to `GasGuzzlers.xml`), GvrIO's newer `Initialize` and the Fire/Drop/Nos1/Nos2 buttons, the board also a DirectInput game controller (`1ab7:7021`: the race binds its actions to it), cabinet database built from its schema; frontend presentation loop's message wait made non-blocking and coins spaced 0.5 s (Start early in the attract froze it) |
| `justice-league-heroes-united` | Justice League: Heroes United | globalvr | works (user: 100%), 2 players | the cabinet's game folder (from the user's wsquashfs: PhysX 2.x, 32-bit OpenAL, dgVoodoo); Direct3D 9 through the dump's dgVoodoo (straight DXVK draws the levels wrong), native d3dx9_32 (Wine's HLSL compiler rejects its shaders), native Indeo 5 for the movies; the two control panels as fake DirectInput joysticks (`WAL_DINPUT_PADS`, hooked in the engine DLL), joystick assignment pinned in its ini reads (`WAL_INI_KEYS`); coins through the Hercules coin monitor started by the payload (`carbon_shell.rs`) |
| `madden-nfl-season-2` | EA SPORTS Madden NFL Football Season 2 | globalvr | works (user: 100%) | 2.0.0.114, dump built from the game discs (unshield): the frontend `shell.exe` (.NET 1.1) starting `mainapp.exe`; `madden.rs`: `MaddenInputRaw.dll` sticks and buttons, HASP dongle, coin/coin sound/stall monitor clients (the game waited for them every frame), the frontend's screen-sized window hidden during the game; PLUSDE on SQLite (add_InfoMessage call removed), cabinet database built from its schema |
| `sin` | SiN | globalvr | works (user: 100%) | the VR pod version (2002, Quake II engine), dump built from the install disc (`SIN.iso`, WinZip installer): the frontend `test.exe` (DirectDraw, Smacker) starting `sin.exe` per play; `sin.rs`: the pod library `ogsdll.dll` kept (configuration, credits in its shared section, bookkeeping), its hardware replaced (gun buttons, head tracker angles turned by the stick, recoil as rumble, coins), the frontend's Hardlock check passed, the game through `wal-loader` with `+set gl_driver opengl32` (default: the 3dfx MiniGL), the renderer's OpenGL extension string reduced to what it knows (a long one overflows its print buffer) |
| `heavy-gear-ii` | Heavy Gear II | globalvr | works (user: 100%) | the VR pod version (2002), dump built from the install disc (`HGX.iso`, WinZip installer) like SiN's: the frontend `HG2Shell.exe` (DirectDraw, Smacker) starting `hgx.exe` per play, whose `GCI_*` exports (read by the game's `hg2sim.dll`) are a front for the pod library `ogsdll.dll`; `sin.rs` (another build of the library: its recoil and credit offsets; the frontend's Hardlock check): buttons (trigger and second button fire, thumb buttons walk), heading turns the gear and picks the arena, pitch aims |
| `pga-tour-golf-championship-edition-iii` | PGA TOUR Golf Championship Edition III | globalvr | works (user: 100%) | 2.2.2.0, dump built from the game CDs (D1 + D2, InstallShield cabinets, `unshield`); the UniverShell frontend (IE pages scripted through `ShellControl.ocx`, Beach Head's family) starting `pga_tour_golf.exe` per play through `wal-loader` (msvcrt's `CreateProcessW` hooked: `_spawnl`); HASP4 API in the control and the game (`pga.rs`: memory with the game code `PGA02.02` and country `CTRY`+`US`); the cabinet's .NET 1.0 database faked: the pages' ActiveX classes (`gvrnet.rs`, XML data sets, configuration from `gvr.net/Country/US`, free play), the game's GvrNet*.dll C++ APIs (`gvrnet_api.rs`, payload copies) and its early-bound typed DataSets (`netds.rs`, AutoDual vtable slots); `GVRInputRaw.dll` loaded by ordinal (20/25), the trackball board `UsbTrackerDll.dll` (`pga_trackball.rs`: Y counter up = swing) rolled by the stick and by the mouse/touchpad (launcher `input.mouse_wrap`: wrapping counters on rx/ry); launcher's wineboot now registers mshtml (blank shell pages otherwise); attract WMV3 movies needed GE winedmo patch 0106 (decoder flushed after a timestamp gap); game window black while loading |
| `pga-tour-golf-championship-edition-ii` | PGA TOUR Golf Championship Edition II | globalvr | works (user: 100%) | 2.1.1, dump built from the 2.1.0 game CDs (D1 + D2) with the 2.1.1 patch CD applied (its `Patch.bat`); Championship Edition III's previous version, the same frontend, game and payload: its dongle code `PGA02.01` in the HASP memory, its data sets' CLSIDs (`netds.rs`), DXWrap's video memory rounding (`(memory + 8 MB) & 0x0F000000`, 0 from 256 MB on: "not enough video memory") patched to 240 MB; `cabinet.reg` from III's (four courses less, the course lists deleted first); the install CD's REG_EXPAND_SZ course values made plain strings in both (the game got UTF-16 text and waited on a never-shown "insert the disc" box) |
| `pga-tour-golf` | PGA TOUR Golf | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | 1.2, dump built from the two install CDs (InstallShield cabinets); the first edition: no .NET database, the game `TW2002.exe` (Tiger Woods PGA TOUR 2002, debug CRT) in `C:\Program Files\EA SPORTS\Tiger Woods PGA TOUR 2002` (nested `c_folders` entry), frontend `TW2002_Shell`; shell pages fixed (`beachhead.rs`): body tag searched after `</head>` (a commented `<body>` in a script), `;` before lines starting with `++`/`--` (Wine's JScript), `<EMBED>` sounds' `play()` guarded; ActiveMovie `SelectionStart`/`SelectionEnd`; Indeo 5 movies with the dump's native `ir50_32.dll`; the 1.2 trackball library has no `Update` (payload thread fills it, buttons at +0x14); shell buttons through `joyGetPosEx` with PGA's bits; HASP4 word reads (services 3/4), its code `PGATOUR1` in the first 8 bytes; the watchdog's shared archives (`WATCHDOG_*.GVR` file mappings) made by the shell; `GVR_Resources.dll`'s date check (archives only in Jan-Feb 2003) answered and the archives built before the payload opened again; `Engine.dll`'s ripple map read above its texture (an access violation in Wine) reads the pixel itself; the loader without a console window |
| `pga-tour-golf-team-challenge` | PGA TOUR Golf Team Challenge | globalvr | works (user: 100%), trackball: the mouse or touchpad (or stick) | 4.0.0, dump built from the three install CDs (InstallShield cabinets) at the cabinet's paths (`C:\PGA_TOUR_GOLF_2006`, `C:\hercules`, `C:\GvrPlus\4`); the Anark frontend (`AMPlayer.exe Shell.am`, Client 3.0 build in `carbon_shell.rs`: script compilations, presentation loop wait) as NASCAR Racing's, .NET 1.1 prefix (`dotnet11`); the cabinet database on SQLite (NASCAR Racing's PLUSDE 1.1.0.7 + GvrSqlite, dump only) built from the decrypted `pgacabinet*.enc` + game `.tbl` files, Instant Online turned off; payload as the frontend's `GvrIO.dll` / `GvrIOd.dll` (dongle PGA 4.0 US, its game name exact, trackball direction events `WAL_GLOBALVR_TRACKBALL_EVENTS`, panel buttons) and the game's `USBIOExtreme.dll` (trackball report in the record, device SPI memory); the dongle's smart device (`DongleStorageDevice.dll`, header zero padded) answered in both (`smartcard.rs`, wrapper hooked on its load notification: the game's accounting needs it); PGAPlugIn's country and CD checks patched; Direct3D 9 on wined3d (faces white with DXVK) |
| `ultrapin` | UltraPin | globalvr | works (user: 100%) | Global VR's pinball cabinet (2007, 12 tables): the game disc's InstallShield install (`unshield`) at `C:\UPin`; the frontend `plfe.exe` (OpenGL, the cabinet's two side-turned monitors in one 2640x1024 window) starts Global VR's Visual Pinball (`VPinball.exe -play`, DirectDraw / D3D7: dgVoodoo) through `wal-loader` per table, VPinMAME a registered COM server; Microsoft's VBScript (`wsh57`, isolated in the shared prefix: wine's lacks `ExecuteGlobal` / `GetRef`); HASP4 stub by its code in both (`ultrapin.rs`), the uShock HID board faked (`ushock.rs`); the control board is the DirectInput wheel named `UltraCade Pinball` (VPinball reads the analog plunger and turns the accelerometer only for that name): buttons in the cabinet's order, buffered events with an axis heartbeat (a table ends after 3 s without one), calibration kept in the registry (the operator menu calibrates, the player reads), axis rests (spring plunger); a release of button 7 toggles VPinball's joystick ball control (no gravity): buttons only reported when they change; `ultrapin_screen.rs`: VPinMAME's DMD (GDI `StretchDIBits`, `ddraw=0`) composed into the table, the table turned upright (`WAL_ULTRAPIN_ROTATE`, default) from the primary surface's `Blt` (dirty tiles), its frames paced to the screen's refresh (it draws ~6500/s), its window kept on top and the editor / DMD windows hidden; `ultrapin_frontend.rs`: the frontend turned upright at `SwapBuffers`, hidden while a table plays; `ultrapin_movie.rs`: the DivX 5 movies (no VFW codec) decoded through `AVIStreamRead` and Media Foundation's MPEG-4 part 2 decoder (given as `MP4V`) |
| `akai-katana-shin` | Akai Katana Shin | nesica | in game (GAME_START) | `tricks: [d3dx9_37]` (Wine fails its .cfx effects, crash) |
| `aquapazza` | Aquapazza: Aquaplus Dream Match | nesica | works (user: 100%) | hide dinput8 (loader hack) |
| `arcana-heart-2` | Arcana Heart 2 | nesica | in game (user) | - |
| `arcana-heart-3-lmss` | Arcana Heart 3 Love Max Six Stars!!!!!! | nesica | in game (user, GAME_START) | D: data in WindowsLoader |
| `suggoi-arcana-heart-2` | Suggoi! Arcana Heart 2 | nesica | works (user: 100%) | as `arcana-heart-2` (its newer version: DirectDraw 7, XACT 3.5), wine's XAudio2 2.5 / XACT 3.5 (`dll_overrides`) |
| `battle-fantasia-nesica` | Battle Fantasia (NESiCAxLive) | nesica | works (user: 100%) | Type X JVS payload through wal-loader, 1280x800, A/B/C/D/G layout; no executable patches needed |
| `spica-adventure-nesica` | Spica Adventure (NESiCAxLive) | nesica | works (user: 100%) | Type X JVS payload through wal-loader, standard attack / jump buttons; no executable patches needed |
| `blazblue-central-fiction` | BlazBlue Central Fiction 2.01 | nesica | in game, NESiCA online | key bbcf, shop hours patch |
| `blazblue-chronophantasma` | BlazBlue Chronophantasma 2.03 | nesica | works (user: 100%) | - |
| `blazblue-continuum-shift-2` | BlazBlue Continuum Shift II | nesica | works (user: 100%) | hide MS dinput8 |
| `chaos-breaker` | Chaos Breaker | nesica | in fight, music | d3d8 1280x800, DirectMusic tricks (native dsound) |
| `chaos-code-103` | Chaos Code: New Sign of Catastrophe 1.03 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `chaos-code-211` | Chaos Code: New Sign of Catastrophe 2.11 | nesica | in fight (user) | CRT D: redirection (`fopen("D:/ChaosCode/...")`), `WAL_D3D9_FULLSCREEN` |
| `crimzon-clover` | Crimzon Clover | nesica | works fullscreen (user: 100%; 1280x800: pillarboxed on a 16:9 screen) | native dsound (wine dsound caps made DxLib compute a 5-million-pixel window / overrun its mixer), ranking NULL-check patches, `WAL_D3D9_FULLSCREEN` (9Ex display mode), `WAL_FONT_SCALE: 0.28` |
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
| `space-invaders` | Space Invaders | nesica | works (user: 100%) | - |
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
| `theatrhythm-final-fantasy-all-star-carnival` | Theatrhythm Final Fantasy All-Star Carnival | nesica2 | works (user: 100%), NESiCA cards (loader's card server, progress saved), touch screen (mouse, red pointer); right lever on I/J/K/L; no matching with other cabinets | `game.exe` (Unity 5.3, 64-bit); payload as `game_Data/Plugins` ismACIO / NESiCAReader / SimpleNesys (one module); NESYS online with the payload's card server (HTTP, managed `SetProtcol` / Photon `Connect` replaced); touch = Unity's mouse, red dot (Direct3D 11 Present); original `Assembly-CSharp_org.dll` installed |
| `3d-cosplay-mahjong` | 3D Cosplay Mahjong | typex | in game (mahjong hand) | wal-loader, JVS, 1280x800, `tricks: [d3dx9_33]` |
| `battle-fantasia` | Battle Fantasia | typex | works (user: 100%) | wal-loader, JVS, 1280x800, game patches, runner hotfix (winedmo) |
| `cyber-diver` | Cyber Diver | typex | works (user: 100%) | Source client hl2.exe, -io 1, serial hooks in bin/xioman.dll, complete JVS 0x25 reply, cabinet start and active-low switches, fallback axis calibration; offline; some Japanese labels garbled; testmode.exe absent |
| `sonic-blast-heroes` | Sonic Blast Heroes | typex | works (user: 100%) | wal-loader, JVS (Confirm = down, Selection = up), game_release.exe (game.exe encrypted), code page 932, the game's keyboard pad and safety sensor forced, punch from a trigger's pressure or a full force button (`WAL_TYPEX_PUNCH`, punch.rs) |
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
| `k-on-after-school-rhythm-selection` | K-On! After School Rhythm Selection | typex | works (headless tests: boot, card purchase, 3 virtual cards scanned, song select, rhythm game) | wal-loader, JVS (re-init after bus reset, card dispenser `k-on` layout), virtual card scanner with a card picker overlay (`WAL_TYPEX_KON_CARDS`, kon_cards.rs), window mode in a screen-sized popup, `WAL_ANSI_CODEPAGE: 932` (d3dx9 DrawTextA) |
| `king-of-fighters-maximum-impact-regulation-a` | King of Fighters Maximum Impact Regulation A | typex | works (user: 100%), intro movie | wal-loader, JVS, `dxvk: false`, hide the dump's Wine DLLs, patch 0x447C, runner ddraw overlay emulation (docs/KOF-MIRA-INTRO-VIDEO-DEBUG.md) |
| `king-of-fighters-xii` | The King of Fighters XII | typex | in game, intro video | wal-loader, JVS, 1280x800, A/B/C/D button map, runner quartz fix (#823) |
| `elevator-action-death-parade` | Elevator Action Death Parade | typex | works (user: 100%, perfect), 2 guns, attraction doors and bezel, crosshairs option | wal-loader, JVS (`7D` acknowledged, cabinet switch layout `elevator-action`, volume knob on analog), guns in the gun board record (`WAL_TYPEX_GUNS`, aim relative to the picture), attraction doors and bezel drawn from the dump's pictures in a screen composed over the game's window (`WAL_TYPEX_DOORS`, `doors.rs`; crosshairs `WAL_CROSSHAIR`), PhysX 2.8.1 system runtime (`tricks: [physx]`), portrait 720x1280 windowed device, 60 fps, known patches (door board error, door checks) |
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
| `usagi-online` | Usagi - Yasei no Toupai Online | typex | works (user: 100%), offline mode, no cards (Taito G-Net cards: TODO), touch = mouse or gun with a pointer dot | wal-loader, JVS (touch on the screen position input, `WAL_TYPEX_JVS_HELD`), the cabinet's lobby daemons beside the game (`WAL_TYPEX_SERVERS`), D: drive, network check timeout patch 0xD2866 |
| `wontertainment-music-channel` | Won!Tertainment Music Channel | typex | works (user: 100%), touch = mouse or gun with a pointer dot, virtual cards on an overlay | wal-loader, JVS (`wontertainment` switch layout, card dispenser, corrupted output packets read), barcode card scanner on COM3 and Idol Puppy toy on COM1 (`WAL_TYPEX_WONTERTAINMENT`, wontertainment.rs), card picker overlay (wontertainment_cards.rs), touch = posted mouse messages, windowed 640x480 letterboxed 4:3, dispenser check patch 0x3F552 |
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
| `harikiri-professional-baseball` | Harikiri Professional Baseball | typex | works (user: 100%), offline mode, touch = mouse or gun with a pointer dot | touch screen from the mouse, its pointer shown with the common crosshair, test menu |
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
| `teenage-mutant-ninja-turtles` | Teenage Mutant Ninja Turtles | rawthrills | works (user: 100%), four-player controls, test menu, variable-height jump, videos | TMNT 1.59 (`TMNT.exe`); run `tools/build-tmnt-direct-input.sh <game directory>` to patch `IOManager.Update` with direct launcher input before `UpdateCombinedInputs` (the bridge `WAL.DirectInput.dll` is also the system's 64-bit payload, built by `build.sh`: installed over the dump's copy, so dumps patched with an older bridge run the current one; on those, Unity's `Input.GetKey*` wrappers are made to return released at run time through the dump's Harmony), including per-frame counter snapshots and hold timers; player N on `GameInput` P0 + N (P0 is the game's combined input); WALInput disabled and TMNTTPPlugin input writer disabled; every `UnityEngine.Input` key / button / mouse button / axis read in `Assembly-CSharp` returns released (input from the launcher only; the level select's W developer key started a level); volume on player 1's b7 / b8 (`native_map` `volup` / `voldown`); `Pawn.UpdateJump` uses a local height/gravity helper instead of the plugin's host call, restoring landing and hold-dependent jump height; the other `UnityNatives` helpers the plugin forwards to its host (absent: each returned its default, so no wave ever completed and the level never scrolled on after the first fight, players took no damage, enemies never attacked) get results through Harmony postfixes from the bridge (`GameNatives`: wave complete once its enemies are dead, enemy attack pacing, player damage modifier rising past the cycle control time, boss hits before invincibility, throw / grab checks, thrown object rest angles, ground circle height, turn speed), on every dump; bundled Windows `OrenVid.dll` playback works; no winhttp override |
| `super-alpine-racer` | Super Alpine Racer | rawthrills | in progress: boots, a race played from scripted inputs (swing, start) | wal-loader, inputs through the game's `SetControl` without a board (§6.3b), DXVK's d3d8 (dump's d3d8to9 hidden), picture rotated for a vertical monitor (no rotation yet) |
| `x-games-snowboarder` | X-Games Snowboarder | rawthrills | in progress: boots, a race played from scripted inputs | as Super Alpine Racer, landscape |
| `winter-x-games-snocross` | Winter X Games SnoCross | rawthrills | works (user: 100%) | wal-loader, inputs through the game's `SetControl` (§6.3b), no board: I/O init taken as success, no reboot, board missing displays off; the dump's protection traps undone, keyboard developer key Space (ends the game) made Escape, base path empty, force feedback from its handlebar force (untested on a wheel) |
| `h2o-overdrive` | H2O Overdrive | rawthrills | works (user: 100%) | wal-loader, JAMMA USB board reports to the game's parser (§6.3b), the board's `Open` faked, its poll and `GetStatusBlock` replaced, service / test edges written to the press detector's rings, throttle lever centered (gas forward, brake reverse), boost its own control, Bink's sound not taken from FMOD (attract movie crash), service menu exit reboot = relaunch, keypad selects the tracks and boats, force feedback from the game's wheel force float (untested on a wheel) |
| `guitar-hero-arcade` | Guitar Hero Arcade EDITION | rawthrills | works (user: 100%) | Guitar Hero III game 1.0.5 / OS 8.4; two connected virtual guitars, OpenParrot GHA input, per-user Aspyr registry redirect, generated config and menu PAK selection, Game Explorer and window behavior |
| `virtua-tennis-4` | Virtua Tennis 4 | ringedge | works (user: 100%), test menu | wal-loader, amLib devices (SRAM, SMBus EEPROM and push switches, SMBIOS platform id), keychip and master PCP services with the DS28CN01 MACs from the game's table, JVS on `\\.\mxjvs` (§6.3d), test button relaunches into the test menu (`-t`) |
| `guilty-gear-xx-accent-core-plus-r` | Guilty Gear XX Accent Core Plus R | ringedge | works (user: 100%) | RingEdge 2 (§6.3d): platform `AAL2`, amAuth PCP service (TCP 40190), 4-byte SRAM access unit, keychip MACs from `SBYG_Table.dat` |
| `under-night-in-birth-exe-late` | Under Night In-Birth Exe:Late | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), `RingGame.exe.orig` (the dump's `RingGame.exe` is patched), amAuth 1.40 commands, JVS on COM4 |
| `under-night-in-birth-exe-late-st` | Under Night In-Birth Exe:Late[st] | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), newer version of the same game: `RingGame.exe.orig`, amAuth 1.40 commands, JVS on `\\.\mxjvs` |
| `under-night-in-birth-exe-late-cl-r` | Under Night In-Birth Exe:Late[cl-r] | apm3 | works (user: game fine, 2026-11-05); Aime cards + saves headless (2026-11-05); 1 player per cabinet; no network versus | Sega ALLS APM3 (§6.3k): replacement `apm_x86.dll`, title server in the payload, boot / online patches |
| `blade-strangers-apm3` | Blade Strangers (APM3) | apm3 | user 100% (2026-11-05), network banner removed, test menu, 1 player per cabinet, Aime cards + player data (ABAAS game server, user 100%) | Sega ALLS APM3 (§6.3k): OpenGL, `apm_x86.dll` delay loaded, e-money window, backup records, .NET test menu on the 64-bit `apm.dll` |
| `senxin-aleste` | Senxin Aleste | apm3 | user 100% (2026-11-06); headless: attract, coins, Aime cards (records, settings, rankings saved and reloaded), story play, test mode | Sega ALLS APM3 (§6.3k): 32-bit, the ABAAS game server, test mode `-t` |
| `otoshu-dx` | Otoshu DX | apm3 | user 100% (2026-11-06), free play, touch dot; headless: touch screen (mouse / gun), Aime cards (name entry, login, data reloaded), songs, test mode | Sega ALLS APM3 (§6.3k): Unity IL2CPP 64-bit, `abaasgs.dll` plugin hooked, touch emulation |
| `kasiori` | Kasiori | apm3 | user 100% (2026-11-06), link reported available; headless: Aime cards, story play, test mode | Sega ALLS APM3 (§6.3k): Unity 64-bit, `X:\lib\apm.dll` (`run_drives`) |
| `nosferatu-lilinor` | Nosferatu Lilinor | apm3 | user 100% (2026-11-06), no play sequence id; headless: Aime cards (name entry), play, test mode | Sega ALLS APM3 (§6.3k): Unity 64-bit |
| `pengo-online` | Pengo! Online | apm3 | user 100% (2026-11-06): touch menus (red dot), Aime cards, play, test mode, no link banner | Sega ALLS APM3 (§6.3k): 64-bit, touch, cabinet data |
| `puyo-puyo-e-sports` | Puyo Puyo e-Sports | apm3 | user 100% (2026-11-06), icon green; headless: Aime cards (terms, saved), play, test mode | Sega ALLS APM3 (§6.3k): 64-bit, same-program test mode |
| `milkchan` | Milkchan | apm3 | user 100% (2026-11-06); headless: Aime cards, story, test mode | Sega ALLS APM3 (§6.3k): 32-bit, .NET test program |
| `koihime-enbu-ryorairai-v3` | Koihime Enbu RyoRaiRai Version 3 | apm3 | user 100% (2026-11-06); headless: Aime cards, modes, character select, test mode; no network versus | Sega ALLS APM3 (§6.3k): 32-bit, same-program test mode |
| `goonya-fighter` | Goonya Fighter | apm3 | user 100% (2026-11-06); headless: Aime cards, play, test mode | Sega ALLS APM3 (§6.3k): Unity 64-bit, game and test program in folders |
| `shanghai-iv-ryuumon-hiyaku` | Shanghai IV Ryuumon Hiyaku | apm3 | user 100% (2026-11-06); headless: Aime cards (player saved), play, test mode | Sega ALLS APM3 (§6.3k): Unity 64-bit, `X:\lib\apm.dll` (`run_drives`) |
| `rolling-gunner` | Rolling Gunner | apm3 | user 100% (2026-11-06), full screen (-f); headless: Aime cards, tutorial, test mode | Sega ALLS APM3 (§6.3k): 32-bit SDL2, `C:\SDGW\sound` (`c_folders`) |
| `cotton-rock-n-roll` | Cotton Rock 'n' Roll | apm3 | user 100% (2026-11-07); headless: Aime cards, story, play, game over saved (ranking), test mode | Sega ALLS APM3 (§6.3k): 32-bit OpenGL, test program `gametest.exe` (.NET) |
| `dead-or-alive-6` | Dead or Alive 6 | apm3 | user 100% (2026-11-09); headless: Aime cards (player saved at game over, reloaded), solo and in-store versus against the CPU, test mode and back, network icon green; no network versus | Sega ALLS APM3 (§6.3k): 64-bit Direct3D 11, game id SDFM, `WAL_APM3_GS_ONLINE`, test program `gametest.exe` (.NET) |
| `umihara-kawase-fresh` | Umihara Kawase Fresh! for AC | apm3 | user 100% (2026-11-08), jump on b1 (left Ctrl); headless: Aime cards (player loaded, rankings), Time Attack stage start, test mode | Sega ALLS APM3 (§6.3k): 32-bit OpenGL, game id SDGU, test program `gametest.exe` (.NET) |
| `tapping-skill-test` | Tapping Skill Test | apm3 | user ok (2026-11-08): touch (mouse / gun), pinch = mouse wheel on the word (b2 / b3); headless: Aime cards (player loaded), minigames, test mode | Sega ALLS APM3 (§6.3k): 64-bit D3D11 touch game, I/O read as a winmm joystick (`WAL_APM3_JOYSTICK`), pinch = mouse wheel |
| `virtua-fighter-3tb-online` | Virtua Fighter 3tb Online | apm3 | user 100% (2026-11-07), network icon green; headless: Aime cards (terms, user data, saved), team battle play, test mode and back | Sega ALLS APM3 (§6.3k): 64-bit Direct3D 11, Model 3 game in `pre3`, same-program test mode |
| `virtua-fighter-esports` | Virtua Fighter eSports | apm3 | headless (2026-11-07): Aime cards (terms, new player saved), guest and Aime arcade play, test mode (`vf5fs\vfes.exe`) and back, network icon green; no network versus | Sega ALLS APM3 (§6.3k): Dragon Engine front end + VF5FS core DLL (ABAAS client), test program in `vf5fs\` |
| `dengeki-bunko-fighting-climax-apm3` | Dengeki Bunko: Fighting Climax (APM3) | apm3 | user 100% (2026-11-06); headless: online, Aime cards, CPU fights | Sega ALLS APM3 (§6.3k): 32-bit Direct3D 9, network state patches, `Aime_getConfirm` AimeDB |
| `guilty-gear-xrd-rev2-apm3` | Guilty Gear Xrd REV 2 (APM3) | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen | Sega ALLS APM3 (§6.3k): 32-bit Unreal Engine 3, Direct3D 9, built-in link client not started, title server by `gethostbyname` |
| `guilty-gear-strive` | Guilty Gear -Strive- | apm3 | user 100% (2026-11-06): Aime cards, the player saved and reloaded, test menu, full screen; LOCAL plays alone against a still P2, no network versus | Sega ALLS APM3 (§6.3k): Unreal Engine 4, 64-bit, Direct3D 11, Arc System Works' own game server (`tl.rs`, WinINet) |
| `blazblue-cross-tag-battle` | BlazBlue: Cross Tag Battle (APM3) | apm3 | user 100% (2026-11-07): Aime cards, points, practice rooms against the CPU (Lv.1), player saved and reloaded, test menu; no network versus | Sega ALLS APM3 (§6.3k): 32-bit Direct3D 9, Arc System Works' own game server (`bb.rs`, WinINet), app root `W:\` |
| `koihime-enbu` | Koihime Enbu | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), amLib 1.21 (2015 build), JVS on `\\.\mxjvs`, APM network client offline; its sprites are uploaded with an `UpdateTexture` DXVK refuses (1 source level, 11 destination levels): `WAL_D3D9_UPDATE_TEXTURE_FIX` copies level by level (else invisible characters) |
| `blade-arcus-from-shining` | Blade Arcus from Shining (2.10.00) | ringedge | works (user: 100%), Aime offline banner removed; no Aime card play | RingEdge 2 (§6.3d), `crus.exe` (OpenGL), amAime, JVS on `\\.\mxjvs`, profile patch: the attract banner's scene test always hides it (`0x1749b6:EB`) |
| `blade-strangers` | Blade Strangers (1.02.00, 1.3.2) | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `himekaku.exe` (OpenGL), amLib 1.21 (2015 build), JVS on `\\.\mxjvs`, APM network client offline |
| `caladrius-ac` | Caladrius AC | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `SILVER_AMP_R_ROM.exe`, JVS on `\\.\mxjvs`; its own init (no amLibInit) gives up on an invalid credit settings record (backup record 7, made by the system menu) and amCredit stays uninitialized (main loop spins): profile patch `0x13fc1:9090` goes on, amCreditInit takes its defaults |
| `melty-blood-actress-again-current-code-re2` | Melty Blood Actress Again Current Code (RE2) | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `MBAA_RWMasterBuild.exe.orig` (the dump's `.exe` is patched), amLib 1.21, JVS on COM4 |
| `phantom-breaker-another-code` | Phantom Breaker: Another Code | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `pbac_ringedge2.exe`, JVS on `\\.\mxjvs`; a windowed 800x600 device with its 16:9 picture in the middle 800x450: borderless screen-sized window, `WAL_D3D9_SOURCE` presents that band stretched; in game it also reads per player PC keys (P2 on the arrows, our P1 keys) and winmm joysticks: its keyboard reader patched out, joysticks hidden by default |
| `ge-sen-love-plus-pengo` | Ge-Sen Love Plus Pengo! | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `glpp.exe`, JVS on COM4, DIP switches `8F` (resolution bits valid: native 1280x720); it also reads winmm joysticks, hidden by default (a pad's axis at rest moved the ship) |
| `transformers-human-alliance` | Transformers: Human Alliance | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), 2 lightguns on the JVS analog channels (`p2:` inputs), 20 general purpose outputs (else Error 6402), super I/O hardware monitor, keychip SSD proof, Wine large address aware; dumped executables with `.rdata` made read-only (R6002); test menu `TestMode_dumped.exe` (JVS on COM4, item fields zeroed by a patch, keychip records from its own image) |
| `guilty-gear-xrd-rev2` | Guilty Gear Xrd Rev 2 | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), Unreal Engine 3, `Binaries/Win32/GuiltyGearXrd.exe` with `game.bat.orig`'s arguments (`-AUTH`), cabinet drive M: = the dump's SAVE folder, JVS on `\\.\mxjvs`; Aime banner (`AIME_OFFLINE`) skipped by a patch |
| `guilty-gear-xrd-sign` | Guilty Gear Xrd -SIGN- | ringedge | works (user: 100%) | as Xrd Rev 2 (same engine and Aime banner code, patch at its own address), `game.bat`'s arguments |
| `guilty-gear-xrd-revelator` | Guilty Gear Xrd -REVELATOR- | ringedge | works (user: 100%) | the version before Rev 2: as Xrd Rev 2 (same engine and Aime banner code, patch at its own address), `game.bat`'s arguments |
| `dengeki-bunko-fighting-climax` | Dengeki Bunko: Fighting Climax | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `RingGame.exe`, JVS on COM4; Aime banner choice skipped by a patch (as Under Night); vsync off (`DXVK_CONFIG` `d3d9.presentInterval = 0`), paced by the 60 fps limit: its vsync wait plus its own sleeps ran it at ~26 frames per second |
| `dengeki-bunko-fighting-climax-ignition` | Dengeki Bunko: Fighting Climax Ignition | ringedge | works (user: 100%) | as Fighting Climax, but `RingGame.exe.orig` (the .exe is a patched copy) and JVS on `\\.\mxjvs`; banner patch at its own address |
| `dead-or-alive-5-ultimate-arcade` | Dead or Alive 5 Ultimate Arcade | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `DOA5A.exe`, JVS on `\\.\mxjvs`, nothing game specific |
| `under-defeat-hd-plus` | Under Defeat HD+ | ringedge | works (user: 100%) | RingEdge 2 (§6.3d), `UDX_RINGEDGE.exe`, JVS on `\\.\mxjvs`, nothing game specific |
| `lets-go-island-3d` | Let's Go Island 3D: Lost on the Island of Tropics | ringedge | works (user: 100%) | RingEdge 1 (§6.3d), `LGI.exe`, JVS on `\\.\mxjvs` with 20 outputs and the 2 guns on analog channels 1-4 (Y/X per player, inverted; else Error 6402); test menu `-TestMode`; runs in its 2D mode |
| `lets-go-island-ringwide` | Let's Go Island: Lost on the Island of Tropics (RingWide) | ringedge | works (user: 100%) | RingWide (platform AAM), `LGI_RingW_F_safe.exe`, the RingEdge 3D version's setup: `\\.\mxjvs`, 20 outputs, guns on analog channels 1-4 (Y/X per player, inverted); challenge file `MACtbl/SBUH_Table.dat`; test menu `-TestMode`; the cabinet's 1024x600 window restored in its settings table (`data/Fraw/fwDataDB/fwDataDB_Application.txt`, the dump's says 1920x1080: test menu and crosshairs, laid out in the 1024x600 frontend, drawn far right and low), shown as a borderless screen-sized window (`WAL_WINDOW_SIZE`, `WAL_WINDOW_POPUP`, `WAL_D3D9_WINDOWED_SIZE`) |
| `sega-golden-gun` | SEGA Golden Gun | ringedge | works (user: 100%) | RingWide (platform AAM), `fs\RingGunR_RingWide.exe` (game.bat runs `exe\` from `fs`; same file), JVS on `\\.\mxjvs` (test menu `TestMode.exe -t`: COM4), 20 outputs, guns on analog channels 1-4 (X/Y per player, not inverted); challenge file `SBVE_Table.dat` missing from the dump: generated; shader `#include` (d3dx hook); 1360x768 window forced fullscreen (borderless fallback) |
| `dream-raiders` | Dream Raiders | ringedge | works (user: 100%) | RingWide (platform AAM), `prg\Game.exe` (game.bat's path: the game reads `Game.ini` and `Data` from its folder's parent; a dump with it at the root needs it moved), JVS on `\\.\mxjvs`, 20 outputs, guns on analog channels 1-4 (Y/X per player, inverted); region 04 on the keychip and the EEPROM's static record (else error 0903), a credit settings record in SRAM (else error 0025 and a hang); challenge file `SBXM_Table.dat` generated; test menu `T`; the cabinet's 1280x720 in its `Game.ini` (the dump's says 1920x1080: logos and menus at their 1280x720 pixel size, small in the middle, the picture cut on other screens) and main board DIP switches (`WAL_RINGEDGE_DIPSW` `CF`: switches 5-7 = 1280x720 per the manual; 5-7 on = 1920x1080), shown in a borderless screen-sized window with 16:9 kept (`WAL_WINDOW_SIZE`, `WAL_WINDOW_POPUP`, `WAL_D3D9_WINDOWED_SIZE`, `WAL_D3D9_ASPECT`); the test menu's GAME ENTRY MODE INDEPENDENT (SHARE, the EEPROM's default: one start seats both players, one energy bar), CONTINUATION ON, SEAT MOTION DISABLE written in the EEPROM's game settings record at each boot (`WAL_RINGEDGE_GAME_SETTINGS`); a player's sight only while that player plays (status at +0x118ff60 not 0: patched sight drawer and sprite) |
| `operation-ghost` | Operation G.H.O.S.T. | ringedge | works (user: 100%) | RingWide (platform AAM), `gs2.exe.ori` (the dump's original executable; game.bat's `gs2.exe.bak`), XACT 3 sound (`xact`), JVS on `\\.\mxjvs`, 20 outputs, guns on analog channels 1-4 (X/Y per player); gun buttons trigger, screen out (reload: off screen or right button, with change), action, change; region 01 on the EEPROM's static record and a credit settings record in SRAM (else amCredit not initialized: no credit, then error 3005); 1024x600 fullscreen device made a 1024x600 back buffer in a screen-sized window (else 1024x768, picture in part of it); challenge file `SBWA_Table.dat` generated; test menu `-testmode` |
| `target-bravo-operation-ghost` | Target Bravo: Operation G.H.O.S.T. | ringedge | works (user: 100%) | the newer version of Operation G.H.O.S.T., its setup: `gs2.exe.original`, game id SDDV (the dump's `SDDV_Table.dat`), region 04 (keychip and board, `gs2.ini` Region=4), credit record, 1024x600 back buffer in a screen-sized window, same gun buttons; LED board (COM2/COM3) absent (WARNING NO LED BOARD, one handled exception); its stage select's WMV thumbnails crashed GE-Proton's winedmo (runner hotfix `winedmo-sws-padded-output`, §3) |
| `medal-no-gunman` | Medal no Gunman | ringedge | works (user: mouse) | RingWide (platform AAM) medal game, one satellite standalone (`e_NetMgr_Enable=0`): `exe\gunman_dxF.exe` (final build); 1024x768 (DIP BF), region 01 on the EEPROM's static record, credit record; its 2009 amPlatform needs a SMBIOS table of a real size (padded with inactive structures, else no platform id: amBackup fails, error 0903); guns, gun buttons and medals on the satellite's own I/O board on COM1 (`WAL_RINGEDGE_GUNMAN_BOARD`, payload `gunman.rs`: JVS framing, commands 0x60-0x75), not JVS; coin = 10 medals (`WAL_RINGEDGE_GUNMAN_MEDALS`), Start = payout button (skips gun 2 in the controller adjustment); reload: right button or shooting off screen; without the board the game stores error 3111 in SRAM (delete `exe\sv` to clear); its boot state files (`gunman_initialize.txt`, `gunman_lamp_sate.txt`, `gunman_error_sate.txt`) in the cabinet's `C:\Documents and Settings\AppUser\`, redirected to `exe\sv\AppUser` (without the folder every boot ends on "Error Information" and hands over to the test menu); free play: the credit kept at 100 or more (`WAL_RINGEDGE_GUNMAN_CREDITS`, the u16 behind the credit manager at rva 0x68d378) and reloads free (`WAL_PATCHES`: the credit bullet counter's reload and "can reload" vtable slots, which charge a medal per bullet, pointed at the base counter's) |
| `super-monkey-ball-ticket-blitz` | Super Monkey Ball Ticket Blitz | ringedge | works (user: 100%) | RingWide (platform AAM) ticket game, trackball: `prjMonkey_tv_final.exe` (game.bat: `-noerror -fullscreen`, test menu `-t`); the executable's drive letter selects its mode (X: product: amLib, JVS, `SBVJ_Table.dat.product`; Z: development; else a PC build without I/O), patched to always X:; JVS with 20 outputs (it wants 16 at least: "JVSNode[0] does not satisfies function"); the trackball is DirectInput's system mouse (relative), the left stick / keyboard arrows added to it (`WAL_DINPUT_MOUSE_STICK`); its check for 2 mice in the raw input list (else error 1001) patched; the dump's exe is a 1920x1080 hack whose attract mode 2D stays at 1280x720 positions: the original 1280x720 restored (4 immediates), windowed in a screen-sized popup, 16:9; free play by default (amCreditInit returns -5, defaults) |
| `lets-go-safari` | Let's Go Safari | ringedge | works (user: 100%) | RingWide (platform AAM) kids' driving game: `GameSampR_RingWide.exe` (test menu program `GameTestR_RingWide.exe`); JVS on COM4, the board 837-14895 in "FE MODE" (`WAL_TYPEX_JVS_IDENTIFIER`, 1 player x 22 switches, 20 outputs; else error 6402); amStorage on mxstorage 40114 (else a minute's wait, error 1001); region 04 (English; 08 Chinese; keychip and board), credit record; challenge file `SDAX_Table.dat` generated; course buttons 1-4 = JVS buttons 3-6 (pad A B X Y), wheel = HANDLE R / L (buttons 9 / 10), a rotary encoder pulsed while left / right are held (`WAL_TYPEX_JVS_PULSE`); no analog channel read |
| `lady-luck` | Lady Luck | ringedge | works (user: 100%) | RingWide (platform AAM) medal slot machine satellite (game id SBXF), `App_Release.exe` alone (game.bat's `mbcom.exe`, the network medal bank, and `AimeReader.exe` not needed); 1280x1024 (DIP DF), region 01, credit record, challenge file `Data/amFile/SBXF_Table.dat`; its boards from `Data/script/game_conf.lua` (`IOBoardComPort` 3, `TouchPanelComPort` 1, `CPBoardComPort` 5; 0 = unused, debug): the I/O board on COM3 is Medal no Gunman's protocol with its own switches (`WAL_RINGEDGE_MEDAL_IO`, `satellite.rs`: selector panel / main door closed, reset key = service, transfer; medals counted by `66 0C`, coin = 10 medals), the control panel board on COM5 (`WAL_RINGEDGE_CP_BOARD`: buttons select 1-5, start, payout through command `81`), a 3M MicroTouch EX II on COM1 (`WAL_RINGEDGE_MICROTOUCH`, `microtouch.rs`: the mouse, its left button = b8; format tablet reports streamed through the serial layer's idle callback, Y up; the game's default calibration fits). A fresh backup shows error 3001 until the reset key (service) is turned. The game saves the I/O board's last command list in backup RAM and restores it at boot, then reads the `74` handshake reply against it (its checksum as a report: error 3105 at every boot once a payout saved the list): patched to restore it empty (`WAL_PATCHES` 0x2deac). Payout (b6) pays the medals out |
| `sonic-dash-extreme` | Sonic Dash Extreme | seganu | headless tests (2026-11-08): attract, coins, start, character select, play, tickets dispensed during play and at the end, back to attract, test menu and back; not user-verified | Sega Nu SX (§6.3m): AMEX board devices, keychip driver, registry values, JVS with 24 outputs and the ticket dispenser (`WAL_TYPEX_JVS_TICKETS`), portrait window in the dump's bezel |
| `luigis-mansion-arcade` | Luigi's Mansion Arcade | seganu | user (2026-11-08): "game is working perfect" (aim, start, play); headless tests: attract, coins, service, free play from a blank backup | Sega Nu (§6.3m), 64-bit: the Capcom protector's game started once unpacked (`WAL_SEGANU_UNPACK`), the I/O board on COM1 (guns, coins, switches; `WAL_SEGANU_LMAIO`), free play in the game's coin settings |
| `mario-and-sonic-at-rio-2016-olympic-games` | Mario & Sonic at the Rio 2016 Olympic Games | alls | headless tests (2026-11-08): power-on test, attract, coins, mode / character / event select, 100m race to the result, test menu input test; user (2026-11-08): Aime name registration, two card sessions, the save kept | Nu 1.1 with amdaemon (§6.3l): AMEX board JVS in amdaemon (`WAL_ALLS_JVS`), LED board on COM12, levers layout, amRtc sync patch, Aime (837-15286) and the CTS game server (player names and saves kept) |
| `initial-d-arcade-stage-zero` | Initial D ARCADE STAGE Zero | alls | user 100% (2026-11-10), English option user OK; headless tests: attract, guest story race, new Aime player (registration, team, car, race, saved), returning player loaded, test menu network test | Nu 1.1 with amdaemon (§6.3l), segatools idzhook reimplemented: AMEX JVS with the H shifter layout, FFB COM1, LED 15070 COM11, its store server (ServerBox) started with the payload, the IDZ game server (ARTEMiS + minime protocol), billing for Aime |
| `initial-d-arcade-stage-zero-ver-2` | Initial D ARCADE STAGE Zero Ver.2 | alls | user 100% (2026-11-10), English; headless tests: guest race, new Aime player saved, returning player loaded | Version 2.30 (§6.3l): Ver.1's board and profile, the game server's 2.10 / 2.30 protocol (minime v016 + ARTEMiS + the game's own parser for the 2.30 profile) |
| `chunithm-star` | CHUNITHM STAR | alls | user (2026-11-09): "working 100%" (Aime login, play, audio, touchpad slider); headless tests: power-on test, card registration, name entry, tutorial | Nu 1.1 without amdaemon (§6.3l, `WAL_ALLS_BOARD=chuni`): slider COM1, air sensors on JVS, LED boards COM10/11, Aime reader in aimeReaderHost, distribution server in the EEPROM, title server, billing TLS server, RTC privileges, WASAPI shared (CRI 24-in-32), windowed device, touchpad slider (`input.touch`) |
| `chunithm-star-plus` | CHUNITHM STAR PLUS | alls | user (2026-11-09): "working 100%"; headless tests: card login, registration, name entry | as CHUNITHM STAR, title uri `/SDBT/125/` |
| `chunithm-plus` | CHUNITHM PLUS | alls | headless tests (2026-11-09): attract, card login, tutorial with the slider, billing over TLS 1.0 | as CHUNITHM STAR, title uri `/SDBT/106/` |
| `chunithm-paradise-lost` | CHUNITHM PARADISE LOST | alls | headless tests (2026-11-10): title server, card login, tutorial with the slider; with SEGAguide's unpacked executables (the dump's are protected) | as CHUNITHM STAR, title uri `/SDBT/150/` |
| `chunithm-superstar` | CHUNITHM SUPERSTAR | alls | not running (2026-11-09): protected executables, amdaemon dies under Wine (direct system calls) | ALLS with amdaemon (§6.3l, `WAL_ALLS_BOARD=chusan`): profile, IO4 air sensors, slider, LED boards, title server ready |
| `teratoma` | Teratoma: The Last Rebellion | ringedge | works (user: 100%) | Skonec 2014 lightgun game on the Operation G.H.O.S.T. engine (amLib, gs2.ini, game id `SBWA`, RingWide platform AAM). `TT.exe` is packed: `wal-loader` (`WAL_LOADER_TERATOMA`, `crates/loader/src/teratoma.rs`, SHA-256 checked) runs the unpacker past its checks and loads the payload from the game's startup call. Its code calls the system through the packer's stubs: `WAL_PACKED_IMPORTS=kernel32.dll,setupapi.dll,d3d9.dll` hot patches kernelbase's functions (setupapi's and d3d9's: the unpacked import table's pointers). Its 1280x720 fullscreen device (no such mode on a 1600x1200 Xorg screen) made a 1280x720 back buffer in a borderless screen-sized window, stretched (`WAL_D3D9_WINDOWED_SIZE`, `WAL_WINDOW_POPUP`, `WAL_WINDOW_SIZE`). The dump's `root/lua/Sound/Sound.lua` wraps the game's (music and gun shot at 0.25, dialogue doubled): the game's own script, extracted from it, put back (`files`). The dump's gs2.ini (PC mouse mode) replaced by a profile file with `EnableAMLib`, `EnableJvs`, `GunController` on: amLib's JVS on `mxjvs`, the game's own JVS reader (guns, buttons, coins) on the first COM port Windows lists (`COM*`), an overlapped reader (`WaitCommEvent`, reads armed after each reply: `WAL_SERIAL_PENDING_READS`, replies 2 ms after the write's completion, else polled 5 times a second). Test menu: F2 sets `root/TestMode.sac` and asks amMaster for the next application, it stays: the payload ends it after 2 s, the launcher starts it again. The protector's watchdog threads end the game about a minute into a level (`TerminateProcess` from +0x28162a6, not every run): ignored (`WAL_IGNORE_TERMINATE_FROM`). `logo.avi` and the promo movie are WMV2 re-encodes in the dump; the in-game movies WMV3 |
| `chaos-code-ringwide` | Chaos Code (RingWide) | ringedge | works (user: 100%) | RingWide boards in the RingEdge payload |
| `ko-drive` | K.O. Drive | ringedge | works (user: 100%) | RingWide |
| `sega-racing-classic` | SEGA Racing Classic | ringedge | works (user: 100%) | RingWide |
| `sonic-sega-all-stars-racing` | Sonic & Sega All-Stars Racing | ringedge | works (user: 100%) | RingWide |
| `melty-blood-actress-again-current-code-ringwide` | Melty Blood Actress Again Current Code (RingWide) | ringedge | works (user: 100%), 4:3 | RingWide: fake DirectInput button box (`WAL_DINPUT_BOARD`), EEPROM region record, DIP switches 1024x768 |
| `initial-d-arcade-stage-6` | Initial D Arcade Stage 6 AA | ringedge | works (user: full screen, inputs, cards, offline); force feedback not felt on a wheel | OpenGL Cg shader fix (`WAL_RINGEDGE_CG_FIX`), drive board on COM1, IC card emulated |
| `initial-d-arcade-stage-7` | Initial D Arcade Stage 7 AAX | ringedge | works (full screen, cards, offline) | as Initial D 6: shader cache writes redirected, IC card chip emulated |
| `initial-d-arcade-stage-8` | Initial D Arcade Stage 8 Infinity | ringedge | works (full screen, cards, offline) | as Initial D 7, zeroed heap |
| `pleasant-goat-and-big-big-wolf-happy-happy-playground` | Pleasant Goat and Big Big Wolf Happy Happy Playground | ringedge | title, attract and coins verified; gameplay, sound, payout not verified | JVS dispenser-ready switch (`WAL_TYPEX_JVS_LAYOUT=happy-playground`), board EEPROM and keychip region 04 / model 02 |
| `puyo-puyo-quest-arcade` | Puyo Puyo!! Quest Arcade | ringedge | work in progress: does not start yet | RingEdge 2 |
| `transformers-shadows-rising` | Transformers: Shadows Rising | segashell | works (user: 100%), test menu (user) | Sega Amusements Shell PC (§6.3e), 2 lightguns, lever on right/middle click, free play; native 1280x720 (option: 1920x1080), vsync and 60 fps limit; test menu: the Shell's `Shell.exe` on TEST (straight into its test menu, inputs written in its memory, exit back to the game), gun calibration saved in `ShellData.ini` (read by the game) |
| `revolt` | Re-Volt (Tsunami cabinet) | tsunami | works (user: 100%): the cabinet's shell `Tsunami\TVTSHELL.EXE` (dump `games/arcadepc/revolt`), its console then `ReVolt.exe -coinop`; coin then the gas pedal starts a race; wheel, pedals and cabinet buttons through the TsuInput object (GetJoyInfo) | wal-loader, `TsuInput` + `TsuMotion` (idle motion seat) COM objects emulated in the payload, Wine DirectDraw, Wine DirectInput with host joysticks hidden, dump's `tsunet.dll` registered in-proc + adapter-walk patched; `tvtopcon.rs`: the cabinet software (shell, console `tvtopcon.exe`, `Launch.exe`) — Re-Volt registered in `Software\Tsunami\Apps` (profile's `launch.reg`), hardware version and null motion driver (`mdi_null.dll`) set, the console's adapter walk, license and boot waits patched, its programs started through `wal-loader`, its display mode changes kept to the operator console and undone (a 16-bit desktop hid the car bodies), the shell's reboot ending it; a dump whose `.windowsloader` is `ReVolt.exe` runs `-launchGame` (profile `args`) — see docs/REVOLT-DEBUG.md |
| `storm-racer-g` | Storm Racer G | wahlap | works (user: 100%) | `bin/racing.exe` (debug build), `usb_io.dll` replaced (wheel, gas, brake, coin switch, boost; start = its own Enter key, real keyboard and XInput hidden), force feedback (centering spring by speed, off-road vibration), WMV9 movies (`WMV9VCM.dll` in `bin/`, Wine AVIStreamRead fix), asserts answered, missing checkpoint billboards skipped, full screen |
| `storm-rider` | Storm Rider | wahlap | works (user: 100%) | `all_test/game/Gold/motobike.exe` (unprotected build), `motobikeio.dll` replaced (handlebar, throttle, brake, boost, menu buttons, coin counter, board memory in `assets/motobikeio.mem`, uncalibrated channels full range), dongle class answered, start = its own Enter key (real keyboard and XInput hidden), WMV9 movies, full screen |
| `ford-racing-full-blown` | Ford Racing Full Blown | europar | works (user: 100%) | `FordRacing/fordracing.exe`, `inpout32.dll` replaced: Happ UGCI HID board (wheel, pedals, shift up/down, start, view), the GameShell's credits (coins, free play option), parallel port force feedback, 1024x768 (option); test menu (the GameShell's) not yet |
| `sega-rally-3` | SEGA Rally 3 | europar | works (user: 100%) | `Rally/Rally.exe`, payload as `xinput1_3.dll` (native override; `inpout32.dll` loaded at run time = the payload): HID board vendor 0x1043 (wheel, gas, brake, shift up/down, handbrake, view, start), force feedback port 0x1020, credits counted in the game (coin = 1 credit, free play option), 1280x720 or 1920x1080 (in-race texts recentered) |
| `showdown` | Showdown | segashell | works (user: 100%) | `Showdown/Showdown.exe` (§6.3e), Rapture3D 2.7.4 DLLs in the dump, the Shell's DSW protocol (cabinet configuration, credits, free play option), fake DirectInput "SEGA IO Controller" (wheel, pedals, start, view, boost, reset, music), parallel port force feedback (0x378), Shell paths and settings patched, PC inputs hidden |
| `grid` | GRID | europar | works (user: 100%) | `Sega/Grid/GRID.exe`, payload as `inpout32.dll`: its board a fake DirectInput "SEGA IO Controller" (wheel X, brake Z, gas Rx, start, view, reset, gears, look back), the Sega Shell window on its command line (`/h=`, coins, credits), force feedback port 0x1020, boot crash skipped, XInput hidden |
| `nobiteke-byon-byon` | Nobiteke! Byon Byon Daisakusen | konami | works (user: 100%) | `bin/byy.exe ..\ ..\` (§6.3h), `libacio.dll` replaced: HBHI board (switches, coins, two positional guns scaled onto the default calibration, security plugs), ICCA card reader, cabinet IP setup answered, e-Amusement offline (local mode), picture at the screen resolution in a borderless window |
| `gashaaaan` | Gashaaaan | konami | works (headless test) | `bin/ballshooting.exe` (§6.3h), `iob.dll` replaced: I/O board (buttons, coins, ball supply, plugs), ballio's camera board answered (ball hits from the guns), the two overlapping screens shown as one picture, E:/F: save drives, OpenGL crosshairs |
| `gashaaaan-refill` | Gashaaaan Refill! | konami | works (user: 100%) | `IG2/exec_root/gasya.exe` (§6.3h), `libbms.dll` replaced: BM-System (credits, e-Amusement offline), ig2ACLib's boards without hardware and its inputs answered (buttons, coins, ball hits from the guns), two screens in one window, E:/F: save drives, crosshairs |
| `otomedius` | Otomedius | konami | works (user: 100%) | `otomedius.exe` (§6.3h), `libacio.dll` replaced: PJEI board (stick, buttons, coins, security plug), card reader, the libbms imports (BM-System, e-Amusement offline) and libtouchpanel imports (touch screen from the mouse, fitted to the 5:4 picture) answered, missing `ChkSum.bin` data check passed, E:/F: save drives |
| `winning-eleven-2008` | Winning Eleven 2008 | konami | works (user: 100%), e-Amusement cards (loader's server) | `HCK/bin/game.exe` (§6.3h), `libacio.dll` replaced: PJEI board (arcade stick, buttons A-F), PS2 pad port, card reader, the libtouchpanel imports answered (touched only on the PIN screen), security plug of a player cabinet, the payload passed by the ROM check, `GetAdaptersInfo` fitted to the game's buffer, Realtek equalizer answered, Live Monitor off, fake e-Amusement server (services host, its own modules: player records, login, PIN touched), or e-Amusement off, D: (exe and sound.dll) to the dump root |
| `gti-club-supermini-festa` | GTI Club: Supermini Festa! | konami | works (user: 100%) | `gti3.exe` (§6.3h), `libacio.dll` replaced: HBHI board (switches, coin stock, security plug), card reader; `dllibacio.dll` loads the payload: HGTH board (wheel and pedals scaled onto the game's calibration, motor torque and vibration as force feedback); boot calibration check passed, C: the only drive (boot time), wined3d |
| `winning-eleven-2012` | Winning Eleven 2012 | konami | works (user: 100%); e-Amusement cards (login, new player, match; save at a game's end not verified yet) | `contents/modules/weac12_bootstrap_release.dll` run by `wal-avs-boot.exe` (§6.3h, AVS 2.13 names, 30 MB heaps), region `WAL_KONAMI_DEST` (`A`: the product code `KCK-002` answered, English data); `libacio.dll` replaced: PJEI board, PS2 and PS3 pad ports, card reader; touch screen (serial panel in screen pixels, or the Japanese USB one and its COM1 LCD monitor) from the mouse, red pointer; e-Amusement (network on, PIN on the shuffled keypad, `easrv/kck.rs`, missing values read 0 at login) |
| `bishi-bashi-channel` | Bishi Bashi Channel | konami | works (headless: attract, a 1P game) | `contents/modules/bsch.dll` run by `wal-avs-boot64.exe` (§6.3h, AVS2 2.17, 64-bit); `libacio.dll` + `libacio_pix.dll` replaced: BMPU and PIX boards (4 players: R/G/B buttons, disks), card reader, LCD, kbt.dll answered; e-amusement off, network check skipped; exclusive WASAPI played shared |
| `the-bishi-bashi` | The Bishi Bashi | konami | works (user: 100%) | `launcher.exe` (§6.3h), `libacio.dll` replaced: HBHI board (3 players: red, green, blue, yellow buttons; player 3 on the analog channels), card reader, the libbms imports (BM-System offline); clock check skipped |
| `dance-dance-revolution-2013` | Dance Dance Revolution 2013 | konami | works (user: 100%), e-Amusement cards (loader's server) | `modules/mdxja_945.dll` run by `wal-avs-boot.exe` (§6.3h, AVS 2.15.8), payload loaded after it: P3IO (inputs, ACIO card readers, lights), FOOT / SATE / USBMEM serial boards, security plug signed for MDX, kbt dongles, codecs registered; P4IO + MDXF for hm65 |
| `yugioh-duel-terminal-6` | Yu-Gi-Oh! 5D's Duel Terminal 6 | konami | works (user: 100%) | `contents/exe/game.exe`, payload as libacio.dll: HBHI board + card dispenser, Pentel touch panel (COM1, libpenteltp's nopped edge jumps restored), default calibration, blank camera, red touch pointer |
| `hello-popn-music` | Hello! Pop'n Music | konami | works (user: 100%), e-Amusement cards for both players (loader's server) | `PMU/popn.dll` run by `wal-avs-boot.exe` (§6.3h, 30 MB heaps); `libacio.dll` replaced: HBHI board (2 players: OK, red, blue, yellow, green), a card reader per player (PIN typed); kbt.dll answered; DirectInput joystick config refused; e-AMUSEMENT option set with the server (`game` module as Asphyxia's plugin) |
| `metal-gear-arcade` | Metal Gear Arcade | konami | works (user: 100%), gun, e-Amusement cards (loader's server), test menu | `game/launch.dll` run by `wal-avs-boot.exe` (§6.3h); `libacio.dll` replaced: I36G cabinet and gun boards, gun aim (COM1), card reader; fake e-Amusement server (cards: touch, PIN typed by the reader; player data), or offline (boot error taken as non-fatal); free play set in the dump; 3D (line-interleaved) still on by default, START toggles |
| `steel-chronicle-victroopers` | Steel Chronicle Victroopers | konami | works (user: 100%), two flight sticks, touch screen (mouse or gun), e-Amusement cards (loader's server) | `arkkgg.dll` run by `wal-avs-boot.exe` (§6.3h, AVS2 2.16.7), `gamekgg.dll` the game; `libacio.dll` replaced: HBHI board (two flight sticks), card reader; touch screen (TwTouchDriver), LCD (COM1), TTC fonts split per face for Wine's GDI+, Japanese locale, settings marked calibrated, fake e-Amusement server (`WAL_KONAMI_EASRV`) |
| `silent-scope-bone-eater` | Silent Scope: Bone Eater (Widescreen Edition) | konami | works (user: 100%), gun or mouse, scope tap / hold | the edition's `game/BoneEater.exe` (§6.3h, a spice2x runtime: one 16:9 picture), the payload as its `dxva2.dll`: `../user` prepared (NVRAM mounts, full range gun calibration, free play), keys / gun (cursor) / scope key messages from the virtual sticks, real keyboard, mouse buttons and pads hidden, TTC fonts split per face |
| `road-fighters-3d` | Road Fighters 3D | konami | works (headless tests: attract, race, test menu) | `jgt.dll` run by `wal-avs-boot.exe` (§6.3h): AVS and e-Amusement booted from `prop/`, the cabinet's `kbt.dll`/`kld.dll` loaded; `libacio.dll` replaced: HBHI board (switches, H-pattern lever with a sequential gear, coins, security plug), card reader (workflow API); `libacioEx.dll` loads the payload: HGTH board (wheel and pedals, motor torque as force feedback); C: the only drive, wined3d |
| `contra-evolution` | Contra: Evolution | konami | works (user: 100%) | `AMContra.exe` (§6.3h), `libacio.dll` replaced (its MSVC-named imports, never called): controls on a fake DirectInput keyboard from the virtual sticks (the game's key table), coins from the COM1 control panel, clock reported set, GB2312 text (code page 936) |
| `wartran-troopers` | Wartran Troopers | konami | works (user: 100%) | `D00/game/game.exe` (§6.3h), `panicdialog.dll` replaced; COM2 pcio hub chain emulated (4 boards, firmware versions from the dump, switches, coin counter, security plug region `WAL_KONAMI_PLUG`), COM1 gun boards; the gun read replaced by the pointer; two monitors in one window (2 D3D8 devices presenting side by side); E: settings drive seeded from the dump's CFGs; DirectMusic tricks |
| `lethal-enforcers-3` | Lethal Enforcers 3 | konami | works (user: 100%), 1 gun, player 1's screen (rival's screen option), test menu | `game.exe` (§6.3h), `iob.dll` replaced (switches, coins), `gunio.dll` answered (gun boards booted); the gun read replaced by the pointer, off screen = cover/reload; `D:\D22\` paths made relative (data, DirectShow movies); boot CRC, clock and calibration checks skipped; E:/F: save drives; the game's two screens, player 1's alone shown (with one, its CPU rival crashed the stage start) |
| `lethal-enforcers-3-2p` | Lethal Enforcers 3 (2 players) | konami | works (user: 100%), 2 guns, 2 screens side by side, test menu | as `lethal-enforcers-3`, two adapters (`WAL_KONAMI_PLAYERS=2`): one 640x480 screen per player in one window (`two_screens.rs`), each gun aims at its own half |
| `coopers-9` | Cooper's 9 | konami | works (user: 100%), 2 guns, test menu | `game.exe`, a memory dump of the HASP-enveloped game: started by wal-loader with the payload as `inpout32.dll` (`payload_loader`); its imports rebuilt, `WinMain`'s instance fixed, HASP HL API answered, parallel port I/O board and guns (calibration skipped), `z:/data/bin` relative, saves on E: (`SAVE`), MP42 movies need the GE winedmo MP42 patch |
| `rainbow-bombergirl` | Rainbow Bomber Girl | konami | works (user: 100%) | `game.exe` (Unity 2017, 64-bit, §6.3h), the payload as `winhttp.dll`: BM-System's System, Io, Consume and Entry modules bound through Mono (watchdog, credits, test / service, export region, e-Amusement card: player created / loaded / saved by the payload), the I/O board's status handed to UsbIO (COM6 absent), the touch screen's touches; drawn at the cabinet's 2160x1920 desktop into a texture of the payload's, the two monitors and the bezel's marquee composed for the screen; `LC_ALL=en_US.UTF-8` |
| `castlevania-the-arcade` | Castlevania: The Arcade | konami | works (user: 100%), Little Witch unlocked; attract movie black | konami system |
| `silent-hill-the-arcade` | Silent Hill: The Arcade | konami | works (user: 100%) | konami system, GE-Proton movie restart fix |
| `action-deka` | Action Deka | konami | works (user: 100%) | konami system |
| `hyper-sports-2011-remake` | Hyper Sports 2011 Remake | konami | works (user: 100%) | `gf.dll` / Python game |
| `thrill-drive-4` | Thrill Drive 4: Crazy Streets | konami | works (user: 100%) | konami system |
| `chase-chase-jokers` | Chase Chase Jokers | konami | works (user: 100%) | 64-bit konami: libaio TBS board, trackball, touch, Mono field set |
| `galaxy-rangers` | Galaxy Rangers | arcadepc | works (headless tests: coins, start, level select, aim and shots, settings menu), 4 guns | `WXR.exe` (Unity, 64-bit, §6.3i), the dump's community `Assembly-CSharp.dll` (XInput guns): payload = `xinput1_3.dll`, four pads aiming with the guns (absolute left stick), shot off screen = reload; real keyboard/mouse filtered (user32's win32u imports, UnityPlayer's messages), F2 / arrows operator keys from test / directions; xinput1_4 disabled (payload loaded at start); profile `Controls.ini`: 1920x1080 full screen |
| `quick-shot-arcade` | Quick Shot Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, logo, bezel), 1 gun | SpiderZSoft's Top Gun repack (Unity 2019.3, 32-bit, §6.3i): rebuilt dump (`QuickShot.exe`, `Main` patched to 1080x1920), the repack's BepInEx plugins (logo, DemulShooter): the payload is the plugin's TCP client (`WAL_DEMUL_PORT`) and posts its keys; `WAL_DESKTOP_SIZE=1080x1920` fitted on the screen |
| `top-gun-arcade` | Top Gun Arcade | arcadepc | user 100% (2026-10-26: display, crosshair, settings menu, bezel), 1 gun | SpiderZSoft's original (§6.3i), dump rebuilt from the repack's decrypted assembly without BepInEx (`TopGun.exe`, `Main` patched to 1080x1920): the payload emulates the `Dog_Ch.dll` dongle and the SY Fire COM1 board through Mono's `GetProcAddress`; `WAL_DESKTOP_SIZE=1080x1920` fitted on the screen, crosshair, bezel (`WAL_BEZEL=Layer.png`, the dump's cabinet art) |
| `happy-water-sprite-2` | Happy Water Sprite II | arcadepc | user 100% (2026-10-26), 2 water guns | SpyderZSoft's "SpriteT2" (Unity 3.5, 32-bit, §6.3i): rebuilt dump without the repack's BepInEx/DemulShooter; the payload (as `hasp_windows_96625.dll`, `wal-loader`) emulates the CommKing COM1 board, the HASP HL dongle and the registration; calibration in the dump's sys.dat |
| `mech-tornado` | Mech Tornado | arcadepc | user 100% (2026-10-26: laser mode, 4 players), 4 guns | ridong's "jixiesheqiu" (Unity 2017.4, 32-bit, §6.3i): rebuilt dump without the repack's BepInEx/DemulShooter; the payload (as `gameport.dll`, `wal-loader`) is the I/O DLL's C API; calibration in the dump's `D/jijiasheqiu.ini` |
| `fire-hero` | Fire Hero | arcadepc | user 100% (2026-10-26), 2 water cannons | Mrcq's "xiaofang" (Unity 2017.4, 64-bit, §6.3i): rebuilt dump; the payload (`xinput1_3.dll`) answers its `gameport.dll` C API; calibration in the dump's `xiaofang.ini` |
| `zombie-crisis` | Zombie Crisis | arcadepc | user 100% (2026-10-26), 2 guns | SpiderZSoft (Unreal Engine 4.25, 64-bit, §6.3i): rebuilt dump; the payload is its `GamePort.dll` (`GetInstance`'s object); calibration identity in the dump's `uiback.sav` |
| `bullseye-crack-shot` | Bullseye Crack Shot | arcadepc | user 100%, 1 gun | UDC's portrait game (Unity 2019.4, 64-bit, encrypted IL decrypted by its runtime, §6.3i): no board left in the dump: the payload plays its keyboard and mouse mode with the gun (`post_mouse`, `GetCursorPos`); 1080x1920 desktop, `Screen.SetResolution` full screen on it (UI effects at 1080-wide pixels), bezel, crosshair |
| `hot-shots-fire-rescue` | Hot Shots Fire Rescue | arcadepc | user 100% (2026-10-26), 4 water cannons | Dacheng's UAF framework (Unity 2019.2 IL2CPP, 32-bit, §6.3i): rebuilt dump; the payload emulates the `io_board.dll` IO board and the SuperDog (`api_dsp_windows.dll`) through `GameAssembly.dll`'s `GetProcAddress`, `MoveGunController` jumped to the payload's (each cannon's place) |
| `frenzy-express` | Frenzy Express | arcadepc | user 100% (2026-11-02) | Uniana (OpenGL, 32-bit, §6.3i): the payload is its `xio.dll` (`XIO` class, exported input bytes); pedals pushed by turns from one button or the accelerator; needs the runner's `winedmo-parser-rewind-after-eos` (music loop hang) |
| `hog-wild` | Hog Wild | arcadepc | user 100% (2026-11-05) | Uniana (Direct3D 8, 32-bit, §6.3i): the payload is its `Interface.dll` (the original `CInterface` library forwarded, its parallel port board emulated: buttons, handle ADC); patches: the boot's volume serial check skipped (hung in Wine), the menus' handle reading restored; 4:3 window, black sides; no force feedback |
| `space-warp-66` | Space Warp 66 | arcadepc | user 100% (2026-11-06), 1 spinner, bezel | Touchmagix (Unity 2019.3, Mono, 64-bit, OpenGL, §6.3i): the payload (`xinput1_3.dll`) emulates its two serial boards and answers its Linux commands; the spinner is the mouse, left stick or left / right; 9:16 window at the screen's height, the dump's bezel |
| `pigs-might-fly` | Pigs Might Fly | arcadepc | user 100% (2026-11-06) | Gamewax (Direct3D 9, 32-bit, §6.3i): exe below the dump root (`.windowsloader`); the payload (`wal-loader`) is its USB board (vendor 0x1043), the game run without its Shell (`SHELLENABLE=0`, `/h=1`); upright portrait picture in the dump's bezel; no tickets paid; no operator menu |
| `wasteland-racers-2071` | Wasteland Racers 2071 | arcadepc | user 100% (2026-11-05) | DND Arcade (Direct3D 8, 32-bit, §6.3i): the payload is its `SDC2.dll` force feedback wheel board (axes, buttons, coin counter, motor as constant force); patches: the link's bind length, its host request to 127.0.0.1; no frame limit (loading); 1920x1080 settings; runner patch 0112 (VMR-9 device multithreaded) |
| `polar-wars` | Polar Wars | arcadepc | works (user: 100%), 2 guns, last stage unlocked | Maiqiang (Unity 5.6, 32-bit, §6.3i): exe below the dump root (`.windowsloader`); the payload (as `UnrealHid.dll`, `wal-loader`) is the UnrealIO USB board; linear calibration and potentiometer gun version from profile files; the scenario select's IL patched (last stage) |
| `skull-of-shadow` | Skull of Shadow | arcadepc | user 100% (2026-10-26), 4 guns | the "Boys" cabinet (Unity 2021.3, 64-bit, §6.3i): rebuilt dump, English; the payload emulates the BoysUSB HID board and the JHCap2 camera (black pictures), replaces the laser tracking (`Mono::replace`: each gun's place), creates `C:\\Boys` (no HASP); calibration in the dump's `TopData.db` |
| `dino-invasion` | Dino Invasion | unis | user 100% (2026-10-26), 4 guns | UNIS's shunxiang guns (Unity 5.4, 64-bit, §6.3j): rebuilt dump; the payload (`xinput1_3.dll`) emulates the COM3 board, the `LC64.dll` camera tracker and the DllBindFunc / Sense4 dongles through Mono's `GetProcAddress`; the game scene's gun hardware reads the active board (`Mono::replace`); crosshair |
| `after-dark` | After Dark | unis | user 100% (2026-11-03), 2 guns | `GamePlayer.exe` (Virtools 3.5, 32-bit, §6.3j) started by wal-loader with the payload: COM1 gun board (guns, triggers, start, coins, operator keys), the HID board's device object replaced (dongle), `GameConfig` (composition, folders, 1920x1080 full screen), fake DirectInput (COM-created) |
| `after-dark-2-3` | After Dark (2.3) | unis | user ok (2026-11-03), 2 guns; 2026-11-07 user: operator menu, sound without saturation | as `after-dark` (the same executable and profile), Microsoft's `d3dx9_41` (not in the dump) |
| `night-hunter-after-dark-chapter-ii` | Night Hunter: After Dark Chapter II | unis | user 100% (2026-11-03), 2 guns | UNIS's qumo guns (Unity 5.5, 64-bit, §6.3j): the payload (`xinput1_3.dll`) emulates the USB HID board (`SetupDi`, `HidD_GetAttributes`, overlapped reads and writes: Mono `DllImport`s) and the Sense4 dongle; identity gun calibration file; Wine's `winhttp` (no BepInEx) |
| `night-hunter-after-dark-chapter-ii-3-0-2` | Night Hunter: After Dark Chapter II (3.0.2) | unis | user 100% (2026-11-03), 2 guns, 4 missions selectable (Lost Trails) | as `night-hunter-after-dark-chapter-ii` (the same board code), four missions: the mission select at the start and the fourth open (profile config), the fourth island on the select (patched `level0` copy) |
| `raccoon-rampage` | Raccoon Rampage | unis | user 100% (2026-11-05), 4 guns | UE4 4.26 (§6.3j): the payload (`xinput1_3.dll`) answers `UsbPluginsDll.dll`'s I/O, coin and dongle functions through the executable's `GetProcAddress`; `GunDataToScreen` replaced (aim without calibration), `IsGunCalibrate` true |
| `crazy-ride` | Crazy Ride | unis | user 100% (2026-11-05), wheel, force feedback | UE4 4.19 (§6.3j): `UsbPluginsDll_R.dll` answers (I/O, wheel, coins, dongle, wheel motor: spring and vibration), the input calibration (CRLF ini), one 16:9 screen (split screen off), 1920x1080 desktop fitted, arrows steer and drive |
| `crazy-speed-arcade` | Crazy Speed Arcade | unis | user 100% (2026-11-05), wheel | Ogre 32-bit (§6.3j): COM1 game232 board (menu up / down buttons on the d-pad and Page Up / Down, the arrows steer and drive), 1360x768 windowed d3d9 on a 1360x768 desktop fitted, native D3DX, WMI / machine id / dongle action cipher / network thread patches, mirror material |
| `elevator-action-invasion` | Elevator Action Invasion | unis | user 100% (2026-11-05), 2 guns, virtual elevator doors and bezel, every level unlocked | UE4 4.26 (§6.3j): `UsbPluginsDll.dll` answers (ESIO board, USB guns with `iUSB` and a calibration, coins), every level unlocked (`GetLevelIsUnlock` patch), the elevator doors moved as the game commands and drawn over the portrait picture (`WAL_UNIS_DOORS`, Direct3D 11), 1080x1920 desktop, bezel |
| `wicked-tuna` | Wicked Tuna | unis | user 100% (2026-11-05), 4 rods on light guns or pads | Unity 2018 (§6.3j): the payload (`xinput1_3.dll`, `xinput1_4` disabled) answers `iobaordCom.dll` (four rods' serial board): gun x the rod's direction (own column or whole screen), y its angle, trigger the hook, a held button the reel; calibrated `logs/Data.dat`; rear-speaker Bass streams on the front speakers |
| `fantasy-soccer` | Fantasy Soccer | unis | user 100% (2026-11-05), 4 players | UE4 packed executable (§6.3j): `UsbPluginsDll_R.dll`'s exports patched (I/O: sticks, buttons, ball kicks; per-player coins; dongle), every player calibrated, no camera, one screen (`bEnableSplitscreen` off in the saved config), 1920x1080 desktop fitted (packed imports rescanned), arrows move |
| `mighty-moto` | Mighty Moto | unis | user 100% (2026-11-05) | Unity 2.x 32-bit (§6.3j): wal-loader, the cabinet's keys posted to the game window from player 1's stick and pedals (real keyboard filtered), fake DirectInput |
| `big-foot-crush` | Big Foot Crush | unis | user 100% (2026-11-06) | Unity 2022 32-bit (§6.3j): wal-loader, keys posted from player 1's stick, the settings the dump lacks in the profile, the operator menu's link answered, portrait fake desktop |
| `skylanders-cloud-patrol` | Skylanders Cloud Patrol | adrenaline | user 100% (2026-11-02), guns as touches | Adrenaline's touch screen game (Unity 4.2, 32-bit, §6.3k): the payload (`xinput1_3.dll`, loaded at the entry point) is its PQ Labs touch frame client (the guns touch where they point), its Kaboom I/O board and Matrix dongle, through Mono's `GetProcAddress`; crosshair |
| `rabbids-hollywood` | Rabbids Hollywood | adrenaline | user 100% (2026-11-02), 4 guns (Sixense trackers emulated) | Adrenaline's gun game (Unity 2017.2, 64-bit, §6.3k): the payload (`xinput1_3.dll`) emulates its Sixense STEM trackers (each gun turned towards its aim), its Kaboom Adrio board and Matrix dongle, puts `NewInputManager` on the guns; Wine's `winhttp` (no BepInEx) |
| `tomb-raider` | Tomb Raider | adrenaline | user 100% (2026-11-03), 4 guns (Sixense trackers emulated), operator settings saved | Adrenaline's gun game (Unity 2017.2, 64-bit, §6.3k): Rabbids Hollywood's Sixense trackers, Kaboom Adrio board and Matrix dongle; the trigger on the board's player button 2, the tracker's button reloads; Wine's `winhttp` |
| `drakon-realm-keepers` | Drakon: Realm Keepers | adrenaline | user 100% (2026-11-03), 2 turrets | Sarbakan's turret game (Unity 2021.3 IL2CPP, 64-bit, §6.3k): `InputsManager.Update` and Unity's key functions replaced through IL2CPP's API (the guns, the cabinet's keys), Matrix dongle, no serial port, WiFi stubbed |
| `rampage` | Rampage | adrenaline | user 100% (2026-11-02), 3 players | Adrenaline's 3-player monster game (Unity 2017.2, 64-bit, §6.3k): the payload (`xinput1_3.dll`) emulates its Kaboom Adrio board (joysticks, attack buttons on the keypad, callbacks from a Mono-attached thread) and Matrix dongle |
| `hot-wheels-kings-of-the-road` | Hot Wheels: Kings of the Road | adrenaline | works headless (2026-11-02), 4 of 6 seats | Adrenaline's six-seat driving game (Unity 2018.3, 64-bit, §6.3k): the payload (`xinput1_3.dll`) emulates its three Kaboom Adrio Hot Wheels boards (wheels, pedals, coins, callbacks from a Mono-attached thread) and Matrix dongle; Wine's `winhttp` (no third-party patch) |
| `need-for-speed-heat-takedown` | Need for Speed Heat Takedown | adrenaline | user 100% (2026-11-03), 1 wheel, force feedback, render scale | Sarbakan's driving game (Unity 2021.2, Mono, 64-bit, §6.3k): the payload (`xinput1_3.dll`) emulates its steering board on COM3 (Mono's `SerialPort`: the port listed in the registry, the wheel, pedals, nitro, keypad, coins; the car info played as force feedback) and Matrix dongle; Wine's `winhttp` (no third-party patch) |
| `hot-wheels-victory-lap` | Hot Wheels: Victory Lap | adrenaline | user 100% (2026-11-04), 1 wheel, force feedback | Sarbakan's driving game (Unity 6 IL2CPP, 64-bit, §6.3k): NFS Heat Takedown's steering board on COM3 through the game's `RS_232_DLL.dll` (the payload's), Matrix dongle, webcam pictures off; Wine's `winhttp` (no third-party patch) |
| `infinity-blade-fx` | Infinity Blade FX | adrenaline | works headless (2026-11-03: coins, start, fight, gun swipes, shield, dodges), 1 player | Adrenaline's touch screen sword game (Unreal Engine 3, 32-bit, §6.3k): the payload (`xinput1_3.dll`, imported by the game) hooks its PQ Labs client and Matrix dongle imports, answers its built-in Kaboom board on the serial port; gun swipe gain option; engine ini without joysticks, Direct3D 9 |
| `fruit-ninja-fx` | Fruit Ninja FX | adrenaline | user 100% (2026-11-05), gun slices, decrypted executable | Fruit Ninja FX 2's hardware path (§6.3k) on the dump's `FruitNinja.decrypted.exe` (the Matrix envelope removed, FRUIT-NINJA-FX-DECRYPTION.md): PQ Labs client and built-in Kaboom board; no dongle or figurine portal imports |
| `fruit-ninja-fx2` | Fruit Ninja FX 2 | adrenaline | user 100% (2026-11-04), gun slices | Halfbrick's touch screen game (native, 32-bit, §6.3k): Infinity Blade FX's PQ Labs client, Matrix ordinals and built-in Kaboom board (version command 24); each gun pull draws a slice (`WAL_GUN_SLICE` option); figurine portal without a reader; heap slack for its scaler; ReShade hidden |
| `jetpack-joyride-arcade` | Jetpack Joyride Arcade | adrenaline | user 100% (2026-11-04), 1 player | Adrenaline's one-button game (Halfbrick's native engine, 32-bit, §6.3k): the dump's unwrapped `mgjp_standalone.exe` (the original is in the Matrix dongle's envelope), run in `C:\Joyride\DataPC` (saves in the dump's `Saves`); the payload (wal-loader) hooks its dongle imports and its `GetProcAddress` for `KaboomComDll.dll` (keypad and coin callbacks), the removed `InitLib` call patched back |
| `spider-man-homecoming` | Spider-Man Homecoming | adrenaline | user 100% (2026-11-02), 2 buttons, portrait | Adrenaline's reflex game (Unity 5.5, 32-bit, §6.3k): the payload (`xinput1_3.dll`, loaded at the entry point) emulates its Kaboom Adrio two-feeder board and Matrix dongle; a 1080x1920 desktop (`WAL_DESKTOP_SIZE`) for its portrait full screen (NVIDIA black screen) |
| `candy-crush-arcade` | Candy Crush Arcade | adrenaline | works (user: 100%), portrait | mouse as TouchScript touches (WM_TOUCH), real cursor and red dot, portrait full screen |
| `crossy-road` | Crossy Road | adrenaline | user 100% (2026-11-03), 2 buttons, portrait (1080x1920 desktop fitted to the screen) | portrait, two-feeder board, stretched bezel, 64-bit wal-loader |
| `disney-crossy-road` | Disney Crossy Road | adrenaline | user 100% (2026-11-03), 2 buttons, portrait (64-bit, wal-loader64), stretched bezel | as Crossy Road |
| `shooty-skies` | Shooty Skies | adrenaline | user 100% (2026-11-03), 2 joysticks (the game's keys) + buttons, portrait (64-bit), stretched bezel | as Crossy Road, stick keys |
| `flappy-tickets` | Flappy Tickets | adrenaline | user 100% (2026-11-03), 2 buttons, portrait, stretched bezel | as Crossy Road |
| `mech-dinosaur` | Mech Dinosaur | arcadepc | user 100% (2026-10-26, all maps open), 4 guns | JxSoftWare's "RobotDragon" (Unity 2017.2, 64-bit, §6.3i): rebuilt dump; the payload emulates the JX COM1 board (`jxboard.rs`) |
| `wisdom-zombies` | Wisdom Zombies | arcadepc | user 100% (2026-10-26, all maps open), 4 water guns | JxSoftWare's "ShotZembies" (§6.3i): same board as Mech Dinosaur, up/down swapped |
| `top-gun-2` | Top Gun 2 | arcadepc | works (headless tests 2026-11-04: coins, start, aim and shots scoring, operator menu; user: display, crosshairs; 2026-11-05 user: levels full screen in landscape), 2 camera guns, landscape option | JXCompany's "ShotRedLight" (Unity 2017.2, 64-bit, §6.3i): rebuilt dump without BepInEx/ReShade (`TopGun2.exe`); the payload emulates the JX COM1 board (`jxboard.rs`, `jxcamera`), the SoftKey HID dongle and the camera tracker's positions; landscape by default (levels full screen, not stretched; menus, attract and loading in the bezel), portrait 1080x1920 fitted as an option, crosshair |
| `galaxian-3` | Galaxian 3 | arcadepc | user 100% (2026-11-09), 4 guns (of the cabinet's 6) | Namco's Theater 6 LaserDisc game in the dump's patched MAME (`mame.exe gal3`, H.264 laserdiscs, §6.3i): the payload is MAME's raw input devices (a keyboard and one absolute mouse per player, `mame.rs`), `ctrlr\wal.cfg` maps them; startup popups patched out; ReShade off by default; stretch option (`keepaspect`) |
| `burning-striker` | Burning Striker | arcadepc | user 100% (2026-11-09), 2 players (stick, 3 buttons) | Eolith's football game (DirectDraw / D3D7, 32-bit, §6.3i): COM1 board and DIP switches emulated (free play option), the protection's 3ddata.pak key from the dump's `eolith.key`; intro movie needs GE patches 0116/0117 (qasf DMO wrapper); focus loss no longer minimizes (wined3d behind d7vk) |
| `come-on-baby` | Come on Baby! | arcadepc | user 100% (2026-11-09), 2 players (stick, 2 buttons, start) | ExtraLine's party game (Glide 2, 32-bit, §6.3i): the parallel port board's reader replaced (sticks, buttons, operator keys), the menu's service select given its own edge detection, free play option over `eep.rom`'s coins, the C: serial check patched; the runner's gxvk (a build after 0.1: chroma key) in a payload window with its own message loop, 4:3, black bezel |
| `come-on-baby-2` | Come On Baby! 2 Winter Championship | arcadepc | user 100% (2026-11-09), 2 players (stick, 2 buttons, start) | The sequel (DirectDraw / Direct3D 7 on d7vk, §6.3i): `comeonbaby.rs`'s second build (the parallel port reader, free play over `eep.rom`), the lamps' `out` writers and the C: serial check patched, the attract movie's `IDirectDrawVideo` setup skipped (the MPEG plays), the Korean operator menu in code page 949 |
| `robin-hood` | Robin Hood | arcadepc | user 100% (2026-11-09), 1 gun | ICE's ticket redemption crossbow game (Direct3D 9, 32-bit, §6.3i): the serial board emulated (switches, coins, tickets paid at once), the crossbow's DirectInput joystick X from the gun's X position, the linked HASP HL API patched, free play option over `coinfree`, the dump's root as working directory |
| `akuma-mortis-immortal` | Akuma Mortis Immortal | arcadepc | user 100% (2026-11-09), 2 guns | Lightgun game (Direct3D 9, 32-bit, §6.3i): the parallel port gun board emulated (`inpout32`'s `Inp32` / `Out32`: aim, triggers, switches, a gun sample per shot), the Dinkey dongle's checks patched, free play option, crosshairs, the dump's bezel |
| `dcop` | DCOP | arcadepc | user 100% (2026-11-09), 1 gun | Klockworks' live-action police shooter (Unity 2020.3, 64-bit, §6.3i): the game plays with the mouse (left shoots, right reloads, any key starts / continues: free play by design), played by the gun (`dcop.rs`: `post_mouse`, `GetCursorPos`); the dump's DemulShooter plugin kept (Uduino serial off, NVRAM saves, pause menu on 0), told to hide the game's cursor; common crosshair, 16:9 borderless window (other screens: a 1920x1080 desktop, fitted or stretched with KEEP ASPECT RATIO off) |
| `ballistics` | Ballistics | arcadepc | user 100% (2026-11-09), 1 player (steering, throttle, buttons) | Grin's tube racer, arcade version (Direct3D 8 on wined3d, §6.3i): the cabinet's HAPP UGCI GameFrame as the common fake DirectInput joystick (by name, X steering, Y throttle, buttons by name) next to a DirectInput keyboard, coins as the coin program's `COINDROP` window messages, free play as the game's price 0, no hourly restart, the display settings dialog accepted, the D-Box seat's library not loaded, 4:3 window |
| `bug-busters` | Bug Busters | arcadepc | user 100% (2026-11-09), 2 guns | MIZI Production's spray gun game, PC release (DirectDraw on wined3d, 32-bit, §6.3i): the guns and the dead serial I/O board's switches written into the game (`bugbusters.rs`: records, tank reloads, start / test / service flags, coins with their sounds), the CD check patched, free play as the game's price 0, test opens the setup menu from the title, the game's own pointers as the crosshair option, 4:3 centered by Wine's mode scaling (aim checked on 16:9 and 5:4), the Sinden border drawn into the game's back buffer before each present |
| `arctic-thunder` | Arctic Thunder | arcadepc | user 100% (2026-11-09), 1 player (handlebar, throttle, attack) | Midway's snowmobile racer (Glide 3 on the dump's nGlide, 32-bit, goose library, §6.3i): the payload is the coin and security manager wincsm whose memory goose reads (switches, handlebar / throttle, credits, coin lock), the textures' disk-serial RC4 key (0x0a6a209a, base provider) and their read-only + hidden marks, FP exceptions kept masked, free play as `COIN_FREE_ONLY`, no feedback calibration, 4:3; test starts the operator diagnostics (`diag.exe`, payload as its wincsm too), Exit returns to the game |
| `ultimate-arctic-thunder` | Ultimate Arctic Thunder | arcadepc | user 100% (2026-11-10), 1 player (handlebar, throttle, attack) | Arctic Thunder's 2004 build (§6.3i): dump = the `Midway games` folder; same board by `snow.exe` build: its own texture key (0x0cc4cc8d), its parallel port dongle (`kfunc` answered by the payload, its `ppmon.exe` monitor not started), diagnostics and Exit as Arctic Thunder's |
| `hydro-thunder` | Hydro Thunder | arcadepc | user 100% with VCThunder 0.5 (2026-11-10), 1 player (wheel, throttle, boost, 3 views) | Midway's powerboat racer on the VCThunder 0.5 runtime (Glide, 32-bit, §6.3i): the fake DirectInput wheel and its buttons from the virtual sticks, constant force to the launcher's wheel, GPU renderer on the runner's vkd3d-proton, 60 fps, free play option, operator menu (Winsock kept), 4:3 full screen, widescreen option |
| `offroad-thunder` | Offroad Thunder | arcadepc | user 100% with VCThunder 0.5 (2026-11-10), 1 player (wheel, pedals, nitro, 3 cameras, shifter) | Midway's off-road racer on the VCThunder 0.5 runtime (its own dump, §6.3i): as Hydro Thunder, the shifter's gear latched by the payload (first gear at start) |
| `wild-west-shoot-out` | Wild West Shoot Out | arcadepc | user 100% (2026-11-10), 2 guns | Yingqi's western shooter (Unity 5.6, 32-bit, §6.3i): the dump's DemulShooter BepInEx plugin (dongle, serial board, camera guns), the payload its TCP client with its reflection packet (`WAL_DEMUL_FIELDS`), aim in Unity's 1440x1080 picture (`WAL_DEMUL_RESOLUTION`), a shot starts, coin keys per player, free play and the game's crosshairs in its `GameConfig.xml` (`WAL_ARCADEPC_XML`); Doorstop's `GetProcAddress` hook no longer chained by the payload (endless recursion) |
| `mars-sortie` | Mars Sortie | arcadepc | user 100% (2026-11-10), 4 guns | MRCQ's shooter (Unity 5.4, 64-bit, §6.3i): the dump's DemulShooter BepInEx plugin (I/O board, dongle, laser camera guns), the payload its TCP client (reflection packet, aim in Unity's 1680x1050 picture, coin keys per player), board `marssortie`: free play and the game's crosshairs by IL patches of their getters, the crosshairs of the playing stations only, the players' photos from the webcam when present, the operator menus' keypad `keybd_event` posted past the keyboard filter, records made in the run folder |
| `family-guy-bowling-arcade` | Family Guy Bowling Arcade | arcadepc | user 100% (2026-11-10), 1 trackball | Team Play's bowling game (Linux cabinet folder + Unity 5.4 Windows player, 64-bit, §6.3i): the dump's BepInEx loader disabled, the payload is the cabinet's `teamplayio` / `serialio` libraries for the game's DllImports (buttons, coins in cents, the trackball from the mouse's wrapping counters or the stick), free play in its `Adjustables` |
| `bounty-hunter` | Bounty Hunter | arcadepc | user 100% (2026-11-10), 1 player (8 pads) | GCTECH's striking game (Direct3D 8, UPX, §6.3i): its COM1 pad board emulated (the dump's keyboard settings hidden: the board defaults), the stub's `GetProcAddress` hooked for the serial functions, the development keys skipped, the engine's leak-tracking allocator replaced (loading 2.5 min to 10 s), free play, the dump's ArcCabView rotation on DXVK |
| `friction` | Friction | arcadepc | user 100% (2026-11-10), 2 guns | Venum Spyder's shooter (Direct3D 9, 32-bit, §6.3i): the payload is its gun board's `vsIOBoard.dll` (the game's listener: guns, coins, start, operator console), the community library's game patches in the profile, free play as 99 credits never spent, a shot is also its player's start, the payload's crosshairs (common option) |
| `house-of-the-dead-2-remake` | The House of the Dead 2: Remake | arcadepc | user 100% (2026-11-10), 2 guns | MegaPixel's PC remake (Unity HDRP, Mono, 64-bit, §6.3i): player 1's gun is the mouse, player 2's a pad (Windows.Gaming.Input disabled), crosshairs placed at the guns (Mono method replacement), start = new cooperative campaign from the main menu (PC menus skipped) or continue, player 2 joins with start, free play = the game's unlimited credits, coins as credits/continues otherwise, campaign mode and difficulty options |
| `mad-bullets` | Mad Bullets | arcadepc | user 100% (2026-11-10), 1 gun | Ironbird's PC shooting game (OpenGL, 32-bit, §6.3i): the gun plays its mouse (window messages), the crosshair drawn by the payload over its picture (the game's cursor hidden), OpenAL from winetricks, reload = a click on the bullet belt (b2 or a shot off the screen), Escape on start |
| `pop-moto` | Pop Moto | arcadepc | user 100% (2026-11-10), 1 player (handlebar, throttle, item) | Tecway's motorbike racing game (3D GameStudio, Direct3D 9, 32-bit, §6.3i): the payload is its `USB_board.dll` (handlebar, throttle lever, keys, coins) and patches the engine's scripts as the community library, no server, free play as the game's free mode |
| `subway-surfers-arcade` | Subway Surfers Arcade | arcadepc | user 100% (2026-11-10), 1 player | An arcade conversion of Kiloo / SYBO's game (Unity 2022.3, 64-bit, §6.3i): the dump's BepInEx stub disabled, the payload posts the cabinet's keys (arrows, 1 start, 5 coin) and the hoverboard's double tap, the left stick presses the arrows once per tilt, free play as a coin before the start, COM5 (the lamps' Arduino) a silent port |

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

winmm joysticks are hidden from every game (`WAL_JOYSTICK_DISABLE=0` shows them): a cabinet has
none, and games that read them next to their board (Ge-Sen Love Plus Pengo, Phantom Breaker)
got the pads twice.

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
