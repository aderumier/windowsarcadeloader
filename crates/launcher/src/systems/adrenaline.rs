//! Adrenaline Amusements: Adrenaline's PC-based arcade games (Skylanders Cloud Patrol).
//!
//! Skylanders Cloud Patrol (Unity 4.2, Mono, 32-bit): a touch screen game (a PQ Labs
//! multi-touch frame) on the "Kaboom" I/O board (coins, the operator's keypad, the ticket
//! dispenser). Our payload is the `xinput1_3.dll` Unity loads at start: each player's gun
//! touches the screen where it points while its trigger is held, the I/O board and the Matrix
//! dongle are emulated, the real keyboard and mouse filtered (docs §6.3k).
//!
//! Native inputs for `native_map`: `touch coin` (every player) and the operator's `operator`.

use super::{Payload, System};

pub struct Adrenaline;

impl System for Adrenaline {
    fn name(&self) -> &'static str {
        "adrenaline"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_adrenaline.dll", install_as: "xinput1_3.dll" }]
    }
}
