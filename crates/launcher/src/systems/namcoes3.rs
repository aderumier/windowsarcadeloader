//! Namco ES3: 32-bit Mario Kart GP DX and 64-bit Star Wars Battle Pod.
//!
//! The dump's executable is the cabinet launcher `Launcher\RSLauncher.exe`, which starts
//! `Binaries\Win64\SWArcGame-Win64-Shipping.exe`. Both import the HASP HL dongle library
//! `hasp_windows_x64_100610.dll` (one copy in each folder): our 64-bit payload replaces both,
//! so it is loaded in both processes without any injector.
//!
//! Payload options (profile `env`):
//! - `WAL_NAMCOES3_FLATSCREEN`: 1 (default) flat screen render, 0 the cabinet's dome warp
//! - `WAL_NAMCOES3_LANGUAGE`: `ENG JPN CHN ITA SPA RUS POR IND THA` (default: operator setting)
//! - `WAL_NAMCOES3_TEST_TOGGLE`: 1 (default) the test button toggles the latching test switch
//! - `WAL_NAMCOES3_REVERSE_Y`, `WAL_NAMCOES3_REVERSE_THROTTLE`: 1 inverts the axis
//! - `WAL_NAMCOES3_JVS_PORT`: serial port of the I/O board (default `COM3`)
//! - `WAL_NAMCOES3_GAME_XINPUT`: 1 lets the game read the XInput pads itself
//!
//! Native inputs for `native_map`: `test service coin start trigger weapon view enter menuup
//! menudown accelerate brake`. The flight stick is `lx`/`ly` (or the d-pad), the throttle the
//! `accel`/`brake` pedals or `ry` (or the accelerate / brake buttons).

use super::{Payload, System};

pub struct NamcoEs3;

impl System for NamcoEs3 {
    fn name(&self) -> &'static str {
        "namcoes3"
    }

    fn payloads(&self) -> &'static [Payload] {
        // The 32-bit board DLL is also the game's Banapass reader import. It hooks DirectInput
        // for the virtual wheel and reads the game's force-feedback value.
        &[Payload { file: "wal_bngrw.dll", install_as: "bngrw.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        // Mario Kart v1.10.22 does not import bngrw.dll, so load the same payload
        // at the executable entry point. Newer 32-bit builds load it themselves.
        Some("bngrw.dll")
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[
            Payload { file: "wal_namcoes3.dll", install_as: "Launcher/hasp_windows_x64_100610.dll" },
            Payload { file: "wal_namcoes3.dll", install_as: "Binaries/Win64/hasp_windows_x64_100610.dll" },
        ])
    }

    fn hidden(&self) -> &'static [&'static str] {
        // leftovers of another loader installed in the dump
        &["hasp_windows_x64_100610.dll.orig", "swpod.ini", "swpod.log"]
    }
}
