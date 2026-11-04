//! Jetpack Joyride Arcade (Halfbrick's native engine, 32-bit, Direct3D 9).
//!
//! - **Folder.** The game reads its data from `..\DataPC\` (the cabinet runs it from
//!   `C:\Joyride\DataPC`): the profile maps that folder to the dump (`c_folders`) and
//!   `WAL_ADRENALINE_CWD` makes it the current directory before the game's code runs.
//! - **Dongle.** `matrix32.dll` imported by name (`matrix.rs` [`hook_names`]).
//! - **Board.** `KaboomCOMBoardManager` loads `KaboomComDll.dll` and resolves its 25 functions
//!   with the executable's `GetProcAddress` (any missing: no board): `kaboom.rs`'s, `InitLib`
//!   with its callbacks.
//! - **Button.** The jetpack button is on the board's keypad (any of its down, select and up
//!   lines: `kaboom.rs`'s callback); the operator's key opens the operator menu. The game's
//!   DirectInput keyboard sees no key (`WAL_DINPUT_DISABLE`).

use std::sync::atomic::Ordering;

use wal_payload_common::{log, mono_imports};

use crate::matrix::hook_names;

/// Jetpack Joyride's hooks, if the executable is it (it imports the dongle by name).
pub fn init() -> bool {
    if let Ok(dir) = std::env::var("WAL_ADRENALINE_CWD") {
        let wide: Vec<u16> = dir.encode_utf16().chain([0]).collect();
        let ok = unsafe { windows_sys::Win32::System::Environment::SetCurrentDirectoryW(wide.as_ptr()) } != 0;
        log!("adrenaline: current directory {dir}: {}", if ok { "set" } else { "failed" });
    }
    if hook_names() == 0 {
        return false;
    }
    crate::kaboom::WITH_CALLBACKS.store(true, Ordering::Relaxed);
    mono_imports::hook_game(crate::resolve);
    true
}
