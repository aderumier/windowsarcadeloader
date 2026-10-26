//! Arcade PC payload: PC-based arcade games without a cabinet I/O API of their own to replace.
//!
//! Galaxy Rangers (`WXR.exe`, Unity 2019.4, Mono): the dump's `Assembly-CSharp.dll` is a
//! community build whose `ArcadeControls` class replaces the cabinet's camera guns and serial
//! I/O board with `Controls.ini`: each player aims with an XInput pad's left stick as an
//! absolute screen position (or a raw mouse), shoots with its right trigger, and the operator
//! menu is on the keyboard (`F2`, arrows). The payload is that `xinput1_3.dll` (the class's
//! `DllImport`):
//!
//! * `pads.rs`: four connected pads built from the virtual sticks: the gun position on the left
//!   stick, the trigger, reload, weapon change, start and coin buttons.
//! * `payload-common`'s `input_filter`: the real keyboard and mouse never reach the game
//!   (Unity's window messages and raw input, the class's `GetAsyncKeyState` polling); the
//!   operator keys it polls come from the virtual sticks.
//!
//! Unity loads `xinput1_4.dll` first: the profile disables it, so Unity loads this payload at
//! start too (its own XInput pads are ours) and the filters are in place before the first frame.
//!
//! Top Gun (`WAL_ARCADEPC_BOARD=syfire`): `topgun.rs` emulates its dongle and its serial I/O
//! board (coins, keys, the camera gun's shots); no pads, no keyboard.
//!
//! Happy Water Sprite II (`WAL_ARCADEPC_BOARD=commking`): `commking.rs` emulates its I/O board,
//! HASP dongle and registration.
//!
//! Mech Tornado (`WAL_ARCADEPC_BOARD=gameport`): `gameport.rs` is its I/O DLL's C API.
//! Mech Dinosaur, Wisdom Zombies (`WAL_ARCADEPC_BOARD=jxboard`): `jxboard.rs` emulates their
//! COM1 I/O board.
//!
//! Games with a DemulShooter BepInEx plugin (Quick Shot Arcade, `WAL_DEMUL_PORT`): `demul.rs`
//! is the plugin's TCP client (guns) and posts its keys (coin, operator buttons); no pads.
//! 32-bit games get the x86 build (`wal_arcadepc32.dll`). Both: the common crosshair option
//! (`WAL_CROSSHAIR`) draws player 1's crosshair (`pointer_d3d11`).

#![allow(non_snake_case)]

mod commking;
mod demul;
mod gameport;
mod jxboard;
mod mono_imports;
mod pads;
mod topgun;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use pads::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("arcadepc");
        let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) as usize };
        let board = std::env::var("WAL_ARCADEPC_BOARD").unwrap_or_default();
        // the common crosshair option: player 1's gun on the picture (Direct3D 11)
        if (matches!(board.trim(), "syfire" | "jxboard") || std::env::var("WAL_DEMUL_PORT").is_ok()) && wal_payload_common::crosshairs(false) {
            wal_payload_common::pointer_d3d11::init(wal_payload_common::pointer_d3d11::Dot {
                color: [1.0, 0.15, 0.15],
                size: 0.06,
                position: demul::gun_in_picture,
                crosshair: true,
            });
        }
        match std::env::var("WAL_DEMUL_PORT").ok().and_then(|p| p.trim().parse().ok()) {
            _ if board.trim() == "syfire" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                topgun::init();
            }
            _ if board.trim() == "gameport" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                gameport::init_board();
            }
            _ if board.trim() == "jxboard" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                jxboard::init();
            }
            _ if board.trim() == "commking" => {
                pads::disconnect();
                // Unity 3.5: the player is the executable
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                commking::init();
            }
            Some(port) => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, demul::key_held);
                demul::start(port);
            }
            None => wal_payload_common::input_filter::init(unity, pads::key_held),
        }
    }
    1
}
