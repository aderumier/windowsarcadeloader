//! Sega ALLS with its `amdaemon.exe` (Mario & Sonic at the Tokyo 2020 Olympic Games Arcade):
//! the game talks to amdaemon (keychip, ALL.Net, the USB I/O board, credits), which talks to
//! the cabinet's hardware. The profile's `background` list starts amdaemon before the game,
//! both through `wal-loader` with the 64-bit payload: in amdaemon it emulates the keychip,
//! the hardware monitor and the "Type 4" USB I/O board (fed by the virtual sticks), in the game
//! the LED board on COM1 (docs §6.3l).
//!
//! The cabinet's drives (profile `drives`): E: the AMFS (ICF files, the system backup
//! `sysfile.dat`), Y: the game's application data.

use super::{Payload, System};

pub struct Alls;

impl System for Alls {
    fn name(&self) -> &'static str {
        "alls"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_alls.dll", install_as: "wal_alls.dll" }])
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_alls.dll")
    }
}
