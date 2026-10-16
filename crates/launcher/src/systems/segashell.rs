//! Sega Amusements PC games run by Sega's "Shell" (Transformers: Shadows Rising).
//!
//! The cabinet's Shell program owns coins, credits, settings and the test menu; the game
//! connects to it over TCP (127.0.0.1:18888) and reads its I/O board as a DirectInput
//! joystick. The payload, loaded by `wal-loader`, plays the Shell and feeds the board.
//!
//! Payload options (profile `env`):
//! - `WAL_SEGASHELL_BOARD=<rva>`: the game's board update function (`CSAEBoard::Update`),
//!   replaced by one writing the board record from the virtual sticks
//! - `WAL_SEGASHELL_DEFAULT_RESOLUTION=<rva>`: the game's `DefaultResolution()`, made to
//!   return 1280x720 unless `WAL_SEGASHELL_FULLHD=1`
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`

use super::{Payload, System};

pub struct SegaShell;

impl System for SegaShell {
    fn name(&self) -> &'static str {
        "segashell"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_segashell.dll", install_as: "wal_segashell.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_segashell.dll")
    }
}
