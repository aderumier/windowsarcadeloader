//! Raw Thrills PC cabinets (The Fast and the Furious, its Drift, Super Bikes 1 / 2 and SuperCars,
//! Nicktoons Nitro Racing, H2O Overdrive, Super Alpine Racer, X-Games Snowboarder, Winter X
//! Games SnoCross, Dirty Drivin', Cars, Guitar Hero Arcade EDITION, Frogger, Injustice Arcade).
//!
//! Most games read their JAMMA USB I/O board (HID) themselves and import no driver DLL to
//! replace: the payload is started with the game through `wal-loader`. TMNT reads the common
//! input socket in its patched Unity poll; Guitar Hero Arcade feeds the game's internal guitar
//! state from that same input stream.
//!
//! Payload options (profile `env`):
//! - `WAL_RAWTHRILLS_GAME`: `guitarhero` (Guitar Hero Arcade's internal inputs),
//!   `fnf` / `drift` / `superbikes` / `nicktoons` / `h2overdrive` (JAMMA
//!   USB board reports), `supercars` / `alpine` / `snowboarder` / `snocross` (`SetControl`),
//!   `superbikes2` (the game's keyboard and analog tables), `dirtydrivin` (its developer
//!   keyboard, coin callback and switch latches), `cars` (its RIO board behind libusb0),
//!   `frogger` (its serial JAMMA input reports, USB watchdog and HASP stub), `injustice`
//!   (its RIO board, HASP dongle and virtual barcode cards on emulated webcams):
//!   the game's board from the virtual sticks
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
//! `start view tunes shift shiftdown coin1 coin2 test service bill volup voldown brake`;
//! Frogger: `start coin1 coin2 test service volup voldown`); player
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

    fn payloads64(&self) -> Option<&'static [Payload]> {
        // TMNT's patched Unity IOManager reads the launcher's input socket directly; it needs
        // no architecture-specific Raw Thrills board payload.
        Some(&[])
    }

    fn hidden(&self) -> &'static [&'static str] {
        // another loader's input settings: with them, the dump's libusb0.dll takes that
        // loader's path and crashes at start (Super Alpine Racer)
        &["monitor_input.bin", "monitor_misc.bin"]
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        // the game's D: data (default WAL_RAWTHRILLS_DDRIVE)
        &["WindowsLoader"]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_rawthrills.dll")
    }
}
