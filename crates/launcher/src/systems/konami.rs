//! Konami e-Amusement PC games (Nobiteke! Byon Byon Daisakusen, Gashaaaan, Gashaaaan Refill!, Otomedius,
//! Winning Eleven 2008, Winning Eleven 2012, GTI Club: Supermini Festa!, Road Fighters 3D,
//! Contra: Evolution, Wartran Troopers, Chase Chase Jokers).
//!
//! The game drives its cabinet through `libacio.dll`, Konami's ACIO bus driver (HBHI I/O board:
//! switches, analog controllers, coins, security plugs; ICCA card reader): our payload replaces
//! it, so the game loads it through its own imports.
//!
//! The payload also answers the cabinet's network setup (`SetAdapterIpAddress`), runs the game
//! with e-Amusement offline (the boot's connection check takes its offline branch) and gives the
//! picture the screen's resolution in a borderless window.
//!
//! Gashaaaan Refill! has no ACIO: its profile installs the payload as `libbms.dll` (the
//! BM-System client), which answers the game's own I/O library, ig2ACLib (docs §6.3h).
//! Gashaaaan's profile installs it as `iob.dll` (its I/O board library), which also answers
//! its camera board library (ballio) and shows its two screens as one picture.
//! Otomedius's libacio.dll is the payload too (PJEI board), which also answers the game's libbms
//! and libtouchpanel imports; Winning Eleven 2008's too (PJEI board, PS2 pad port, touch panel).
//! GTI Club: Supermini Festa! also loads it as `dllibacio.dll`, its second ACIO bus (HGTH board:
//! wheel, pedals, the wheel's motor, its torque and vibration the force feedback).
//!
//! Contra: Evolution imports libacio's functions under their MSVC names and never calls them:
//! the payload answers its COM1 control panel (coins) and its DirectInput keyboard (the players'
//! controls, from the virtual sticks), and reports its operator clock set.
//!
//! Road Fighters 3D's code is a DLL (`jgt.dll`, the dump's `.windowsloader`): `wal-avs-boot.exe`
//! runs it as the cabinet's launcher does (AVS, e-Amusement boot, the DLL's entry points); its
//! `libacio.dll` and `libacioEx.dll` (the HGTH wheel board's bus) are the payload. Winning
//! Eleven 2012's too (`contents/modules/weac12_bootstrap_release.dll`, AVS 2.13: PJEI board, PS2
//! and PS3 pad ports, touch screen).
//!
//! Wartran Troopers has no ACIO: the payload is its `panicdialog.dll` and answers its serial
//! I/O chain (COM2: hub, boards, security plugs) and gun boards (COM1), its two monitors shown
//! side by side in one window.
//!
//! Chase Chase Jokers (Unity, 64-bit) drives its I/O board through Konami's AIO libraries: the
//! 64-bit payload (`wal_konami64.dll`) is installed as `libaio.dll`, `libaio-iob.dll` and
//! `libaio-iob2_video.dll` (its profile's `payload_install`), and answers its raw input
//! trackball and touch screen.
//!
//! Payload options (profile `env`):
//! - `WAL_KONAMI_CARD_ID`: the e-Amusement card's id, 16 hex digits
//! - `WAL_KONAMI_AVS_LOG=1`: the AVS log (the game system's boot, errors) in the payload log
//! - `WAL_KONAMI_CROSSHAIRS=1` (`_STYLE`, `_SIZE`, `_P1`..`_P4`, `_TINT`): the players' aim drawn
//!   over the picture (Gashaaaan, Gashaaaan Refill!)
//! - `WAL_KEEP_ASPECT_RATIO=0` (Gashaaaan): the picture stretched to the screen (`1`: aspect
//!   kept), Batocera ES's KEEP ASPECT RATIO
//! - Winning Eleven 2008: `WAL_KONAMI_GAME_LOG=1` the game's own log in the payload log;
//!   `WAL_KONAMI_EAMUSEMENT=1`, `WAL_KONAMI_LIVE_MONITOR=1` keep those test menu network options
//!   (forced off); `WAL_KONAMI_DDRIVE` the cabinet's D: folder (default the dump root)
//! - wal-avs-boot: `WAL_KONAMI_AVS_HEAP_MB` the AVS heaps' size (default 16), `WAL_KONAMI_DEST`
//!   the destination (region) in place of the dump's; Winning Eleven 2012: its region (`J`
//!   Japan, `A` Asia, `C` China) and the cabinet's product code
//! - GTI Club: `WAL_KONAMI_FFB` the force feedback strength, percent (default 100)
//! - Wartran Troopers: `WAL_KONAMI_PLUG` the game plug's region id (default `GKD00UA`)
//! - Chase Chase Jokers: `WAL_KONAMI_TRACKBALL_SPEED` the trackball's counts per 10 ms at the
//!   right stick's full tilt (default 12)
//!
//! Native inputs for `native_map`: the board's switches `test service start trigger button2`
//! and `coin` (players 1 and 2 have their own start, trigger and button2); Gashaaaan and Gashaaaan
//! Refill!: `test service coin start throw` (players 1-4); Otomedius: `test service coin start up down
//! left right a b c`; Winning Eleven 2008: also `d e f`, and the PS2 pad `pad_start select up down
//! left right triangle cross square circle l1 l2 l3 r1 r2 r3`; GTI Club: `test service coin start
//! action shift_up shift_down select_up select_down select_left select_right`; Contra: Evolution:
//! `start up down left right shot jump select test service coin` (players 1 and 2); Wartran
//! Troopers: `start trigger option shoulder sense offscreen` (players 1-4), `test service coin`;
//! Chase Chase Jokers: `test service coin dash skill attack jump slide headphone up down left
//! right touch`.

use super::{Payload, System};

pub struct Konami;

impl System for Konami {
    fn name(&self) -> &'static str {
        "konami"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_konami.dll", install_as: "libacio.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        // Chase Chase Jokers: its profile installs it as the AIO libraries (payload_install)
        Some(&[Payload { file: "wal_konami64.dll", install_as: "libaio-iob2_video.dll" }])
    }

    fn dll_host(&self) -> Option<&'static str> {
        Some("wal-avs-boot.exe")
    }
}
