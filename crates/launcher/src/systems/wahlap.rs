//! Wahlap Technology PC cabinets (Storm Racer G, Storm Rider).
//!
//! The games import their I/O board driver statically: `usb_io.dll` (Storm Racer G: wheel,
//! pedals, buttons, coin counter, steering motor) or `motobikeio.dll` (Storm Rider: handlebar,
//! throttle, brake, buttons, coin counters, board memory): our payload replaces both.
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
        // one DLL for both boards (Storm Racer G's usb_io.dll, Storm Rider's motobikeio.dll):
        // each game loads the one it imports
        &[
            Payload { file: "wal_wahlap.dll", install_as: "usb_io.dll" },
            Payload { file: "wal_wahlap.dll", install_as: "motobikeio.dll" },
        ]
    }
}
