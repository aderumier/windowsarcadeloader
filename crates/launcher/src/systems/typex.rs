//! Taito Type X / Type X2 (and NESiCA games built on them, like 3D Cosplay Mahjong).
//!
//! The games talk to a JVS I/O board on a serial port and import no driver DLL: the payload
//! is loaded by `wal-loader` before the game entry point.
//!
//! Payload options (profile `env`):
//! - `WAL_TYPEX_JVS_PORT`: serial port of the JVS board (default `COM2`)
//! - `WAL_TYPEX_DDRIVE`: folder for the game's `D:\` data (default `WindowsLoader`)
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`

use super::{Payload, System};

pub struct TypeX;

impl System for TypeX {
    fn name(&self) -> &'static str {
        "typex"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_typex.dll", install_as: "wal_typex.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_typex.dll")
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["WindowsLoader"]
    }
}
