//! Sega Nu / Nu SX (Sonic Dash Extreme, Luigi's Mansion Arcade).
//!
//! The games link Sega's amLib, whose "amy" layer talks to the AMEX board's drivers (found by
//! their device interface classes: keychip DS, EEPROM, SRAM, GPIO, JVS), the keychip driver
//! `\??\FddDriver` and the system's registry values. They import no driver DLL: the payload is
//! loaded by `wal-loader` (64-bit games: `wal-loader64`, profile `payload_loader`).
//!
//! Payload options (profile `env`):
//! - `WAL_SEGANU_GAME_ID`, `WAL_SEGANU_PLATFORM_ID` (`AAV0` Nu, `AAW0` Nu SX),
//!   `WAL_SEGANU_REGION` (hex): the keychip's values
//! - `WAL_SEGANU_DIPSW`: the main board's DIP switches on, hex (default `00`)
//! - `WAL_SEGANU_SAVE`: folder of the EEPROM and SRAM files (default `sv`)
//! - `WAL_SEGANU_CWD`: the game's working directory, relative to its own
//! - `WAL_SEGANU_AMLOG`, `WAL_SEGANU_AMLOG_LEVELS`: amLib's log in the payload log
//! - `WAL_SEGANU_FREEPLAY_DEFAULT`, `WAL_SEGANU_FREEPLAY_BYTE`: free play in the game's
//!   default credit settings / its own coin settings in the EEPROM
//! - `WAL_SEGANU_UNPACK`, `WAL_SEGANU_UNPACK_PATCHES`: a packed 64-bit executable's entry point
//!   and the code written back once unpacked
//! - `WAL_SEGANU_LMAIO`, `WAL_SEGANU_LMAIO_OBJECT`, `WAL_SEGANU_LMAIO_SW`: Luigi's Mansion
//!   Arcade's I/O board (port, the game's board object, the switches' bits)
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

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_seganu64.dll", install_as: "wal_seganu.dll" }])
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_seganu.dll")
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["sv"]
    }
}
