//! Arcade PC: PC-based arcade games without a cabinet I/O API to replace (Galaxy Rangers).
//!
//! Galaxy Rangers (Unity, 64-bit): the dump's community `Assembly-CSharp.dll` reads the guns
//! as XInput pads (`Controls.ini`). Our 64-bit payload is that `xinput1_3.dll`: four pads from
//! the virtual sticks, the real keyboard and mouse filtered (docs §6.3i).
//!
//! Native inputs for `native_map`: `trigger reload change start coin offscreen` (every player)
//! and player 1's operator keys `menu menu_up menu_down menu_left menu_right`.
//!
//! Top Gun (32-bit, `WAL_ARCADEPC_BOARD=syfire`): the payload emulates its dongle and its COM1
//! I/O board; natives `trigger offscreen coin settings left right replenish refill door`.
//! Games with one of the DemulShooter BepInEx plugins (Quick Shot Arcade): the profile's
//! `WAL_DEMUL_PORT` makes the payload the plugin's TCP client (the guns) and the poster of its
//! keyboard keys; natives `trigger reload action change offscreen key_<name>` (docs §6.3i).

use super::{Payload, System};

pub struct ArcadePc;

impl System for ArcadePc {
    fn name(&self) -> &'static str {
        "arcadepc"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_arcadepc32.dll", install_as: "xinput1_3.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_arcadepc.dll", install_as: "xinput1_3.dll" }])
    }

    fn hidden(&self) -> &'static [&'static str] {
        // the community build's device list, rewritten at each start
        &["InputDevices.txt"]
    }
}
