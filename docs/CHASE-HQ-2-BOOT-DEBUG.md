# Chase H.Q. 2 (Type X2) — boot, input and video notes

Status: **works** (2026-10-09, user: wheel/pedal calibration, switches, Nancy videos).
Profile: `systemprofiles/typex/chase-hq-2.yaml`.

The first attempt (2026-10-05) was stuck on an invisible MessageBox; the causes below
replaced the guesses of that session.

## Network check (the boot blocker)

The cabinet link is DirectPlay 8 (`CLSID_DirectPlay8Peer`, TCP/IP service provider, port
13400). Even a single cabinet hosts a session at boot (cabinet ID 0, from its settings word:
`0x464440`, host path `0x4652f0` → `IDirectPlay8Peer::Host`, session "session"). Wine's
`dpnet` `Host` is a stub returning `DPNERR_GENERIC`: the game builds a new peer and retries
forever (NETWORK CHECKING, then NETWORK ERROR). Fix: `tricks: [directplay]` (Microsoft's
dpnet). The verb is isolated like `dsound`: native only for the games listing it.

Before it, `iphlpapi!AddIPAddress` (stub in Wine) tries to add the cabinet address
192.168.65.110-113 / 255.255.255.0; harmless.

## Inputs (JVS on COM2)

Polled every frame: `20 01 03` (1 player x 3 bytes), `32 03`, `33 06` (outputs), `21 01`,
`22 0A` (10 analog channels). Reply parser at `0x41f630`, state at `[0x170b558]`.

* Analog channels go to state `+0x8 + 2 * channel`. The calibration screens (and the game)
  read `+0xc` steering, `+0xe` accel, `+0x10` brake: **channels 2, 3, 4**
  (`WAL_TYPEX_JVS_ANALOG_INPUTS: ",,lx,accel,brake"`).
* The switch word is `system << 24 | p1 byte 1 << 16 | p1 byte 2 << 8 | p1 byte 3`. The
  game tests only: test `0x80000000`, service `0x400000`, SHIFT `0x200000` (up), SHIFT-NITRO
  `0x100000` (down), START `0x800` (button 7), PATO-NITRO `0x400` (button 8) — names from
  its switch test (`0x42b7bc`). The standard start switch (`0x800000`) is never read:
  `native_map` start → `btn7` (btn7/btn8 added to the JVS board's native names).
* Known patch `0x107E3:EB` skips the boot pedal calibration check.

## Display

The game creates an **800x600** fullscreen device and draws an 800x600 viewport. A forced
`WAL_D3D9_FULLSCREEN_SIZE: 1280x800` (first attempt, from a guessed 1280x768 mode) left the
picture in the top-left corner: no override, Wine scales its own 800x600 mode.
The back buffer format is A2R10G10B10 (35): the screenshot shim does not save it (regression
picture check "-").

## Nancy videos

`data\mv` holds WMV9 (WMV3) AVIs read through AVIFile. Microsoft's WMV9 VCM codec
(`WMV9VCM.dll`, extracted from the dump's `wmv9VCMsetup.exe` into the game folder),
registered by `WAL_VFW_CODECS: vidc.wmv3=WMV9VCM.dll` (as Gaia Attack 4).

## Environment notes

* Xwayland on this machine logged "Maximum number of clients reached" (256 slots in use,
  mostly by other applications) during the first runs. That is the likely source of the
  bogus ~5434188x5434103 desktop seen in the 2026-10-05 traces (and of
  `docs/GOUKETSUJI-INTRO-VIDEO-DEBUG.md`'s 5434182 texture width): Wine's X queries fail.
* Twice the game exited by itself (status 27) about 37 s in, after coin/start; two later
  regression runs and the user's sessions ran on. Not explained.
* `WAL_SERIAL_TRACE=<n>` logs the first n serial packets (default 40): how the inputs above
  were checked.
