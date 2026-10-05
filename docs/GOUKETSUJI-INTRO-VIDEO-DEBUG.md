# Gouketsuji Ichizoku (Type X2) — intro video not played, debug notes

Status: **SOLVED** (2026-10-05) — the opening movie plays (user-confirmed).
Two quartz bugs, both fixed in a rebuilt runner `quartz.dll` (GE-Proton tree,
`patches/ge-video-rework/0073`, `0074`; see "Resolution" at the end and
IMPLEMENTATION.md §5.3 `quartz-vmr7-rgb24-getavailable`):

1. **VMR7 refused RGB24.** The VMR7's default presenter only allocates
   surfaces at the primary's depth (32 bpp), so the RGB24 connection
   Grabber → Video Renderer failed with `E_FAIL` (0x80004005) and the graph
   dropped the renderer. The GL_INVALID_VALUE at `adapter_gl.c:3437` below is
   **noise**, not the cause.
2. **`IMediaSeeking::GetAvailable` was a stub** (S_OK, outputs untouched).
   The game calls it and seeks to "earliest": it got stack garbage
   (~450 million s — the bytes of the MEDIATYPE_Video GUID), the streams
   started past their end and the movie ended at once.

The analysis below is the original investigation, kept for reference; its
conclusions about the wined3d second device were wrong.

Reproduce:

```
cd windowsarcadeloader
timeout 120 env WAL_SCREENSHOT=5 dist/arcade-launcher run "games/typex2/Gouketsuji Ichizoku - Matsuri Senzo Kuyou"
```

Movie attempt window: t≈55–70 s. Without a coin the attract rotates
title (static ~20–30 s) → animated title crest fade → demo match → village
character scene → …; the `OPENING` state is one slot of that cycle. Note the
game also **self-exits (exit 0)** after a random attract duration (observed
~15 s, ~110 s, sometimes still alive at 180 s) — normal for this port, not a
crash.

Key traces (keep until solved):

| file | run | notes |
|---|---|---|
| `/tmp/opencode/gk-dshow.log` | 100 s, `WINEDEBUG=+quartz,+dmo,+mfplat` | **the key one**: full DirectShow graph build + the exact failure |
| `/tmp/opencode/gk-both.log` | 150 s, `WINEDEBUG=+file` | AVI open/reads/cancel on thread `013c`; ~950k lines, movie phase at lines ~946456–947140 |
| `/tmp/opencode/gk-libgl.log` | killed early, `LIBGL_DEBUG=verbose MESA_LOADER_DEBUG=all DRI3_DEBUG=1` | DRI3/driver init: two Mesa driver inits (fds 15 and 49 = game device, then VMR device) |
| `/tmp/opencode/gk-origq.log` | 100 s, prefix quartz swapped to the **original GE** `quartz.dll.orig` | A/B test: movie still not played (see "Ruled out") |

## 1. The "AVI" is a raw MPEG-1 system stream

`Data/Movie/Opening.avi` (38,078,468 bytes) is **not an AVI**. First bytes:
`00 00 01 BA` (MPEG-1 SEQUENCE_HEADER). ffprobe demuxes it as `mpeg`:
mpeg1video 640x480 @ 30 fps, 8000 kb/s + mp2 44.1 kHz stereo 384 kb/s,
35.48 s, total 8585 kb/s. The extension is a lie; there is no RIFF header.
(Relevant to any "install a codec" theory: there is nothing to demux.)

## 2. How the game plays it

`game.exe` imports **no** codec/AV/quartz DLL (KERNEL32, USER32, GDI32, d3d9,
d3dx9_37, DSOUND, DINPUT8, WINMM, ole32 only). The movie thread (`013c`)
**`LoadLibrary("quartz.dll")` dynamically** (the string is not in the binary —
the game decrypts/constructs its data strings at runtime; the AVI path
`data\movie\opening.avi` is at abs `0x00C32DB8`, referenced from code at file
`0x2448D` as `push 0; push 0x832DB8; push eax; call [ecx+0x34]` — a
load-by-RVA virtual call, NULL result = skip). It then builds this graph
(from `gk-dshow.log`, all on `013c`):

```
FilterGraph (CoCreateInstance, IID_IMediaControl)
+ SampleGrabber (CLSID_SampleGrabber cda42200-...) added manually first
RenderFile(L"data\movie\opening.avi")
  File Source (Async) "Reader"  (e436ebb3-...)  output = MEDIASUBTYPE_MPEG1System
  -> MPEG-I Stream Splitter
       -> Video: MPEG Video Decoder (winedmo, GStreamer mpeg1video)
                  output negotiated = MEDIASUBTYPE_RGB24 640x480  (decoding WORKS)
                -> Grabber (SampleGrabber) -> "Video Renderer" (VMR) Input0   <-- FAILS HERE
       -> Audio: MPEG Audio Decoder (winedmo) -> ACM Wrapper -> Default DirectSound Device
```

The SampleGrabber between decoder and VMR is how the game receives decoded
frames (same pattern KOF XII uses). GStreamer successfully probes the stream
(`stream 0: ... duration: 35.483333`) and negotiates RGB24 640x480, so the
decode side is fine.

## 3. Where it fails (the bug)

`gk-dshow.log`, movie phase:

```
filter_config_SetRenderingMode filter 05A2A978, mode 1.          (VMR windowless)
vmr7_presenter_create
  err:d3d:wined3d_check_gl_call >>>>>>> GL_INVALID_VALUE (0x501)
        from Querying context profile @ ../src-wine/dlls/wined3d/adapter_gl.c / 3437.
allocate_surfaces Initializing in mode 1, our window 00040078
warn:quartz:initialize_device Failed to allocate surface, hr 0x80004005.
BaseOutputPinImpl_AttemptConnection peer ReceiveConnection returned 0x80004005.
...
err:d3d:wined3d_device_cleanup Context array not freed!
FilterGraph2_RemoveFilter Removing filter L"Video Renderer".
```

Chain: the last graph connection (Grabber.Output → Video Renderer/VMR, RGB24
640x480) makes the VMR presenter create a **second wined3d device on the game
window**. That device's **GL context is not current** when wined3d initializes
the adapter (`glGetIntegerv(GL_CONTEXT_PROFILE_MASK)` → GL_INVALID_VALUE,
i.e. no current context at `adapter_gl.c:3437`), so surface allocation fails
(`0x80004005` = E_FAIL), the connection is rejected, and the graph removes the
Video Renderer. The game then lets the (now unrendered) decoder run briefly —
`[mpeg1video] Invalid frame dimensions 0x0`, `transform_drain failed, status
0xc0000001` — then tears down the whole graph (all filters removed) and the
movie is cancelled. Attract resumes.

Secondary errors on the broken VMR context (thread `01a0`), symptom not cause:

```
err:d3d:wined3d_debug_callback 022475A0: "GL_INVALID_FRAMEBUFFER_OPERATION in glClear(incomplete framebuffer)".
err:d3d:wined3d_debug_callback 022475A0: "GL_INVALID_VALUE in glTexImage2D(invalid width=5434182 or height=2 or depth=1)".
```

**width=5434182 is the bogus X desktop size territory** (the
5434188x5434103 Xwayland garbage from `docs/CHASE-HQ-2-BOOT-DEBUG.md`). The
game's own device renders fine (attract + screenshots work); only this second
context is broken.

Environment facts for the failure:

- Backend is **DRI3** ("Using DRI3 for screen 0", Mesa radeonsi, AMD Radeon
  860M). The VMR device opens a **second DRI3 connection** (`using driver
  amdgpu for 49`) — that connection itself succeeds; the failure is later
  (context/surface level).
- Wine's GL path here is classic GLX calls on top of Mesa GLX→DRI3:
  `pglXCreateContextAttribsARB(display, fbconfig, share=NULL, direct, attribs)`
  (`winex11.drv/opengl.c x11drv_context_create` → `create_glxcontext`),
  surfaces via `pglXCreateWindow`, make-current via
  `pglXMakeContextCurrent`. No "Failed to create a WGL context" /
  "Context creation failed" ERR appeared, so the context object was created
  and the failure is at make-current/surface level (unproven which call).
- GE-Proton production builds have the `d3d9`/`wined3d` **debug traces
  stripped** (no trace format strings in the DLLs) — `WINEDEBUG=+d3d9,+d3d`
  is silent by design, not evidence of inactivity. `+quartz`/`+dmo`/`+mfplat`
  DO have trace points (the winedmo/quartz DLLs keep them); Mesa-side data
  comes from `LIBGL_DEBUG=verbose`.

## 4. Ruled out

- **Our quartz rebuild is not the cause.** A/B test: swapped the prefix
  `syswow64/quartz.dll` back to the original GE `quartz.dll.orig` and reran —
  the movie still did not play (same failure class; attract scene variety
  differs run to run, that is not the signal). The rebuilt quartz (GE-patched
  sources + #823, needed for KOF XII) was restored after the test.
- **Not a missing codec / VFW / ICDecompress path**: no such imports, the
  "AVI" is a raw MPEG-1 system stream, and GStreamer (winedmo) decodes it
  fine up to the render stage.
- **Not the game's own d3d9 device**: the game renders attract correctly on
  its first device throughout; only the second (VMR) context fails.
- **Not locale**: `LC_ALL=C` makes it worse (game self-exited at ~30 s vs
  ~110 s+ in fr-FR runs). Do not force `LC_ALL=C` for this game. (General
  note: `objdump`/tool output is French on this box; use `LC_ALL=C` for
  tooling only.)

## 5. Code/data facts (so nobody re-digs)

- `OPENING` is a game **state name**, abs `0x00C2007C`, in a string table
  with `SELECT`, `DEMO_TANO/IKARI/AI/OMOI`, `WINNER`, `ENDING`, `STAFFROLL`, …
  The attract cycle enters it ~55–60 s after boot.
- The AVI is opened by thread `013c` ~4× (header probe: 16-byte + 4-byte
  reads), then pre-read ~6.6 MB (25×256 KB + 67,588 B ≈ 6 s of stream) by
  `01b0`, then `NtCancelIoFileEx` + `title.vsa` — consistent with the graph
  teardown above.
- Engine movie strings point at a dev disk: `d:/shingou/diskimg/_movie/*.sfd`
  (`op.sfd` = opening cutscene) — not in the dump, not used at runtime
  (`D:\` is redirected to the run dir's `WindowsLoader/`).
- `debug.ini` (UTF-16LE, JP): デバッグメニュー=0, 当たり判定表示=0,
  隠しキャラの開放=1, ウィンドウタイプ=1, システムエラーチェック=1,
  ロケテスト=0, コインクレジット表示=1. A comment in the file says a normal
  PC needs システムエラーチェック=0 — **untested** whether that affects the
  movie (the file is read 7× at boot, one GetPrivateProfileString per key).
- File atime is useless here (not updated on read); use `WINEDEBUG=+file`.

## 6. Open questions / next steps (in order)

1. **Identify the failing GLX call.** `ltrace -f -e 'glX*'` on the run
   (32-bit wine process) through the movie window, or X-protocol logging;
   check whether `pglXMakeContextCurrent` (or `pglXCreateWindow`) for the VMR
   context returns False/produces a BadMatch. Also a minimal standalone GLX
   test on this display: one window, two contexts (2nd shared, matching
   attributes wined3d requests: core profile, GL version from
   `gl_info->selected_gl_version`), make each current.
2. **The bogus X desktop (5434188x5434103) is the prime environmental
   suspect** — same root as the CHQ2 blocker (that session is user-driven,
   see `docs/CHASE-HQ-2-BOOT-DEBUG.md`). Once the Xwayland size is fixed,
   rerun and re-check the VMR path; the `5434182` texture width strongly
   suggests the same garbage is involved.
3. Check whether the movie plays at all on real Windows — web
   search was inconclusive. If it never played on the PC port, the
   "skip movie, keep attract" behavior we see may be the game's own fallback
   for a failed movie, and the target becomes "make the movie renderable",
   not "restore arcade behavior".
4. If the failure is a wined3d/VMR bug (likely), fix belongs in the GE tree
   (`~/code/protonge/wine`, `dlls/wined3d` or `dlls/quartz/vmr7_presenter.c`)
   and can go upstream as a GloriousEggroll/proton-ge-custom PR, like
   `quartz-find-filter` (#823). Build rule: **GE-Proton tree only, never
   plain wine** (see IMPLEMENTATION.md §3).
5. Cheap untested knob: `debug.ini` システムエラーチェック=0.

## 7. Environment quick facts

- Profile: `systemprofiles/typex/gouketsuji-ichizoku-typex2.yaml` (8 lines, no
  backbuffer override — the game runs fine at its native 640x480).
- Dump: `games/typex2/Gouketsuji Ichizoku - Matsuri Senzo Kuyou/` — 5 MB PE32
  `game.exe`, base 0x400000 (`.text` file 0x1000 / VMA 0x401000, `.rdata`
  0x3ef000 / 0x7ef000, `.data` 0x443000 / 0x843000); CRC 0x777df862 (=
  "Power Instinct V"); dump ships its own `D3DX9_37.dll` (Taito)
  and `debug.ini`.
- Run dir: `wine-prefix/common/drive_c/wal/typex/gouketsuji-ichizoku-typex2/`
  (`wal-typex.log` = live payload log; `shot-*.bmp`).
- Screenshot cadence is ~8–10 s in practice with `WAL_SCREENSHOT=5`.
- One launcher at a time (127.0.0.1:33700); prefix locale fr-FR.
- `timeout`+wine: if the game hangs past the timeout, wineserver can swallow
  SIGTERM and keep the port bound — kill the `arcade-launcher` pid (then
  wineserver goes too) instead of waiting.

## 8. Resolution (2026-10-05)

`WINEDEBUG=+quartz` after fix 1 showed the whole graph connecting
(`allocate_surfaces Initializing in mode 1` then `ReceiveConnection returned 0`),
followed by:

```
fixme:quartz:MediaSeeking_GetAvailable (...)->(0314FE78, 0314FE80): stub !!!
MediaSeeking_SetPositions ... current 10000073646976 ... stop 719b3800aa000080
```

`0x...73646976` = `'vids'`, `719b3800aa000080` = the tail of the
MEDIATYPE_Video GUID: uninitialized stack. With fix 2 the game seeks to
0 → 35.5 s (`stop 152653b5`), runs the graph and ~2100 frames go through the
VMR until the end of the movie.

Fixes (GE-Proton tree `~/code/protonge`, branch for upstream, see
IMPLEMENTATION.md §5.3):

* `dlls/quartz/vmr7.c`: when the presenter refuses an RGB24 surface, retry
  with RGB32 and expand the pixels in `vmr_render()` (Windows' VMR7 accepts
  RGB24 input while its default presenter refuses RGB24 surfaces, as the
  existing Wine tests show).
* `dlls/quartz/filtergraph.c`: `GetAvailable` returns earliest 0, latest the
  graph duration (`E_NOTIMPL`, as on Windows, when no filter can seek).
