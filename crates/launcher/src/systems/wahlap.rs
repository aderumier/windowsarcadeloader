//! Wahlap Technology PC cabinets (Storm Racer G).
//!
//! The game imports `usb_io.dll` (Wahlap's USB I/O board driver: wheel, pedals, buttons,
//! coin counter, steering motor) statically: our payload replaces it.
//!
//! Payload options (profile `env`):
//! - `WAL_WAHLAP_FFB_SPRING` / `WAL_WAHLAP_FFB_ROAD`: force feedback strengths in percent
//!   (default 100): the steering motor's centering spring (by speed), the vibration off the road
//! - `WAL_VFW_CODECS`, `WAL_VFW_READ_FIX`: the movies' WMV9 codec, Wine's AVIStreamRead sizes
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`
//!
//! Native inputs for `native_map`: the board's button bits `btn1 start coin btn4 btn5 up down
//! boost btn10` (`coin`: its coin switch), its coin counter `coin_counter`, and the game's own PC keys `key_enter key_space
//! key_backspace key_left key_up key_right key_down key_b key_e key_q key_w` (the real keyboard
//! and XInput pads are hidden from the game: every input comes from the launcher).

use super::{Payload, System};

pub struct Wahlap;

impl System for Wahlap {
    fn name(&self) -> &'static str {
        "wahlap"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_wahlap.dll", install_as: "usb_io.dll" }]
    }
}
