//! Adrenaline Amusements: Adrenaline's PC-based arcade games (Skylanders Cloud Patrol, Rabbids
//! Hollywood, Tomb Raider, Rampage, Hot Wheels, Spider-Man Homecoming).
//!
//! Skylanders Cloud Patrol (Unity 4.2, Mono, 32-bit): a touch screen game (a PQ Labs
//! multi-touch frame) on the "Kaboom" I/O board (coins, the operator's keypad, the ticket
//! dispenser). Rabbids Hollywood (Unity 2017.2, Mono, 64-bit): four Sixense STEM tracked guns
//! on the Kaboom Adrio board (coins, start buttons, the operator's keypad, the ticket
//! feeders). Our payload is the `xinput1_3.dll` Unity loads at start: each player's gun
//! touches the screen (Skylanders) or its tracker aims (Rabbids) where it points, the I/O
//! board and the Matrix dongle are emulated, the real keyboard and mouse filtered (docs
//! §6.3k).
//!
//! Native inputs for `native_map`: `touch trigger reload start coin` (every player) and the
//! operator's `operator up down select`.

use super::{Payload, System};

pub struct Adrenaline;

impl System for Adrenaline {
    fn name(&self) -> &'static str {
        "adrenaline"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_adrenaline.dll", install_as: "xinput1_3.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_adrenaline64.dll", install_as: "xinput1_3.dll" }])
    }
}
