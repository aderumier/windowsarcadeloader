//! Sega Nu / Nu SX (Sonic Dash Extreme).
//!
//! The games link Sega's amLib, whose "amy" layer talks to the AMEX board's drivers (found by
//! their device interface classes: keychip DS, EEPROM, SRAM, GPIO, JVS), the keychip driver
//! `\??\FddDriver` and the system's registry values. They import no driver DLL: the payload is
//! loaded by `wal-loader`.
//!
//! Payload options (profile `env`):
//! - `WAL_SEGANU_GAME_ID`, `WAL_SEGANU_PLATFORM_ID` (`AAV0` Nu, `AAW0` Nu SX),
//!   `WAL_SEGANU_REGION` (hex): the keychip's values
//! - `WAL_SEGANU_DIPSW`: the main board's DIP switches on, hex (default `00`)
//! - `WAL_SEGANU_SAVE`: folder of the EEPROM and SRAM files (default `sv`)
//! - `WAL_SEGANU_CWD`: the game's working directory, relative to its own
//! - `WAL_SEGANU_AMLOG`, `WAL_SEGANU_AMLOG_LEVELS`: amLib's log in the payload log
//! - `WAL_TYPEX_JVS_*`: the JVS board's options (outputs, layout...)
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`

use super::{Payload, System};

pub struct SegaNu;

impl System for SegaNu {
    fn name(&self) -> &'static str {
        "seganu"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_seganu.dll", install_as: "wal_seganu.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_seganu.dll")
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["sv"]
    }
}
