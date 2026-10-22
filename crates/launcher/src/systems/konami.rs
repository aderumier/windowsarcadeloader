//! Konami e-Amusement PC games (Nobiteke! Byon Byon Daisakusen, Gashaaaan Refill!, Otomedius).
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
//! Otomedius's libacio.dll is the payload too (PJEI board), which also answers the game's libbms
//! and libtouchpanel imports.
//!
//! Payload options (profile `env`):
//! - `WAL_KONAMI_CARD_ID`: the e-Amusement card's id, 16 hex digits
//! - `WAL_KONAMI_AVS_LOG=1`: the AVS log (the game system's boot, errors) in the payload log
//! - `WAL_KONAMI_CROSSHAIRS=1` (`_STYLE`, `_SIZE`, `_P1`..`_P4`, `_TINT`): the players' aim drawn
//!   over the picture (Gashaaaan Refill!)
//!
//! Native inputs for `native_map`: the board's switches `test service start trigger button2`
//! and `coin` (players 1 and 2 have their own start, trigger and button2); Gashaaaan Refill!:
//! `test service coin start throw` (players 1-4); Otomedius: `test service coin start up down
//! left right a b c`.

use super::{Payload, System};

pub struct Konami;

impl System for Konami {
    fn name(&self) -> &'static str {
        "konami"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_konami.dll", install_as: "libacio.dll" }]
    }
}
