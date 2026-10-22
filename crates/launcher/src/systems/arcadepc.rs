//! Arcade PC: PC-based arcade games without a cabinet I/O API to replace (Galaxy Rangers).
//!
//! Galaxy Rangers (Unity, 64-bit): the dump's community `Assembly-CSharp.dll` reads the guns
//! as XInput pads (`Controls.ini`). Our 64-bit payload is that `xinput1_3.dll`: four pads from
//! the virtual sticks, the real keyboard and mouse filtered (docs §6.3i).
//!
//! Native inputs for `native_map`: `trigger reload change start coin offscreen` (every player)
//! and player 1's operator keys `menu menu_up menu_down menu_left menu_right`.

use super::{Payload, System};

pub struct ArcadePc;

impl System for ArcadePc {
    fn name(&self) -> &'static str {
        "arcadepc"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_arcadepc.dll", install_as: "xinput1_3.dll" }])
    }

    fn hidden(&self) -> &'static [&'static str] {
        // the community build's device list, rewritten at each start
        &["InputDevices.txt"]
    }
}
