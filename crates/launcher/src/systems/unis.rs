//! UNIS: UNIS's PC-based arcade games (Dino Invasion).
//!
//! Dino Invasion (Unity 5.4, Mono, 64-bit; the "shunxiang" gun cabinet): our payload is the
//! `xinput1_3.dll` Unity loads at start; it emulates the COM3 I/O board (coins, triggers, the
//! operator's keys), the guns' camera tracker (`LC64.dll`) and the dongles, the real keyboard
//! and mouse filtered (docs §6.3j).
//!
//! Native inputs for `native_map`: `trigger start coin` (every player) and the operator's
//! `menu select`.

use super::{Payload, System};

pub struct Unis;

impl System for Unis {
    fn name(&self) -> &'static str {
        "unis"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_unis32.dll", install_as: "xinput1_3.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_unis.dll", install_as: "xinput1_3.dll" }])
    }
}
