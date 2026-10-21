# Teratoma: The Last Rebellion

Works (user: 100%): boot, attract, coins and start, the guns (the mouse), the test menu.
Profile `systemprofiles/ringedge/teratoma.yaml`, dump `games/ringedge2/Teratoma` (`TT.exe`).

## The game

Skonec, 2014, a two-player lightgun game on the Operation G.H.O.S.T. engine: Sega amLib,
`gs2.ini`, Lua scripts with `lwGun` and `GameIO`, amDongle's game id `SBWA` (hard-coded: its
keychip challenge file is `.\SBWA_Table.dat`). It runs as a RingWide game (platform `AAM`) on
the RingEdge payload.

## Packed executable

`TT.exe` (SHA-256 `cb711483cfb1b64923bf5e966fa11e9a4745f7c930781f7d479bcb34f79365ba`) is
packed; started plainly it exits with code 15 in the unpacker. `wal-loader` with
`WAL_LOADER_TERATOMA=1` (`crates/loader/src/teratoma.rs`) checks the hash, skips the first
check, installs two pending VM operands, then, once unpacked, opens the game's native startup
gate and loads the payload from its startup call. The dump is not modified.

The unpacked code calls Windows through the packer's stubs, not its import table:
`WAL_PACKED_IMPORTS=kernel32.dll,setupapi.dll` makes the payload's hooks hot patch kernelbase's
functions (setupapi's have no hot patch prologue: the unpacked import table holds their
addresses, replaced). Hot patching user32, winmm or d3d9 the same way hung the game.

Two consequences in the payload: a hook must not log while holding a lock its own `WriteFile`
hook takes, and must not call `Path::exists` (it opens the file: `CreateFileW`, the hook again).

## Inputs

The dump's `gs2.ini` is in PC mode (`EnableJvs=0`, `EnableAMLib=0`, `EnableInputDevices=1`,
`GunController=0`); the profile replaces it with a copy turning amLib, JVS and
`GunController` on. Two JVS readers then run:

- amLib's own, on the JVS driver `\\.\mxjvs`;
- the game's (`GunController`): it lists the serial ports with `QueryDosDevice`, opens the
  first `COMn` (`\\.\COMn`, overlapped), resets and addresses the board, then polls
  `20 02 02 21 02 22 08 32 04 ...` (switches, coins, 8 analog channels, outputs). Its guns,
  start, service and test come from this reader only. The board is on every COM port
  (`WAL_TYPEX_JVS_PORT: mxjvs,COM*`): the first one Wine lists varies.

The reader thread waits on `WaitCommEvent` and arms its next read only after receiving bytes,
and sends its next poll from the reply handler, dropped while its previous write is not
complete: with `WAL_SERIAL_PENDING_READS=1` overlapped reads stay pending until the board's
reply, which comes 2 ms after the write's completion (else the game polls on its 200 ms timer:
a choppy crosshair).

The analog reply is read as 16-bit words; the guns' X axis is inverted (`-lx,ly`).

## Test menu

F2 (test) sets `root/TestMode.sac` and asks amMaster for the next application; the game does
not end: started again in place, amMaster fails (error 3005). The payload ends a game still
running 2 s after `mxmaster.foreground.next`, the launcher starts it again, and it boots into
the menu. Service (F1) moves, test (F2) selects; 退出 (Exit) clears the flag and starts the game
again. A game ended while in the menu boots into it next time.

## Protector watchdog

About a minute into a level (58.5 to 59.1 s after start, not in every run), the protector's
watchdog thread ends the game: one of its threads (start `0x2c049d3`) returns, its partner
sees it gone and calls `TerminateProcess` (return address `TT.exe+0x28162a6`). Large address
awareness does not change it. `WAL_IGNORE_TERMINATE_FROM=0x28162a6` ignores that termination
(`crash.rs`, the `NtTerminateProcess` system call stub); `WAL_TRACE_EXIT=1` logs who ends the
process.

## Movies

`scene/advertise/MOVIE_zh/logo.avi` and `scene/title/MOVIE_zh/TT_promo_movie02_SD.avi` are
WMV2/MP3 re-encodes (ffmpeg `Lavf61.7.100`); the in-game movies are WMV3/PCM. They play in the
shared prefix.
