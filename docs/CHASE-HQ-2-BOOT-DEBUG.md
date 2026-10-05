# Chase H.Q. 2 (Type X2) — boot failure, debug notes

Status: **blocked** (as of 2026-10-05). The game boots, shows a Windows MessageBox
(within seconds, before any D3D9), stays in it for ~100–180 s, then exits on its own
(exit code 0). The window is **not visible to the user at all** — the game is
effectively off-screen. No gameplay is possible in the current state.

Reproduce:

```
cd windowsarcadeloader
timeout 120 env WAL_SCREENSHOT=5 dist/arcade-launcher run "games/typex2/Chase H.Q. 2"
```

Best traces (keep until solved):

| file | run | notes |
|---|---|---|
| `/tmp/opencode/chq2-win.log` | 45 s, `WINEDEBUG=+win` | **the key one**: all window creations, dialog contents, screen size |
| `/tmp/opencode/chq2b.log` | 90 s, `WINEDEBUG=+d3d9,+seh` | 914k lines; proves zero D3D9 calls; end-of-run stack-overflow loop |
| `/tmp/opencode/chq2.log`, `chq2c.log`, `chq2d.log` | plain | short boot logs (8 lines + "game exited (exit status: 0)") |

## Observed behavior (verified)

1. Launcher boots, payload `wal_typex.dll` connects (protocol v1), patch applied
   (unconditional by design, see below).
2. Within a few seconds the game process creates:
   - its main window: class **`RwAppClass`** (RenderWare/RAGE engine), title
     **`PJ_CHASE`**, at position **`-2147483648,-2147483648`** (INT_MIN,INT_MIN) with
     size **`806x5434701`** → completely off-screen, garbage size
     (chq2-win.log line 282).
   - a window class **`IPTip_Main_Window`**, caption **`Input`** (thread `0134`),
     which then expands to cover the whole "desktop"
     `(4,4)-(5434192,5434107)` (chq2-win.log line 341 + get_visible_rect after it).
     Unknown what "IPTip" is (Taito input tooling? RAGE?). Not resolved.
   - a standard **`#32770` MessageBox** (chq2-win.log lines 643–1050) with *all*
     standard buttons: `&Oui`, `&Non`, `A&bandonner`, `Ré&péter`, `OK`, `Annuler`,
     `&Ignorer`, `&R&eacute;essayer`, `&Continuer`, `Aide` (IDs 1–A = IDOK..IDHELP,
     i.e. every MB_* group at once).
     **The French captions are Wine's own fr-FR locale, not the game's** — the game
     calls `MessageBoxW` with a full button mask and Wine renders it.
     The dialog caption and message-text Static are set to **`L""`** (empty) in the
     trace (`set_window_text L""` on the dialog and on the 264x78 text static at
     line ~1100). Either the text is set later (not captured) or the box really is
     empty. **Nobody has seen the text: the dialog is off-screen/invisible to the
     user too** (user confirmed: "the game is not displayed").
3. The game then sits on the dialog: thread `0140` polls the dialog's buttons via
   oleacc/UIA (364× `find_class_data unhandled window class: #32770/Button` fixmes).
   **Zero D3D9 calls in 90 s** (`+d3d9` trace) → zero `WAL_SCREENSHOT` frames → the
   payload's d3d9 backbuffer hook never fires.
4. In the last few seconds (chq2b.log lines ~914498–914540) the main thread `00e0`
   enters a repeated `EXCEPTION_STACK_OVERFLOW (0xc00000fd)` → `RtlUnwindEx`
   loop (a small 32-bit thread stack, `0x1740000-0x1741000`, overflow at
   `wow64cpu.dll` dispatch). Whether this precedes or causes the exit is unknown.
5. The game exits **by itself** (run 1: `launcher: game exited (exit status: 0)`).
   No unhandled exception, no `winedbg` involvement.

Other startup facts from the traces:

- `fixme:iphlpapi:AddIPAddress :stub` on the main thread (LAN multiplayer setup;
  imports include `iphlpapi`, `WS2_32`).
- One thread raised `RPC_S_SERVER_UNAVAILABLE (0x6ba)` early — handled, not fatal.
- Game strings (RAGE engine error texts, `strings game.exe`):
  `Error initializing RenderWare.`, `GRAPHICS SYSTEM ERROR`, `?CALIBRATION FAILED`,
  `IO ERROR:`, `NETWORK ERROR`, `SOUND SYSTEM ERROR`, `CONNECTION FAILED` —
  any of these could be the dialog's real message.
- `game.exe` imports: WINMM, WS2_32, iphlpapi, DSOUND, **d3d9**, KERNEL32, USER32,
  GDI32, ADVAPI32, SHELL32, ole32, AVIFIL32. It **is** a D3D9 (RenderWare) game;
  it just never gets to create a D3D9 device.

## The 5.4-million-pixel "desktop" (main lead)

`WINEDEBUG=+win` shows Wine's user32 believing the X screen is:

```
0134:trace:win:get_min_max_info 1928 1040 / -4 44 / 1932 1092 / 5434188 5434103
```

(min-track / min-max / max-track / **screen = 5434188 x 5434103**, ~5.4M x 5.4M,
nearly square). All window math in the game is derived from that garbage:
INT_MIN position, 806x5434701 size, "Input" window covering 5434192x5434107.

Consequences that match the symptoms:

- The user sees **nothing** (windows placed off the real monitor).
- A game that sizes/positions from `SM_CXSCREEN/SM_CYSCREEN` behaves nonsense.
- Plausible chain: desktop metrics garbage → RAGE graphics/window init fails or
  misbehaves → MessageBox (probably `GRAPHICS SYSTEM ERROR` /
  `Error initializing RenderWare.`) → game gives up and exits 0.
- **User's own first guess was "could be resolution related" — this is it.**

Why the other Type X2 games still work on the *same* prefix/display: their D3D9
backbuffer is forced to 1280x800 by the payload hook (`WAL_D3D9_FULLSCREEN_SIZE`),
so they never depend on the desktop metrics. Chase HQ2's pre-D3D9 Win32 window
path does.

Contradiction worth noting: the `IPTip_Main_Window` WM_NORMAL_HINTS are
min 1928x1040 / max 1932x1092 — i.e. the game (or Taito's tool) thinks the display
is ~1920x1080. So the game has *some* sane display info, yet Wine's user32 reports
5.4M x 5.4M. Either the hints come from a fixed config, or the X size is only seen
through one API path.

### What is known about the display environment

- Desktop is **Wayland** (Mutter); the X server is **Xwayland** (`:0`, `-rootless
  -noreset ... -byteswappedclients`, pid 4143 at the time of debugging).
  `WAYLAND_DISPLAY=wayland-0`, `DISPLAY=:0` in the shell environment.
- Shell can talk to `:0` (`xprop -root` works; no XAUTHORITY trouble).
- `import -window root` (ImageMagick 7) fails with the misleading
  `missing an image filename` error — consistent with trying to allocate the
  5.4M x 5.4M root (~29 GB at 24 bpp) and failing.
- **The actual X root geometry was never read successfully.** Attempts (all
  failed/interrupted):
  - raw X11 protocol `GetGeometry` (opcode 40) over `/tmp/.X11-unix/X0` — first
    attempts died on short `recv()` (need a read-loop; the X11 *setup* reply does
    **not** contain screen dimensions, they come from GetGeometry on root);
    the fixed script was interrupted before it ran.
  - `python3` + `libX11` via ctypes — `XScreenNumberOfScreens` not exported by the
    distro libX11 (symbol moved); the `XGetGeometry` attempt segfaulted on a
    wrong `argtypes` (XID is 8 bytes on x86_64, I used `c_ulong`).
  - small C program + wine: blocked on producing a working wine import library
    for `gcc -m32` (winebuild `--implib` kept failing with "missing .spec file";
    the prebuilt `i386-windows/libuser32.a` doesn't resolve under gcc).
  - `xorg-x11-utils` (xdpyinfo/xwininfo) not installed; `pacman -Ss`/`-S`
    (CachyOS, Arch-based) **does not find `xorg-x11-utils` at all** — the
    package set here looks unusual; check `pacman -Ss x11` / repo status.
- Suggested next moves to get the truth:
  1. fix the raw-protocol script (read-loop for the setup, then GetGeometry on the
     root window ID at byte 40 of the setup reply);
  2. or ctypes with correct argtypes (`c_ulonglong` for XID/Window);
  3. or check the Wayland side: Xwayland is `-rootless`, so its size is a Wayland
     surface size — `wayland-info` / Mutter dconf monitor settings
     (`gsettings get org.gnome.monitor ...`) to see if Mutter computed a broken
     output size (e.g. a scaling bug) and passed it to Xwayland.

## Ruled out / verified

- **The pedal-calibration patch IS applied.** `WAL_PATCHES` is written unconditionally by
  the payload (`crates/payload-common/src/patches.rs`, no CRC gate); dump bytes at
  RVA/file offset `0x107E3` are `75 12 6a 01` (JZ rel8) and the patch overwrites
  the `75` with `EB` (unconditional JMP — "skip pedal calibration", per the
  profile comment). So calibration skip is in effect and still the game fails.
- Not a .NET/mono issue: `game.exe` does not import mscoree. (The
  `"Exited with code %d"` string does exist in wine-mono's libmono, but see below.)
- Not d3d8/ddraw: the `d3d8` grep hits in the trace were register values in SEH
  dumps; the game imports d3d9 only.
- Not a missing-file problem in the dump: `data/` (fonts, models, mv, path, sh,
  sound, textures), `dinput8.dll` (Taito bridge, kept — no override),
  `chq2.ini` (DEST_COUNTRY=0 UK), `game.inf` all present; the game creates its
  `WindowsLoader/` dir (ranking0/1.bin, rtb.dat, rtc.dat) at runtime, so it got
  past filesystem init.

## Red herring solved (so nobody re-chases it)

The mysterious `Exited with code N` lines that appeared in *shell tool output*
during this debug session are **the harness reporting the non-zero exit code of
the shell command itself** (correlated exactly: `ls` on missing shots → 2,
`grep -c` with 0 matches → 1, python crash → 139, clean `sleep` → nothing).
Unrelated to wine, the game, or mono.

## Open questions / next steps

1. **Read the real X root geometry** (section above). If X says 5.4M x 5.4M, the
   bug is in Xwayland/Mutter (report upstream or work around). If X says a sane
   size (e.g. 2560x1440), the bug is in Wine's X11 driver size query for this
   Xwayland version — different fix.
2. **Workaround candidates** (test once the cause is known):
   - force a sane virtual desktop for this game (Wine `user.reg`
     `HKCU\Control Panel\Desktop` / wine's `desktop` options), if that changes
     what `SM_CXSCREEN` reports;
   - a payload hook forcing `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)` (the
     payload already hooks user32-adjacent surfaces; this would be a
     `WAL_DESKTOP_SIZE=w x h`-style knob, like `WAL_D3D9_FULLSCREEN_SIZE`);
   - fix the Xwayland/Mutter size if that's the source.
3. **Read the dialog text** once the window is visible (or hook `MessageBoxW` in
   the payload and log its `lpText` — cheap and decisive).
4. Figure out what `IPTip_Main_Window` ("Input") is and whether it is a Taito
   helper process the game spawns (multiple `loader_init` events in the trace
   hint at helper processes: `002c`, `0024`, `00e0`).
5. Minor: after run 1 the launcher process stayed a few seconds after logging
   `game exited (exit status: 0)` (wineserver drain?); it was killed manually.
   Not confirmed to be a bug — keep an eye on clean shutdown for this game.

## Environment quick facts (for the next session)

- Profile: `systemprofiles/typex/chase-hq-2.yaml` (patch `0x107E3:EB`,
  `WAL_D3D9_FULLSCREEN_SIZE: 1280x800`, Taito `dinput8.dll` kept).
- Dump: `games/typex2/Chase H.Q. 2/` (1.9 MB PE32 `game.exe`, image base 0x400000,
  `.text` VMA 0x401000 / file 0x1000, so RVA == file offset for .text; CRC
  0x1f2f9497 raw / per earlier analysis).
- Run dir (screenshots land here, none ever did):
  `wine-prefix/common/drive_c/wal/typex/chase-hq-2/`.
- Locale: the Wine prefix is **fr-FR** (French standard button captions).
- One launcher at a time: 127.0.0.1:33700.
- Chase HQ2 also needs analog wheel/gas/brake from JVS which the payload does not
  emulate — even once boot is fixed, **driving will not be possible**; boot +
  menu is the realistic target (same situation as Gaia Attack 4).
