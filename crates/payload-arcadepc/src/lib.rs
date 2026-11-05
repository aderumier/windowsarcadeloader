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
//! COM1 I/O board. Top Gun 2 (`WAL_ARCADEPC_BOARD=jxcamera`): the same board, its SoftKey
//! dongle (`softkey.rs`) and its camera guns (`jxcamera.rs`).
//!
//! Skull of Shadow (`WAL_ARCADEPC_BOARD=boys`): `boys.rs` emulates its USB board and its gun
//! camera.
//!
//! Bullseye Crack Shot (`WAL_ARCADEPC_BOARD=bullseye`): `bullseye.rs` plays its keyboard and
//! mouse mode with the gun.
//!
//! Zombie Crisis (`WAL_ARCADEPC_BOARD=zombiecrisis`): `zombiecrisis.rs` is its `GamePort.dll`
//! (`GetInstance`'s object).
//!
//! Polar Wars (`WAL_ARCADEPC_BOARD=unrealio`, x86 build): `unrealio.rs` is its `UnrealHid.dll`
//! (the UnrealIO USB board's frames: keys, coins, the guns' aim, tickets).
//!
//! Frenzy Express (`WAL_ARCADEPC_BOARD=xio`, x86 build): `xio.rs` is its `xio.dll` (the
//! handlebar, brake, pedals and keys).
//!
//! Hog Wild (`WAL_ARCADEPC_BOARD=interface`, x86 build): `interface.rs` is its `Interface.dll`
//! (the original library's calls forwarded, its parallel port board emulated).
//!
//! Wasteland Racers 2071 (`WAL_ARCADEPC_BOARD=sdc2`, x86 build): `sdc2.rs` is its `SDC2.dll`
//! (the force feedback wheel board: wheel, pedals, buttons).
//!
//! Games with a DemulShooter BepInEx plugin (Quick Shot Arcade, `WAL_DEMUL_PORT`): `demul.rs`
//! is the plugin's TCP client (guns) and posts its keys (coin, operator buttons); no pads.
//! 32-bit games get the x86 build (`wal_arcadepc32.dll`). Both: the common crosshair option
//! (`WAL_CROSSHAIR`) draws player 1's crosshair (`pointer_d3d11`).

#![allow(non_snake_case)]

mod boys;
mod bullseye;
mod commking;
mod demul;
mod gameport;
#[cfg(target_arch = "x86")]
mod interface;
mod jxboard;
mod jxcamera;
mod pads;
#[cfg(target_arch = "x86")]
mod sdc2;
mod softkey;
mod topgun;
mod uaf;
mod unrealio;
#[cfg(target_arch = "x86")]
mod xio;
mod zombiecrisis;

use std::ffi::c_void;

use wal_payload_common::mono_imports;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use pads::*;
pub use zombiecrisis::GetInstance;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // a portrait game in landscape (WAL_UNITY_LANDSCAPE): its portrait setup undone
        // before the common start reads it
        wal_payload_common::unity_landscape::prepare();
        wal_payload_common::start("arcadepc");
        let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) as usize };
        let board = std::env::var("WAL_ARCADEPC_BOARD").unwrap_or_default();
        // the common crosshair option: player 1's gun on the picture (Direct3D 11)
        if (matches!(board.trim(), "syfire" | "jxboard" | "jxcamera" | "bullseye") || std::env::var("WAL_DEMUL_PORT").is_ok()) && wal_payload_common::crosshairs(false) {
            wal_payload_common::pointer_d3d11::init(wal_payload_common::pointer_d3d11::Dot {
                color: [1.0, 0.15, 0.15],
                size: 0.06,
                position: demul::gun_in_picture,
                crosshair: true,
            });
            // Top Gun 2's second gun
            if board.trim() == "jxcamera" {
                wal_payload_common::pointer_d3d11::more_players(2, demul::gun_of_player_in_picture);
            }
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
            _ if matches!(board.trim(), "jxboard" | "jxcamera") => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                if unity == 0 {
                    // loaded at the start (Top Gun 2's `payload_loader`): Unity's messages once it is loaded
                    std::thread::spawn(|| {
                        let unity = loop {
                            let m = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
                            if m != 0 {
                                break m;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        };
                        wal_payload_common::input_filter::messages(unity);
                    });
                }
                jxboard::init(board.trim() == "jxcamera");
            }
            _ if board.trim() == "zombiecrisis" => {
                pads::disconnect();
                // Unreal's messages: the executable's
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
            }
            _ if board.trim() == "boys" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                boys::init();
            }
            _ if board.trim() == "uaf" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, |_| false);
                uaf::init();
            }
            _ if board.trim() == "xio" => {
                pads::disconnect();
                // the profile's code patches (the boot's handlebar and brake calibration)
                wal_payload_common::patches::apply();
                // the game's window messages: the executable's
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
            }
            _ if matches!(board.trim(), "interface" | "sdc2") => {
                pads::disconnect();
                wal_payload_common::patches::apply();
                // the game's window messages: the executable's
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
            }
            _ if board.trim() == "bullseye" => {
                pads::disconnect();
                bullseye::init(unity);
            }
            _ if board.trim() == "unrealio" => {
                pads::disconnect();
                // Unity 5.6: the player is the executable
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                unrealio::init();
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
