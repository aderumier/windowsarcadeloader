//! Raw Thrills PC cabinets (The Fast and the Furious, its Drift and Super Bikes).
//!
//! The games read their JAMMA USB I/O board (HID) themselves and import no driver DLL to
//! replace: the payload is started with the game through `wal-loader`.
//!
//! Payload options (profile `env`):
//! - `WAL_RAWTHRILLS_GAME`: `fnf` / `drift` / `superbikes`, the game's board from the virtual sticks
//! - `WAL_RAWTHRILLS_BRAKE`: `switch` or `analog` brake (default: the game's, Super Bikes
//!   and Drift switch, The Fast and the Furious analog)
//! - `WAL_RAWTHRILLS_DDRIVE`: folder for the game's `D:\` paths (logs, personal bests),
//!   relative to the game directory or absolute (default `WindowsLoader`)
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`
//!
//! Native inputs for `native_map`: `start coin1 coin2 brake handbrake service test bill
//! volup voldown view button2 button3 tunes gear1..gear4 shiftup (SHIFT) shiftdown`; player 2's coin is
//! coin 2, the keypad the numeric keypad (`WAL_RAWTHRILLS_KEYPAD=0`: off).

use super::{Payload, System};

pub struct RawThrills;

impl System for RawThrills {
    fn name(&self) -> &'static str {
        "rawthrills"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_rawthrills.dll", install_as: "wal_rawthrills.dll" }]
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        // the game's D: data (default WAL_RAWTHRILLS_DDRIVE)
        &["WindowsLoader"]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_rawthrills.dll")
    }
}
