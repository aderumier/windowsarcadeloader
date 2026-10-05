# KOF Maximum Impact Regulation A (Type X2) — intro video not played, debug notes

Status: **video SOLVED** (2026-10-05, user-confirmed: the opening movie plays).
**Open: intermittent crash at the movie end** (section 8).

Root cause: the movies are played by **CRI Sofdec through a DirectDraw7 YUY2
overlay**, not through D3D. Wine never reported `DDCAPS_OVERLAY` and never
drew overlays, so the player gave up (audio only). Fixed by a runner ddraw
patch (GE-Proton `patches/ge-video-rework/0075`, IMPLEMENTATION.md §5.3
`ddraw-overlay-emulation`) plus profile changes (`dxvk: false`, hide the
dump's Wine DLLs). Sections 1–6 are the original investigation; their
hypotheses (wined3d second device, MMX/wow64, engine decode) were wrong.

Reproduce:

```
cd windowsarcadeloader
timeout 240 env WAL_SCREENSHOT=5 dist/arcade-launcher run "games/typex2/King of Fighters Maximum Impact Regulation A"
```

Movie window: t≈25–105 s after boot (79 s film). Crash at the movie end
(t≈60–70 s in the observed runs).

Key traces (keep until solved):

| file | run | notes |
|---|---|---|
| `/tmp/opencode/mira-file.log` | 200 s, `WINEDEBUG=+file` (wined3d) | **the key one**: media thread reads the whole movie (78.6 MB to EOF), menu pak, then the page fault |
| `/tmp/opencode/mira-dxvk.log` | 240 s, plain (DXVK+d7vk) | D7VK v3.1 + DXVK d3d9 banners; clean Vulkan init; still exit 5 |
| `/tmp/opencode/mira-dshow.log` | 240 s, `WINEDEBUG=+quartz,+dmo,+mfplat` (wined3d) | zero quartz/dmo lines (no DirectShow); the wined3d 2nd-device GL failure; page fault |
| `/tmp/opencode/mira2.log`, `mira.log` | plain wined3d | user-watched runs; killed before the movie end (no crash observed) |

Parser for the file trace: `/tmp/opencode/parse-file-trace.py`.

## 1. What the movie is

`data/__gmdata/movie/kof_op.sfd` (77,266,944 B) is a **raw MPEG-1 program
stream** (no container; first bytes `00 00 01 BA`), ffprobe:

* video: mpeg1video 640x480
* audio: **adpcm_adx** (ADX), 432 kb/s — not MP2
* duration 79.07 s

Sibling movies: `staff.sfd` (104 MB), `bargein.sfd`/`bargein2.sfd`,
`gover.sfd`, plus `ign_logo.sfd`/`title.sfd` referenced in the binary. Same
`.sfd` family as Gouketsuji's engine movies; dev-path strings in the binary:
`d:/kof3d2ac/diskimg/_movie/*.sfd` (cf. Gouketsuji's `d:/shingou/...`).

## 2. Verified facts

* **The movie file is fully streamed.** Media thread `0174` opens
  `kof_op.sfd` (~t=25 s) and issues **912 sequential 128 KB reads** totalling
  **78,622,000 bytes** (to EOF, final 0x800-byte tail read) over ~35–40 s.
  The game consumes the entire film; the black video is a
  decode→render→present problem, not a file/streaming problem.
* **The video is never presented.** The 640x480 back buffer is pure black
  (0,0,0) for the whole movie phase (screenshots; note: ImageMagick reports a
  25 % "mean" for these frames — that is the unused 4th BMP byte read as
  alpha, not content). No second X window ever appears (only the main
  1920x1080 WM-fullscreen window) — no separate movie window.
* **Audio heard by the user is ambiguous.** The media thread also opens
  `diskimg/common/bgm_j.afs` (the game's BGM archive) at boot, before the
  movie. Whether the movie's ADX audio is decoded by the engine at all is
  unverified.
* **Crash at movie end.** After the EOF, the same thread opens
  `menu_ac_load_j.pak` (post-movie menu assets), then:
  `page fault on read access to 0x4 at 0x004021E9` (main thread, wow64
  32-bit; RVA 0x21E9 — near the very start of `.text`; a NULL+4 dereference),
  `launcher: game exited (exit status: 5)`. Seen in every traced run and in
  the DXVK run (exit 5 without a trace). The one long **untraced wined3d run
  survived past this point for 240 s** — the crash looks timing-dependent.
* **wined3d second-device GL failure (same signature as Gouketsuji).** At the
  movie start, after the boot device is destroyed (`wined3d_device_cleanup
  Context array not freed!`), a second wined3d device init fails:
  `err:d3d:wined3d_check_gl_call >>>>>>> GL_INVALID_VALUE (0x501) from
  Querying context profile @ ../src-wine/dlls/wined3d/adapter_gl.c / 3437` —
  the adapter caps query running with **no current GL context** (the caps
  context dies between `glGetString(GL_VERSION)` and
  `glGetIntegerv(GL_CONTEXT_PROFILE_MASK)` in `wined3d_adapter_init_gl_caps`;
  see GOUKETSUJI-INTRO-VIDEO-DEBUG.md). This is the movie's second d3d9
  device — the same bug class as Gouketsuji's VMR device.
* **DXVK/d7vk does not fix the video.** With `dxvk: true` +
  `graphics: d7vk` (launcher links runner DXVK into the prefix, override
  `d3d8,d3d9=...=n`): D7VK v3.1 loads at boot, Vulkan device init OK
  (RADV KRACKAN1, radv 26.2.3), DXVK d3d9 format table initialized, **no GL
  errors at all** — and the movie is still black, and the game still exits 5
  at the movie point. Conclusion: the black video does not depend on the
  wined3d second-device GL bug (that bug is real and worth fixing upstream,
  but it is not the whole story here).
* **The dump's d3d8.dll / d3d9.dll / ddraw.dll / wined3d.dll are not
  used.** `setup_d3d` always forces `d3d8,d3d9,...=b` (builtin) or the DXVK
  set, so the game-dir copies never load (the old profile comment "keep them,
  coherent Wine 10.16 set" was misleading).
* **Native resolution is 640x480.** The game requests a 640x480 fullscreen
  back buffer and lays its scene out for it. The old
  `WAL_D3D9_FULLSCREEN_SIZE: 1280x800` override put the whole scene in the
  top-left quarter of the buffer (small image, movie black) — **override
  removed**; Wine's desktop follows the 640x480 buffer and the WM stretches
  the fullscreen window to the monitor (user-acceptable).

## 3. Environment facts

* Profile: `systemprofiles/typex/king-of-fighters-maximum-impact-regulation-a.yaml`
  (patch `0x447C:B80008009090` for CRC 0x1782f027; hide MS
  dinput8; no back buffer override).
* Dump: `games/typex2/King of Fighters Maximum Impact Regulation A/` —
  `game.exe` (PE32, base 0x400000) imports d3d8, d3d9, ddraw, d3dx9_29,
  dinput8, dsound, ole32, winmm, msvcr80 (+ kernel32/user32/gdi32). No
  quartz import; no quartz/AVI strings in the binary (Gouketsuji plays its
  movie via dynamically loaded quartz; **no** quartz activity was observed
  for KOF MIRA before the crash, but that window is not conclusive).
* Content: `data/__gmdata/` (paks, `movie/*.sfd`), `diskimg/common/`
  (AFS archives, incl. `bgm_j.afs`), `WindowsLoader/`.
* Run dir: `wine-prefix/common/drive_c/wal/typex/king-of-fighters-maximum-impact-regulation-a/`.
* One launcher at a time (127.0.0.1:33700).

## 4. Hypotheses (unproven, in rough order)

1. **Engine-side video decode/present failure.** No codec DLL ships in the
   dump, so the engine likely decodes the MPEG itself (software). Frames are
   produced into a texture/surface that is never presented to the visible
   back buffer (or decoding yields 0 frames and the "movie" completes
   instantly at stream EOF — consistent with the ~40 s full-file stream, the
   menu load right after EOF, and the user hearing only BGM). The crash at
   the movie→menu transition suggests the engine's movie end-of-stream path
   touches an object that is NULL under Wine.
2. **wow64 + inline asm (MMX) in the software decoder.** The game runs as
   wow64 32-bit on the 64-bit loader; an MMX-based MPEG decode can misbehave
   under wow64 (FPU/MMX state handling). A 32-bit-loader run would test this.
3. **wined3d second-device GL bug (Gouketsuji's bug)** as a *contributor* on
   the wined3d stack: the movie's second device fails to initialize its GL
   context (adapter_gl.c:3437). Fixed in GE-Proton → re-test. (Does not
   explain the DXVK black video.)
4. Movie played via a mechanism not yet traced (e.g. runtime
   LoadLibrary("quartz") after the traced window, or a ddraw overlay) —
   `DXVK_DEBUG=api` / `ltrace` on d3d9/ddraw calls through the movie window
   would settle it.

## 5. Next steps (in order)

1. **Who draws the movie frames?** Run with `DXVK_DEBUG=api` (DXVK logs the
   d3d9 calls) or `ltrace -f -e 'Direct3DCreate9*'` through the movie window:
   which device/texture the engine renders, whether any draw/present of the
   movie frames happens at all.
2. **32-bit loader test** (`wow64: false` if the launcher allows for this
   game) to test the MMX/wow64 hypothesis.
3. Fix the wined3d caps-context/second-device GL bug in the GE tree
   (protonge, `dlls/wined3d/adapter_gl.c` — caps context not current at the
   profile query) and rebuild per IMPLEMENTATION.md §3; re-test Gouketsuji
   **and** KOF MIRA. Upstream candidate like #821–823.
4. Check whether the opening plays at all on real Windows.
5. If the engine decode is the blocker, look for a known PC-port behavior
   (movie skipped under certain conditions / `game.inf` flags).

## 6. What was changed

* `systemprofiles/typex/king-of-fighters-maximum-impact-regulation-a.yaml`:
  removed the stale `WAL_D3D9_FULLSCREEN_SIZE: 1280x800` (broke the 640x480
  layout); the DXVK/d7vk experiment was reverted (movie black on both
  stacks; DXVK run died at the movie point).

## 7. Resolution (2026-10-05)

Found by disassembly (`objdump -d -M intel game.exe`, search the YUY2 FourCC
`0x32595559`):

* `0x404420`: `IDirectDraw7::GetCaps`, gives up unless `dwCaps & DDCAPS_OVERLAY`.
* `0x404100`: `DirectDrawCreateEx`, `SetCooperativeLevel(NULL, DDSCL_NORMAL)`
  (**no window**).
* `0x404200`: primary + 640x480 YUY2 overlay flip chain
  (`DDSCAPS_OVERLAY|FLIP|COMPLEX|VIDEOMEMORY`, 2 back buffers), colour-fill
  the chain black; retried every second until it works.
* per frame `0x403f20`: `Lock` back buffer, Sofdec writes YUY2, `Unlock`;
  `0x403e30`: `Flip`. `0x404689`: `UpdateOverlay(src, primary, dst, DDOVER_SHOW)`.

Fix (ddraw, see the patch): report one overlay; draw the visible overlay over
its destination when the destination is presented, refresh it on overlay
`Flip`; with no clipper, bind ddraw's swapchain to the app window (the game
gives none: the topmost visible window of the process) while the overlay is
shown — otherwise a `DDSCL_NORMAL` primary goes to ddraw's hidden window.

Profile changes, both required:

* `dxvk: false`: with DXVK the game window already has a Vulkan swapchain;
  wined3d (GL) could not draw the overlay on it (`swapchain_blit_gdi Failed to
  blit`).
* hide the dump's `d3d8/d3d9/ddraw/wined3d.dll`: they are Wine 10.16 builtin
  DLLs; Wine loaded `d3d8.dll` and `wined3d.dll` **from the game dir** despite
  the `=b` overrides (`+loaddll`), and the runner's ddraw then bound to the
  dump's older wined3d.

## 8. Open: crash at the movie end (intermittent)

`page fault on read access to 0x4 at 0x004021E9`, main thread, exit 5. Not
caused by the overlay work (it predates it, on every renderer) and timing
dependent: a run with `WAL_SCREENSHOT=2` went through the movie into the
attract demo and the ranking screen without crashing.

Call chain (payload crash log): `0x422624` → `0x4021e0(camera = 0xc10308)`.
At `0x422608` the game destroys its RenderWare camera (`0x401eb0`), resets the
world (`0x420500`, `0x420440`) and re-creates the camera (`0x401e60` →
`0x401ed0`: `RwCameraCreate`, frame, camera raster) **without checking the
result**; when a step fails the camera pointer stays NULL and `0x4021e9`
dereferences it. Next step: find which step fails (likely the camera raster
— D3D8 device state right after the movie's ddraw object is released by the
movie thread, i.e. a race between the two threads).
