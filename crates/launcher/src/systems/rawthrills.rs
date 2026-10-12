//! Raw Thrills PC cabinets (The Fast and the Furious, its Drift, Super Bikes 1 / 2 and SuperCars,
//! Nicktoons Nitro Racing, H2O Overdrive, Super Alpine Racer, X-Games Snowboarder, Winter X
//! Games SnoCross).
//!
//! The games read their JAMMA USB I/O board (HID) themselves and import no driver DLL to
//! replace: the payload is started with the game through `wal-loader`.
//!
//! Payload options (profile `env`):
//! - `WAL_RAWTHRILLS_GAME`: `fnf` / `drift` / `superbikes` / `nicktoons` / `h2overdrive` (JAMMA
//!   USB board reports), `supercars` / `alpine` / `snowboarder` / `snocross` (`SetControl`),
//!   `superbikes2` (the game's keyboard and analog tables): the game's board from the virtual
//!   sticks
//! - `WAL_RAWTHRILLS_BRAKE`: `switch` or `analog` brake (default: the game's, Super Bikes
//!   and Drift switch, The Fast and the Furious analog)
//! - `WAL_RAWTHRILLS_DDRIVE`: folder for the game's `D:\` paths (logs, personal bests),
//!   relative to the game directory or absolute (default `WindowsLoader`)
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`
//!
//! Native inputs for `native_map`: `start coin1 coin2 brake handbrake service test bill
//! volup voldown view button2 button3 tunes gear1..gear4 shiftup (SHIFT) shiftdown` (H2O
//! Overdrive: `start coin1 coin2 service test bill volup voldown view view2 view3`; ski games:
//! `start view1 view2 view3 tunes coin1 coin2 test service bill volup voldown start2`;
//! SnoCross: `start snowball view coin1 coin2 test service bill volup voldown`; Super Bikes 2:
//! `start view tunes shift shiftdown coin1 coin2 test service bill volup voldown brake`); player
//! 2's coin is coin 2, the keypad the numeric keypad (`WAL_RAWTHRILLS_KEYPAD=0`: off).

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
