# Initial D Arcade Stage 6 AA (RingEdge 2, SBUU) — findings and status

Dump: `games/ringedge2/Initial D Arcade Stage 6 Double Ace` (exe `id6_dump_.exe`, base 0x400000).
Profile: `systemprofiles/ringedge/initial-d-arcade-stage-6.yaml`. Payload: `crates/payload-ringedge`.

## Status (2026-10-17)

| Part | State | Commit |
|---|---|---|
| Boot (keychip, PCP incl. `mxnetwork` 40104, JVS on COM4) | works | 33c979d |
| Display on AMD/Mesa | works ("display is perfect") | 33c979d |
| Display on NVIDIA | **not tested** (fix is off there: original shaders) | — |
| Wheel, pedals, shifter, view, menus | works ("all inputs working") | a199804 |
| Keyboard ramp (0.5 s) | in profile, not confirmed | a199804 |
| Force feedback (drive board COM1) | outputs sent, **not felt on a real wheel** | 49b98e4 |
| Card reader | WIP, **off in profile**: unit detected, cards rejected (RFID chip) | 06c9393 |
| Deployed to Batocera | yes (rgs de664b9a2) | — |

## Display on non-NVIDIA GPUs (`cg.rs`, `nvasm.rs`, `WAL_RINGEDGE_CG_FIX: auto`)

`auto` = on when `/sys/class/drm/*/device/vendor` has no 0x10de. Everything below was needed:

1. **cgGL profiles**: the game only takes its NV40 rendering path when cgGL says vp40/fp40
   are supported. cgGL registers each profile with `test eax,eax; je; push edi; mov edi,<profile>`:
   the `je` of 7001 (vp40) and 6151 (fp40) is inverted (pattern search, cgGL+0x2045 / +0x2144).
2. **cgCreateProgram**: fp30/fp40 compiled as arbfp1, vp30/vp40 as arbvp1, `-po MaxLocalParams=256`
   (bone palettes), `BLENDWEIGHT` → `ATTR1`, `mtxWorldViewProj` & co bound to `C0`.
   `cgGetProgramString` (fragment): `ARB_fragment_program_shadow` / `SHADOW2D` removed.
3. **Shader caches** (`data/V5SHADER/*.asm_gl`, NV assembly, some with no source e.g. `pcarbonDSF`)
   and precompiled `shader/Cg/*/*.asm_gl`: `kernelbase!CreateFileW` hot-patched (every reader),
   NV assembly translated to ARB by `nvasm.rs` (310/381 translate; the rest are dated stale via
   `GetFileAttributesExW` so the game recompiles them). Fixed copies in `cgfix\` of the run dir,
   same size as the original (the game reads with the original size).
4. **Black screen cause**: the game writes parameters with `glProgramEnvParameters4fvEXT`, the
   programs read `program.local`: each env write is mirrored to the bound program's locals.
5. **White 3D cause**: the game feeds `glVertexAttribPointer` 0/2/3/8 (NVIDIA aliases them to
   position/normal/color/texcoord0); mirrored to `glNormalPointer`/`glColorPointer`/`glTexCoordPointer`.
   Do NOT rewrite shader inputs to `ATTRn` (breaks the immediate-mode 2D: glColor/glTexCoord).
- Debug: `WAL_RINGEDGE_GL_SHOT=<s>` (glReadPixels shots `glshot-NNNN.bmp` + `glprog\` program dumps),
  `WAL_RINGEDGE_CG_LOG=1`, `WAL_RINGEDGE_CG_DUMP=1`.
- References used: Lindbergh loader `src/loader/graphics/shaderWork/id.c` (ID4/5 Mesa fixes).

## Inputs (`jvs_board.rs`)

- Analog: ch0 steering `lx*0.35` (cabinet sensor covers ~1/3 of the channel), ch1 gas, ch2 brake.
- Switches: view = P1 btn1, menus = P1 directions, **shift up/down = P2 up/down**
  (new natives `p2up`, `p2down`, `p2btn1`…). Gears 1-6 (P2 btn1-6) not mapped.

## Force feedback (`ffb.rs`, `WAL_RINGEDGE_FFB: COM1`)

- Packets `cmd p1 p2 ((cmd^p1^p2)&0x7F)`; board answers 1 status byte: `00` ready,
  `11` not initialized (after `81`), `44` busy (first `FC`, wheel bounds test).
- `84 dir n`: constant (dir 0 = right: (0x80-n)/127; dir 1 = left: n/127), `85 period n`:
  vibration n/127, `86 x n`: spring n/127, `80 00 01`: full spring.
- To verify on the wheel: **direction** (may need inverting), strength, boot errors.
- Not done: the wheel bounds test motion feedback (Lindbergh moves the steering value while
  `84` runs before init) — only needed if the game reports a steering error.

## Card reader (`card.rs`, `WAL_RINGEDGE_CARD_DAEMON: picodaemon.exe`, commented out)

Architecture (from the dump's `game.bat`): `picodaemon.exe` is started before the game, drives
the magnetic card reader on **COM2** and shares state with the game through shared memory
`zmShare%08x` (game side: 0xbd9xxx, "RPS1/2/3" status). The game never opens a COM port.

Done:
- The game's payload starts `wal-loader wal_ringedge.dll picodaemon.exe` (job object: dies with
  the game); in picodaemon the payload is a helper (`start_helper`, no launcher connection).
- Sanwa **CRP-1231BR** reader emulated (protocol reference: YACardEmu, GPL, not copied):
  `02 len cmd 00 00 00 params 03 xor` → `06`; `05` poll → `02 len cmd pos err state data 03 xor`.
  BR position byte: shutter bit 7/6, dispenser bit 5, `00/01/18/07/1C`. Commands 10, 20, 33, 35,
  40, 53, 78, 7A-7E, 80, A0, B0 (dispense), C0, C1, D0 (shutter), E1, F0, F1, F5 handled.
- Test menu: unit detected.
- `D1 <sub...>` = RFID passthrough (picodaemon handler 0x401340): reply data[0] is a status,
  `0B` = ok; no data → picodaemon error 0x87 (game 5102). `D2 <30|31>`: setting, needs "done".
  Unknown commands must be accepted (an "illegal command" state = error).

Missing — **the card's RFID chip** (game errors 5104 "unknown card" in test, 5110 in game):
- Player data lives in the chip, not the 207-byte stripe (card editor: version at 0x50, name at
  240, car at 256/271, level 164, gender 90; reference card file = 0x3C-byte header + `.crd`).
- picodaemon strings: `_rfid_req`, `req2 error`, `sel2 error` (ISO 14443-A cascade level 2 →
  7-byte UID, MIFARE Ultralight family), `carrier-on error`, `_rfid_adjust`, key
  `B3E8CFA7D2149560` (3DES → Ultralight C?).
- Observed `D1` sequence on insert: `48`, `F7`, `D2 31`, `28`, `4D 00×8`, `2D 00 00 00 00 F5 61 1A EB`,
  `0D 00 00`, `ED`, `8D 00 08`, `CD 00×8`, `8D 00 09`, `38`… (reader RF module sub-commands).
- picodaemon sites: sub-command bytes built at 0x402b2d (4D), 0x402313 (2D), 0x402427/0x403a7b (8D),
  0x4023b8/0x403b18 (CD), 0x402458/0x403a34 (ED), 0x4024d8/0x402bff (28), 0x401663/0x4024f8 (38),
  0x402518 (F7), 0x402538 (48); D1 reply parsing 0x4013c7 (switch on data[0]-0x0A: 0x1A/2A/3A/4A/5A/CB
  errors 0x88-0x8D, 0x4B → +0x28, 0x8B → +0x36); D2 at 0x402200; status check 0x401230.

Next steps:
1. RE picodaemon's RFID layer: what each sub-command means and the reply data it expects
   (UID, page reads, auth challenge/response).
2. Emulate the tag: UID, auth with the key, pages stored in the card file.
3. GUI game option "new card" (delete/replace `sv/card.bin`); card file format decided then.
4. Enable `WAL_RINGEDGE_CARD_DAEMON` in the profile.
- The game's built-in file-card code (`cFileCard`, `InidCrdNNN.crd`, strings at 0x9ea3dc..) looks
  unreferenced in this build (only `cCardIF` / `cRealCardIF` vtables at 0xdf62f4 / 0xdf640c;
  cRealCardIF ctor 0x662e40 called from 0x6644d0). Lindbergh ID4/5 switch to it with a ctor-call
  patch + `cFileCardIF::emulation()+0x21` + `checkTrgOn(port,0x1000)` detour; no equivalent found here.

## Known issue: card stocker cleaning prompt (Stage 6 and 7, 2026-10-18)

With the card reader on, every boot shows "card stocker cleaning needed" (remove the IC cards
from the stocker, set the cleaning card and weight, START + TEST). VIEW CHANGE dismisses it;
cards, races and saves work. Neither the START + TEST cleaning (`A0`, emulated: a cleaning card
goes in and out) nor the test menu's CLEAN STOCKER (slow but completes: `A1 30`, `A0`, a test
card dispensed `B0` and ejected) clears it. No reader command precedes it at boot, so the game
decides from saved state. Suspects: the stocker check `A1 30` (picodaemon 0x402040, game
command 0x1D), answered with no data, or a backup-memory counter only reset when that check
reports an empty stocker. To do: find the game code showing the prompt and what it reads.

## Other notes

- Card screen shows "closing time soon, card play unavailable" (shop close setting = 24h); also
  "network error 8301" at boot (offline single cabinet, harmless).
- Boot takes ~60 s ("Initializing…").
- No screen capture of OpenGL games under Wayland: use `WAL_RINGEDGE_GL_SHOT`.
