//! Tsunami arcade cabinets (Tsumo "TsuMo Racing": ReVolt).
//!
//! The game imports no cabinet driver DLL (plain DirectInput, DirectDraw and Miles sound):
//! on the cabinet, `tsuinput.exe` feeds the wheel to a virtual DirectInput device
//! ("TsuMo Joystick, Throttle") and exposes the coin mechanism through the `TsuInput` COM
//! object of `TsuInputLib.dll` (credits, `SimulateCoin`...). Our payload emulates both,
//! loaded by `wal-loader` before the game entry point.

use super::{Payload, System};

pub struct Tsunami;

impl System for Tsunami {
    fn name(&self) -> &'static str {
        "tsunami"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_tsunami.dll", install_as: "wal_tsunami.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_tsunami.dll")
    }
}
