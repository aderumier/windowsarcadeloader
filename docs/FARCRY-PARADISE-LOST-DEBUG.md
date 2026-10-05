# Far Cry Paradise Lost: in-level debug notes

Status: boots, attract, test menu, coins, level select and the dongle work. **In a level the
game does not play**: after the intro fly-over the camera sits at sea level, enemies stand
still, shots and hits have no effect. Same symptom with another launcher under Wine; the dump
plays on Windows.

## Done

- **Dongle** (`payload-globalvr/src/hasp.rs`): HASP4 `hasp()` answered in memory. Memory:
  `PARLST1.0CTRYUS\0CAB1` then 0xff. Byte 0x0f is the cabinet type (`g_arcadecabinet`: '0' /
  0 Single, '2' Deluxe, '1' Standard). Read back from a working setup with
  `tools/dongle-recording/farcry-dongle-dump.py` (struct at `0xa5278c`).
- **Dongle checks**: even with that memory the boot and level checks show "IncorrectDongle":
  `0x5cbc57`, `0x5cbc68`, `0x5ceef0` jne -> jmp, `0x5cc4af` je -> nop (profile `WAL_PATCHES`).
- **PC mouse**: the input manager (`0x7af990`, only while the window has focus) updates the
  keyboard (`0x7abab0`), the mouse (`0x7ac3b0`, FPS mouse look in levels) and the USBIO guns
  (`0x7af480`). The mouse update is removed (`0x7af9a1`, 18 bytes nop). Forcing "no focus"
  instead (`0x403e9b` = 0) also stops the guns: crosshair frozen in levels.
- **PC keyboard** (`payload-globalvr/src/keyboard.rs`): the console game's controls and debug
  keys (WASD walk, G god mode, F1-F5 difficulty...) come from DirectInput (`0x7abaf3`) and
  window messages; only test / service / up / down reach the game.
- **USBIO version**: the game looks for the board over HID (VID 0x1ab7 or 0x04b4, PID 0x7013)
  for its firmware version; "n/a" raises `g_arcadeError` 0x400 every frame (controller test
  shows the trigger BAD). `0x7af4b6` je -> nop.

## Ruled out

Frame rate (60 fps cap), CPU (one core; no 3DNow!, no vendor or CPU count check), timer (QPC
deltas), `p_max_substeps`, missing files (strace), tutorial videos (Indeo 5 AVIs replaced:
same), Lua errors (`WAL_GLOBALVR_GAMELOG=1` hooks the script and Lua error paths: none),
swallowed exceptions (`WINEDEBUG=+seh`: none), XML (static expat).

## Leads

- Both players' HUD show "FREE PLAY" during the level: maybe no player joined (start not
  taken in the level?). Try start / trigger in the level, check `g_arcadePlayerState0`.
- `g_arcadeError` still 0x1800: per-player gun idle timeouts (`0x5c5e71`, `0x5c7f40`, input
  manager +0x388 / +0x38c timestamps).
- Second HID board (PID 0x7015 / 0x7012, string at input manager +0x394): outputs / force
  feedback.
