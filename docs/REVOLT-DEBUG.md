# Re-Volt (Tsunami cabinet) — debug notes

Status (2026-10-06): **WORKS** (user-confirmed) with `args: [-launchGame]`:
coin, then the gas pedal starts a race; wheel and pedals work.

* `-launchAttract` is the demo only: a paid gas press makes it exit 1 (back to the operator,
  whose `tvtopcon` relaunches `-launchGame`).
* `-launchGame` / `-coinop` / no argument create the **TsuMotion** COM object (motion seat,
  CLSID {BAC8CFED-0530-11D6-9D8A-0001031DF57D}, IID {BAC8CFEC-…}, global 0x9f74ac, init
  0x4c8510); unregistered, its creation failure was the old hang. `tsumotion.rs` emulates an
  idle seat (slots 9, 10, 11, 14, 15, 19, 20, 22, arg counts from the call sites).
* Still to do: make the cabinet env (`C:\Tsunami\` + `launch.reg`) reproducible from the
  profile/payload, and check whether it's needed at all now.

What session 2 found (supersedes "DirectInput — current blocker" below):

* **run13 crash cause**: `DirectInputCreateA` is the standard 4-arg call; the 5th pushed word
  (0x44a994 `push ebx`) is a register save. The 5-arg hook popped 20 bytes, so `pop ebx` got
  the caller's return address (`ebx=0x458844`) → jump to garbage.
* **DI devices A/B are the system keyboard and mouse** (GUID_SysKeyboard/SysMouse,
  c_dfDIKeyboard/c_dfDIMouse); the 256-byte state is the keyboard. `EnumDevices(DIDEVTYPE_JOYSTICK)`
  lists joysticks; then two built-in controllers without a DI device: "Driving Pod" and
  "TsuMo Joystick, Throttle". `dinput.rs` now forwards to Wine's DirectInput and answers the
  joystick enumeration empty (a host pad would otherwise become controller 0).
* **Wheel, pedals and cabinet buttons all come from TsuInput m21 GetJoyInfo** (poll 0x44b9c0,
  joy struct at *0x9f746c: +4 wheel, +8 brake, +0xc gas, +0x1c buttons): o1 wheel
  (`(raw-127)/127`), o2 brake, o3 gas (0..255 each in layout 0), o4 POV (-1), o5 buttons
  (bit 0 WHEEL, 1 ABORT, 2 MUSIC, 3 VIEW1, 4 VIEW2, 5 VIEW3). The layout (= controller index
  0x9b6ad8) is set through m22 SetJoyRange; `tsuinput.rs` adapts to it.
* **Real TsuInput slot names**, recovered from `TsuInputLib.dll` (each flat export calls one
  slot): 10 NumCoins, 11 CoinsPerPlay, 12 SufficientCredit, 13 DeductPlay, 14 GetCoinMode,
  15 SetCoinMode, 16 GetCoinType, 17 GetMotionEnabled, 18/19 Get/SetVolumeMode, 20 GetVolume,
  21 GetJoyInfo, 22 SetJoyRange, 23 SimulateCoin, 24 SimulateMotionSwitch,
  25 SimulateOperatorSwitch, 26/27 Enable/DisableWatchdog, 28 PulseWatchdog, 37 EndPlay,
  38 StartPlay, 39 SetStatParam, 40 SetWatchdogTimerTimeout. The earlier guesses (m12 = START
  button, m10 = SufficientCredit...) were wrong.
* **Race start** (0x4ba2c1): `m12 SufficientCredit != 0`, gas < 0.25 arms, gas > 0.75 starts.
* In attract a cabinet button (o5) or START with a credit quits (exit 1 = back to operator),
  so START is not mapped to a cabinet button.
* No launch argument: blocks after TsuNet init like `-coinop` (0x9f7d1c = 1 standalone path).
* Launch modes (0x4cb8e0): `-coinop` 1, `-launchGame` 2, `-launchAttract` 3, `-nohud`.
* Profile: keyboard Left/Right wheel, Up gas, Down brake; pad R2 gas, L2 brake.
  Input scripts accept `accel` / `brake` (pedal held fully).

## Reproduce

```
cd /home/aderumier/code/windowsarcadeloader
./build.sh
timeout 300 dist/arcade-launcher run "games/misc/ReVolt" --profile /tmp/opencode/revolt-d7vk.yaml
# --input-script FILE: e.g. "30 p1 coin", "30.3 p1 -", "33 p1 accel" (start a race unattended)
```

* Test override `/tmp/opencode/revolt-d7vk.yaml`: prefix `/tmp/opencode/revolt-prefix`,
  `graphics: d7vk`, `args: [-launchattract]` (drop `args` to get the profile's `-launchGame`).
* Payload log: `/tmp/opencode/revolt-prefix/drive_c/wal/tsunami/revolt/wal-tsunami.log`
  — filter with `grep -a "tsuinput\|dinput\|crash"`.
* Full disassembly (objdump, French-locale; externals are IAT thunks — e.g.
  `call 0x4cea80` = `DirectInputCreateA`): `/tmp/opencode/revolt-disasm.txt`.
* Past runs: `/tmp/opencode/run11.log` (clean exit), `run12.log` (attract demo, no DI),
  `run13.log` (DI crash).
* `WINEDEBUG=+relay` output goes to the launcher's **stderr** (this GE-Proton build
  ignores `WINEDEBUGFILE`): `WINEDEBUG=+relay timeout 45 dist/arcade-launcher run ... > relay.log 2>&1`.
* **Never `pkill -f "ReVolt.exe"`** (matches your own shell). Use `pkill -x ReVolt.exe`
  / `wal-loader.exe` / `arcade-launcher` / `tvtopcon.exe`.
* XWayland here is blind to window tooling; the user at the real monitor is the only
  eye on rendered output. d7vk has no screenshot shim (WAL_SCREENSHOT only covers
  d3d8/d3d9).
* The cabinet env in the test prefix (`C:\Tsunami\` copy + `launch.reg` applied to
  HKLM) is **manually set**, not yet reproducible from profile/payload — do that once
  the game is playable (see Next, step 6). Source files: `games/misc/ReVolt/`.

## White screen — SOLVED (history)

`-coinop` / `-launchgame` idle in `GetMessage` before the first frame, waiting for a
cabinet (`tvtopcon`) session signal — that was the "white screen". It is **not** a
bug: on the real cabinet `tvtopcon.exe` runs the session. The fix was the launch
argument: **`-launchattract` boots straight into the render loop and renders the
attract demo with no cabinet session** (the cabinet's own `launchAttract` mode).

Consequences, both confirmed with the user:

* Attract mode is **demo-only**. Coin + start in attract makes the game **exit
  cleanly** (run11, exit status 0) — that is "return to operator"; on the cabinet,
  `tvtopcon` then relaunches the real game (`-launchgame`).
* **Starting a real game requires the gas pedal (throttle), NOT the start button** —
  user: "only coin is working. I need to press gas pedal to start". Gas = virtual
  stick `Axis::Accel`, brake = `Axis::Brake` (`crates/protocol/src/lib.rs`,
  `axis_u8` scales 0..255). The pedals come from the **DirectInput device**, not
  TsuInput (see below) — hence `dinput.rs` must work before a real race can start.
* Input scripts (`crates/launcher/src/script.rs`) are buttons-only, so the gas pedal
  cannot be scripted today; the user must press it on the real stick. (Axis support in
  scripts would be a nice-to-have later.)

## DirectInput — current blocker

### What the game does (from the disassembly)

The only DI entry point imported is `DirectInputCreateA` (IAT thunk `0x4cea80`).
The whole device setup is one function, **0x44a98e–0x44b111**:

1. `DirectInputCreateA(hinst, 0x500, 0x8eaf7c, 0, 0)` — **5 words pushed**; the
   created object is read back from global `0x8eaf7c`, i.e. the game passes the
   **out-pointer as arg3** (it does not pass an IID — wine's real
   `DirectInputCreateA` would deref `[0x8eaf7c]` as an IID, fail with
   E_NOCLASSEXIST, and the game then **tolerates the failure**: logs it, sets flag
   `0x909a94=1`, returns 0). That is why run12 (no hook) renders fine.
2. `CreateDevice(&GUID@0x4dffe0, out=0x8f0f90, outer=0)` — device A (4 words).
3. A: `SetDataFormat(0x4e093c)` (2 words; **custom** DIDATAFORMAT, not a standard one).
4. A: `SetCooperativeLevel(hwnd 0x909adc, 0x6)` (3 words).
5. `CreateDevice(&GUID@0x4dffd0, out=0x8eaf78, outer=0)` — device B (different GUID).
6. B: `SetDataFormat(0x4e0924)`.
7. B: `SetCooperativeLevel(hwnd, 0x5)`.
8. **5-word call to DI vtable slot 4 (0x10)**: `(DI, 0x4, 0x44b150, 0, 0x101)` —
   a callback-style enumeration. Callback `0x44b150` itself calls **DI slot 3
   (CreateDevice)** with `guid = (callback_arg1 + 4)`, `out = local`, `outer = 0`,
   then QI's the result and builds a 0x17xx-byte device descriptor. It increments
   global `0x8f0ff8` (device count) on success.
9. If count == 0 (our case, since the callback is never invoked): the game builds
   the full **"TsuMo Joystick, Throttle" device descriptor from embedded .rdata
   strings** (0x4e9xxx) into .data `0x8eaf80` (+ a per-device area at
   `0x8e977c*4*count`), zeroing function slots at offsets 0x17dc–0x17f4, then
   `call 0x4c4100` with a 0x418-byte stack struct, and returns.
10. Per-frame poll (function ~0x44b820, called from 0x4443de, 0x45937a, 0x4922e7,
    0x496f01, 0x4c48b0 — the race simulation): device A
    `Acquire` (slot 7) → **`GetDeviceState(0x100, buf 0x8eae68)` (slot 9, standard
    2-arg)** → the 256-byte buffer is copied to global `0x8f0ffc`. A helper at
    0x44b120 does device B `Unacquire` (slot 8) + `SetCooperativeLevel(hwnd, 0x6−hr)`.

The game reads **bytes only** from the state buffer (button-byte offsets
0x00,0x01,0x04,0x05,0x08,0x09,0x0d,0x0f,0x15–0x18,0x23–0x27,0x30,0x33–0x35,0x3a,0x3f,
0x43–0x4a,0x98,0xb1,0xc5,0xc7,0xc9,0xcd,0xce,0xcf). The wheel (steering) is NOT in
the buffer — it comes from TsuInput m21. Gas/brake are somewhere in the buffer
(offsets currently guessed as `[0]`=gas, `[1]`=brake).

### Our shim (`crates/payload-tsunami/src/dinput.rs`)

IAT-hook of `dinput.dll!DirectInputCreateA` (5-arg, `out` = arg3, matching the
game's push order — a wrong-arity stdcall hook corrupts the stack). Static
`DINPUT` (11-slot vtable) + `DEVICE` (32-slot vtable); `create_device` returns the
same `&DEVICE` for both GUIDs; `get_device_state` zero-fills 256 bytes, sets
`[0]=Accel`, `[1]=Brake`, logs on change. `dinput::init()` is wired in lib.rs
DllMain.

### The crash (run13)

After the 2nd `CreateDevice` log line, before any `GetDeviceState`:

```
crash: access violation (writing 0x458844) at 0x400003 ReVolt.exe+0x3
eax=1 ebx=0x458844 ecx=0xc0000034 edx=0x8eeb08 esi=0x400000 edi=1 ebp=ffffffff esp=0x00affc84
exit status 5
```

Reading: the CPU **started executing at 0x400000 — the PE/DOS header** (a `call`
through a pointer whose value is `0x400000`, the image base = garbage); byte at
offset 3 decodes as `add [eax], al` with `eax=1` → the write fault. `ecx=0xc0000034`
(E_INVALIDARG) is a leftover. So the game `call`ed a **function pointer slot that
holds 0x400000** — most likely a slot our shim was supposed to fill (the callback at
step 8, or something the fallback table expects) but didn't. 0x458844 (ebx) is a
game function prologue — the value it was about to store.

### Next steps (in order)

1. **Per-slot logging**: log the slot index on every call into both vtables
   (slots 0–17 at least, with the args). Re-run `-launchattract` and identify the
   exact call after the 2nd `CreateDevice` that precedes the NULL-ish call.
2. Cross-check that call's push count in `/tmp/opencode/revolt-disasm.txt`
   (setup continues from 0x44a9f2 / 0x44ab64 onward; the function ends at
   0x44b111, last internal call `0x4c4100`) and fix the shim (arity, or the slot's
   semantics — e.g. it may need to *call the callback at step 8 once with a valid
   device instance*, or fill a function-pointer slot).
3. Consider **per-GUID device instances** (devices A and B use different GUIDs and
   globals; the game may rely on them being distinct objects).
4. Safe fallback to keep the game alive while debugging: make the `create` hook
   return `E_NOTIMPL`/`E_NOCLASSEXIST` (unhooked path = attract works, no pedals).
5. Once stable: user coins + presses **gas pedal** (not start); iterate the
   gas/brake byte offsets in `get_device_state` (guess is `[0]`/`[1]`) based on
   user feedback + `dinput: GetDeviceState gas=…` log lines.
6. Once playable: make the cabinet env (`C:\Tsunami\` + `launch.reg`) reproducible
   from profile/payload setup; update docs + commit.
7. Fallbacks if DI can't be made to work: drive `tvtopcon.exe` to launch
   `-launchgame` (find its window via `wine server`, post `TSU_COIN_MSG`), or patch
   the game's cabinet-wait (init fn 0x4cba00 sets global 0x9f7d1c; TSU_MESSAGE
   handler 0x4b19e0). Note `TSU_OPCONS_MSG` (0xc03d) posted to the game made it exit
   cleanly — handle with care.

## What the game is

Re-Volt (Tsunami/TsuMo Racing cabinet, game id `revolt`, system `tsunami`).
`ReVolt.exe` is a plain PE32 i386 (ImageBase 0x400000, .text 0x401000, .rdata
0x4df000, .data 0x4e3000, .rsrc 0x9fa000; file offset = VA − 0x400000) that imports
only system DLLs: DDRAW, DINPUT, DSOUND, mss32, ole32, OLEAUT32, WINMM, WS2_32,
ADVAPI32, GDI32, KERNEL32, user32 — **no cabinet driver import**, so the payload is
injected with `wal-loader` at entry 0x4d1309.

D3D7 via DirectDraw; `graphics: d7vk` is the working graphics path (d7vk in
GE-Proton11-7 is commit d6faa06, 2026-09-15 — its release notes mention Re-Volt
specifically). Gamescope + d7vk crashes (nested Xwayland), shelved.

The dump's `Tsunami/` folder is the cabinet software suite: `tsuinput.exe` (the
input/coin daemon), `TsuInputLib.dll` (its in-proc COM server, 27 flat `TsuInput*`
exports), `tsumotion.exe` + `TsuMotionLib.dll` (motion seat), `tsunet.dll`
(network, the "TsuNet" in-proc COM server), `cbw32.dll`, `tvtopcon.exe`, a custom
`ddraw.dll` and input `in_*.dll` (GCI) stubs. `launch.reg` is the cabinet app
config (real cabinet: `-coinop`, `launchGame`, `launchAttract`; paths
`c:\games\ReVolt` — they don't match our `C:\wal\tsunami\revolt` layout).

The dump also contains `revolt.map` / `revoltDebug.map` and PDBs — symbol sources
for future reverse engineering.

## TsuInput (emulated, working)

The coin mechanism is a COM object created early-bound:

```
CoCreateInstance(CLSID {74B3FA08-50CC-4275-A5CF-8BBF6DDB75C2}, NULL, CLSCTX_ALL, IID_IUnknown)
  → IUnknown::QueryInterface({5AE0E518-373D-4323-98DE-EAF0046041E5})
```

no TLB anywhere in the dump, so the 49-slot vtable was reverse engineered from the
call sites. The payload (`crates/payload-tsunami/src/tsuinput.rs`) IAT-hooks
`ole32!CoCreateInstance` in the game module (`0x4df384`; other ole32 slots:
0x4df378 OleRun, 0x4df37c CoInitialize, 0x4df380 CoUninitialize) and answers only
the TsuInput CLSID, forwarding everything else to the original. It is also
registered as a real in-proc COM server of the payload
(`DllGetClassObject`/`DllCanUnloadNow` exports), so it no longer depends on the
IAT hook (wine can restore the hooked slot when loading later modules — an
IAT watchdog re-hooks it anyway). WINE's `OleRun` wrappers QI `ITypeInfo` on the
result and proceed with S_OK on `E_NOINTERFACE`.

The object is cached in the game global `0x9f7468`. x86 stdcall: each vtable slot
must `ret` exactly arg-count×4.

Observed init sequence (payload log): `m7 Attach=0x1032aa4` (handle from a 12-byte
struct) → `m22 SetJoyRange(0,0,0xff)` → `m39 ReportButtons=0` → `m15 Enable(0)` →
`m19 SetParam(0)` → `m17 GetCoinType=0` → TsuNet created → boot.

Method semantics (first pass, from call sites + `TsuInputLib.dll` export names
TsuInputCoinsPerPlay/DeductPlay/SimulateCoin/SimulateMotionSwitch/...):

* **m11 = coin/credit poll** (`NumCoins`): start-play trigger at 0x4c368c is
  `m11(out) != 0` then `m12(out) != 0`, which resets the player struct at 0x9f7d0c
  (arcade "coin + start"). Edge-detected in the payload (`COIN_DOWN` static) —
  **verify** a single physical coin press still adds exactly one credit (an earlier
  run showed 0→4); also note `CREDITS` is effectively uncapped (`fetch_add(1).min(8)`
  caps only the logged value).
* **m12 = level state of cabinet button 0 (P1 START)**, *not* a 0..12 button index:
  the game pushes the constant 0 into the press/release handlers (0x4ca8d0 /
  0x4ca8f0, 0x14-byte entries at 0x9f7bd0). Buttons 1–8 are game-internal state;
  only button 0 is cabinet-fed.
* **m21 = GetJoyInfo** (per-frame refresh of the joy struct 0x9f746c; o1=0,
  o2=raw−1, o3=raw 0..255 centered 128, o4=0, o5=0x3e), **m28 = PulseWatchdog**
  (per-frame, watchdog timeout 60000 ms set at startup via m40).
* **m13 = StartPlay** (decrements a credit), m10 = credits>0, m8 = Detach,
  m7 = Attach, m19 = SetParam, m22 = SetJoyRange, m39 = ReportButtons (~50 ms,
  animates in attract).
* Float constants: 0x4e091c = 1/256, 0x4e0920 = −1/256, 0x4df3b0 = 0.0,
  0x4df3b8 = 0.5, 0x4df3c4 = 1.0, 0x4df45c = 0.25, 0x4df498 = 0.75 (centered-wheel
  thresholds).
* Slot-21 calls at 0x4b7fd9+ in the disasm are a DIFFERENT object
  (ds:0x9f73d0+0x24, takes (id, out)) — not TsuInput m21.

## TsuNet (the dump's real DLL, registered + patched — working)

Historical blocker, fixed: the game's TsuNet init (0x4c8d00) does
`CoCreateInstance(CLSID {9F17431A-FFA3-42A5-A8EB-F9033D3403AA}, ...)` →
`QueryInterface({360FE1FA-D991-4032-A65D-01500E07F05E})` → `vtbl[7](&wrapper+0x198)`
(stored in global 0x9f74b0). It *tolerates* a creation failure but then unconditionally
`call [NULL+0x30]` → AV → MSVCRT "Runtime Error!" modal (parent = the minimized game
window) → looked like an idle `GetMessage`.

`tsunet.dll` is a VC++ COM in-proc server ("TsuNet Class", ProgID `TsuNet.TsuNet.1`,
exports exactly the four COM server entry points; imports WS2_32, WINMM, KERNEL32,
USER32, ADVAPI32, ole32, OLEAUT32). `tsunet.rs` copies it next to the exe (profile
`files:`), writes `HKLM\...\CLSID\{9F17431A-...}\InprocServer32`
(`@=…tsunet.dll`, `ThreadingModel="Both"`) at payload load, and `patch_dll` fixes a
**Wine `GetAdaptersInfo` size bug**: tsunet's adapter walk has no bounds check, wine
reports a required size smaller than the data it writes (multi-interface host) → the
walk spun forever. The patch takes the first adapter and stops (network unused in
single-cabinet mode). The same CLSID appears in `tvtopcon.exe`.

## Keyboard (for liveness probes)

WM_KEYDOWN char handler at 0x4ba9fa (key=0x100 down / 0x101 up, char arg):
`'C'` → m23, `'9'` → m24, `'7'` → m25, `']'` toggles global 0x9f73f4. All call the
cabinet object (logged by the payload when the loop is alive). Synthetic X11 key
events (`XSendEvent`, tool at /tmp/opencode/xkey.c) did not reach the handler
when the main thread was in the old modal loop; XTEST segfaults on this setup.

## Game internals (reference)

* WndProc 0x458bd0, window class `"Revolt"`, HWND global `0x909adc`.
* Custom messages: `TSU_MESSAGE=0xc03c` (cabinet session), `TSU_OPCONS_MSG=0xc03d`
  (registered by tsunet.dll; **posting it to the game made it exit cleanly**),
  `TSU_SELECT_MSG=0xc03f`.
* Init chain: 0x4cba00 → TsuNet-init 0x4c8d00 (result global 0x9f7d1c);
  FindWindowA single-instance check; TSU_MESSAGE handler 0x4b19e0.
* tvtopcon.exe: launches games via CreateProcessA using launch.reg
  `launchGame`/`launchAttract`; strings `TSU_COIN_MSG`, COIN1/COIN2,
  LaunchProcessId, "FCabinet is not linked."

## Analysis notes

* Full disassembly: `objdump -d -M intel ReVolt.exe` (scratch copies in /tmp; the
  current one is /tmp/opencode/revolt-disasm.txt).
* Relay: redirect the launcher output (see Reproduce); analyze the tail (the game
  is the 00e0-ish thread; the mss32/D7VK threads are audio and render).
* The "spin" threads in old relays were Miles Sound (mss32.dll) audio/timer
  infrastructure, not the game loop.
* GUIDs: TsuInput CLSID {74B3FA08-50CC-4275-A5CF-8BBF6DDB75C2}; TsuNet CLSID
  {9F17431A-FFA3-42A5-A8EB-F9033D3403AA}; DPlay {D7B70EE0-…} → stubbed to 0x0.
  DI device GUIDs: `0x4dffe0` (device A) and `0x4dffd0` (device B) — dump the
  16 bytes at those VAs if you need the actual GUIDs.
