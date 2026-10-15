//! Sega RingEdge (Virtua Tennis 4, Let's Go Island 3D...).
//!
//! The games link Sega's amLib and talk to the cabinet's kernel drivers (SRAM, SMBus EEPROM),
//! its system services (keychip, master: PCP over TCP) and a JVS I/O board.
//! They import no driver DLL: the payload is loaded by `wal-loader`.
//!
//! Payload options (profile `env`):
//! - `WAL_RINGEDGE_BOARD`: `ringedge2` for RingEdge 2 games (platform id `AAL2`)
//! - `WAL_RINGEDGE_SAVE`: folder of the SRAM and EEPROM files (default `sv`)
//! - `WAL_RINGEDGE_PCP`: keychip / master answers `key=value;...` (e.g. the game id)
//! - `WAL_RINGEDGE_FOREGROUND`: arguments of the cabinet's applications the game hands over
//!   to (relaunch), `<n>=<arguments>;...` (default `3=-t`: the test menu)
//! - `WAL_RINGEDGE_AMLOG`, `WAL_RINGEDGE_AMLOG_LEVELS`: amLib's log in the payload log
//! - `WAL_TYPEX_JVS_PORT`: port of the JVS board (default `mxjvs`, the JVS driver; `COM4`:
//!   games using the serial port)
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`

use super::{Payload, System};

pub struct RingEdge;

impl System for RingEdge {
    fn name(&self) -> &'static str {
        "ringedge"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_ringedge.dll", install_as: "wal_ringedge.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_ringedge.dll")
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["sv"]
    }
}
