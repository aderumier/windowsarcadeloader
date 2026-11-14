//! Sega Europa-R / Sega Amusements Europe PC cabinets (Ford Racing Full Blown, SEGA Rally 3, GRID,
//! Daytona Championship USA and its New Season Edition, Men In Black).
//!
//! Ford Racing imports `inpout32.dll` statically (the force feedback board on the parallel
//! port): our payload replaces it and, loaded before the game code, also plays its USB I/O
//! board (a Happ UGCI HID device: wheel, pedals, buttons) and the cabinet's GameShell (coins,
//! credits). TEST relaunches into the cabinet's GameShell (its test menu), the payload loaded
//! there by `wal-loader`.
//!
//! Payload options (profile `env`):
//! - `WAL_EUROPAR_FFB`: force feedback strength in percent (default 100)
//! - `WAL_EUROPAR_FREEPLAY=0|1`: free play over the game's setting (`data/gamesettings.ini`)
//! - `WAL_EUROPAR_RESOLUTION=<w>x<h>`: the game's screen mode; `WAL_EUROPAR_PATCHES_<w>`: code
//!   patches for that width
//! - `WAL_EUROPAR_BOARD=rally3`, `WAL_EUROPAR_FFB_FORMAT=rally3`, `WAL_EUROPAR_CREDITS=<rva>`:
//!   SEGA Rally 3's board, force feedback and credit count (the payload installed as its
//!   `xinput1_3.dll`, profile `payload_install`)
//! - `WAL_EUROPAR_SHELL_ARG=1`, `WAL_EUROPAR_SHELL_MSGS=window=<n>,credit=<n>`: GRID's shell
//!   (its window on the command line, its message numbers); `WAL_EUROPAR_FFB_FORMAT=grid`
//! - `WAL_EUROPAR_SHELL_TCP=<port>`, `WAL_EUROPAR_INPWRAPPER=1`, `WAL_EUROPAR_FFB_FORMAT=daytona`:
//!   Daytona Championship USA's Sega Shell over TCP and InpWrapper force feedback (the game run by
//!   `wal-loader`, profile `payload_loader`)
//! - `WAL_EUROPAR_BOARD=mib`: Men In Black (64-bit Unity: `wal_europar64.dll`, profile
//!   `payload_loader`): its guns, its shell over TCP (coins, credits, security), free play
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`
//!
//! Native inputs for `native_map`: the board's buttons `shift_up shift_down start view`.

use super::{Payload, System};

pub struct EuropaR;

impl System for EuropaR {
    fn name(&self) -> &'static str {
        "europar"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_europar.dll", install_as: "inpout32.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        // Men In Black (Unity): loaded by wal-loader (profile `payload_loader`)
        Some(&[Payload { file: "wal_europar64.dll", install_as: "inpout32.dll" }])
    }

    fn program_loader(&self) -> Option<&'static str> {
        // the test menu: GameShell.exe, which does not import inpout32.dll
        Some("wal_europar.dll")
    }
}
