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
//! Pigs Might Fly (`WAL_ARCADEPC_BOARD=gamewax`, x86 build): `gamewax.rs` emulates its USB board
//! and overrides its settings (no Shell).
//!
//! Space Warp 66 (`WAL_ARCADEPC_BOARD=spacewarp`): `spacewarp.rs` emulates its two serial boards
//! (coins, the spinner: mouse, stick or keys) and answers its Linux commands.
//!
//! Burning Striker (`WAL_ARCADEPC_BOARD=eolith`, x86 build): `eolith.rs` emulates its COM1 board
//! and hands it its protection's key.
//!
//! Come on Baby! and Come On Baby! 2 (`WAL_ARCADEPC_BOARD=comeonbaby`, x86 build):
//! `comeonbaby.rs` is their parallel port board's reader.
//!
//! Xtom 3D (`WAL_ARCADEPC_BOARD=dosbox`, x86 build, a Glide DOSBox): `dosbox.rs` is DOSBox's
//! SDL keyboard (the virtual sticks' keys) and gives the guest's Glide a window of its own.
//!
//! Robin Hood (`WAL_ARCADEPC_BOARD=robinhood`, x86 build): `robinhood.rs` emulates its serial
//! board (keys, coins, tickets); its crossbow is the common DirectInput joystick.
//!
//! Bounty Hunter (`WAL_ARCADEPC_BOARD=bountyhunter`, x86 build): `bountyhunter.rs` emulates its
//! COM1 board (the pads, coins), replaces its leak-tracking allocator and sets free play.
//!
//! Friction (`WAL_ARCADEPC_BOARD=vsioboard`, x86 build): `vsioboard.rs` is its gun board's
//! `vsIOBoard.dll` (the guns, coins, start and service buttons, through the game's listener); the
//! common crosshair option draws the guns (Direct3D 9).
//!
//! Pop Moto (`WAL_ARCADEPC_BOARD=popmoto`, x86 build): `popmoto.rs` is its `USB_board.dll` (the
//! handlebar, throttle lever, keys, coins) and patches its engine's scripts as the community one.
//!
//! Subway Surfers Arcade (`WAL_ARCADEPC_BOARD=subway`): `subway.rs` posts its cabinet's keys and
//! the hoverboard's double tap, and is its lamps' silent Arduino.
//!
//! Galaxian 3 (`WAL_ARCADEPC_BOARD=mame`, a patched `mame.exe`): `mame.rs` is MAME's raw input
//! devices (a keyboard and the players' guns, from the virtual sticks).
//!
//! Akuma Mortis Immortal (`WAL_ARCADEPC_BOARD=ngun`, x86 build): `ngun.rs` is its parallel port
//! gun board (`inpout32.dll`'s `Inp32` / `Out32`); the common crosshair option draws the guns
//! (Direct3D 9).
//!
//! Bug Busters (`WAL_ARCADEPC_BOARD=bugbusters`, x86 build): `bugbusters.rs` writes its guns and
//! its I/O board's switches into the game; the game draws its own pointers.
//!
//! Hydro Thunder, Offroad Thunder (`WAL_ARCADEPC_BOARD=vcthunder`, x86 build): `vcthunder.rs`
//! feeds the VCThunder runtime's keys and its DirectInput wheel, and sets free play.
//!
//! DCOP (`WAL_ARCADEPC_BOARD=dcop`): `dcop.rs` plays its mouse with the gun (its DemulShooter
//! plugin only hides the game's cursor).
//!
//! Ballistics (`WAL_ARCADEPC_BOARD=ballistics`, x86 build): `ballistics.rs` posts its coin
//! program's coin messages, accepts its display settings dialog and sets free play; its
//! cabinet's GameFrame is the common DirectInput wheel (as a joystick of that name).
//!
//! Arctic Thunder, Ultimate Arctic Thunder (`WAL_ARCADEPC_BOARD=arcticthunder`, x86 build): `arcticthunder.rs` is the
//! cabinet's coin and security manager (`wincsm.exe`) that `goose.dll` reads and writes: the
//! switches, the handlebar and throttle, the credits; and the same for its operator diagnostics
//! (`diag.exe`, started by the test button through the launcher).
//!
//! The House of the Dead 2: Remake (`WAL_ARCADEPC_BOARD=hotd2`, x64 build): `hotd2.rs` plays
//! player 1's mouse with the gun, player 2 on a pad, and puts the crosshairs on the guns (Mono).
//!
//! Mad Bullets (`WAL_ARCADEPC_BOARD=madbullets`, x86 build): `madbullets.rs` plays its mouse
//! with the gun.
//!
//! Games with a DemulShooter BepInEx plugin (Quick Shot Arcade, Wild West Shoot Out,
//! `WAL_DEMUL_PORT`): `demul.rs` is the plugin's TCP client (guns) and posts its keys (coin,
//! operator buttons); no pads. `xmlset.rs`: a game's XML settings set at start
//! (`WAL_ARCADEPC_XML`, Wild West Shoot Out's free play). Mars Sortie
//! (`WAL_ARCADEPC_BOARD=marssortie`, with its plugin): `marssortie.rs` patches its settings'
//! getters (free play, crosshairs, no photos) and posts its operator menus' keypad keys.
//!
//! Family Guy Bowling Arcade (`WAL_ARCADEPC_BOARD=teamplay`): `teamplay.rs` is the Linux
//! cabinet's Team Play I/O library (`teamplayio`, `serialio`) for its DllImports.
//! 32-bit games get the x86 build (`wal_arcadepc32.dll`). Both: the common crosshair option
//! (`WAL_CROSSHAIR`) draws player 1's crosshair (`pointer_d3d11`).

#![allow(non_snake_case)]

#[cfg(target_arch = "x86")]
mod arcticthunder;
#[cfg(target_arch = "x86")]
mod ballistics;
#[cfg(target_arch = "x86")]
mod bountyhunter;
mod boys;
#[cfg(target_arch = "x86")]
mod bugbusters;
mod bullseye;
mod commking;
#[cfg(target_arch = "x86")]
mod comeonbaby;
mod dcop;
mod demul;
#[cfg(target_arch = "x86")]
mod dosbox;
#[cfg(target_arch = "x86")]
mod eolith;
mod gameport;
#[cfg(target_arch = "x86")]
mod gamewax;
#[cfg(target_arch = "x86_64")]
mod hotd2;
#[cfg(target_arch = "x86")]
mod interface;
#[cfg(target_arch = "x86")]
mod madbullets;
mod jxboard;
mod jxcamera;
mod mame;
mod marssortie;
#[cfg(target_arch = "x86")]
mod ngun;
#[cfg(target_arch = "x86")]
mod robinhood;
mod topgun2_landscape;
mod pads;
#[cfg(target_arch = "x86")]
mod popmoto;
#[cfg(target_arch = "x86")]
mod sdc2;
mod softkey;
mod spacewarp;
mod subway;
mod teamplay;
mod topgun;
mod uaf;
mod unrealio;
#[cfg(target_arch = "x86")]
mod vcthunder;
#[cfg(target_arch = "x86")]
mod vsioboard;
#[cfg(target_arch = "x86")]
mod xio;
mod xmlset;
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
        // Only Top Gun 2 changes its portrait setup and draws a dynamic landscape bezel.
        let board = std::env::var("WAL_ARCADEPC_BOARD").unwrap_or_default();
        if board.trim() == "jxcamera" {
            topgun2_landscape::prepare();
        }
        // DCOP on a screen other than 1920x1080: its 1920x1080 desktop
        if board.trim() == "dcop" {
            dcop::prepare();
        }
        #[cfg(target_arch = "x86")]
        if board.trim() == "bugbusters" {
            bugbusters::prepare();
        }
        wal_payload_common::start("arcadepc");
        xmlset::init();
        // Mars Sortie: free play (its guns are the DemulShooter plugin's, below)
        if board.trim() == "marssortie" {
            marssortie::init();
        }
        let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) as usize };
        // the common crosshair option: player 1's gun on the picture (Direct3D 11; the
        // reflection packet plugins show the game's own, `WAL_DEMUL_FIELDS`)
        let demul_dot = std::env::var("WAL_DEMUL_PORT").is_ok() && std::env::var("WAL_DEMUL_FIELDS").is_err();
        if (matches!(board.trim(), "syfire" | "jxboard" | "jxcamera" | "bullseye" | "dcop") || demul_dot) && wal_payload_common::crosshairs(false) {
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
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "gamewax" => {
                pads::disconnect();
                wal_payload_common::patches::apply();
                // the game's window messages: the executable's
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                gamewax::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "eolith" => {
                pads::disconnect();
                // the vault check skipped
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                eolith::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "comeonbaby" => {
                pads::disconnect();
                // the volume serial check skipped
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                comeonbaby::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "robinhood" => {
                pads::disconnect();
                // the dongle's API
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                robinhood::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "bountyhunter" => {
                pads::disconnect();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                bountyhunter::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "vsioboard" => {
                pads::disconnect();
                // the community library's game patches (reboot, DirectInput reading)
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                // the guns' crosshairs (the common option: the game shows none in its levels)
                wal_payload_common::crosshair::init();
                vsioboard::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "arcticthunder" => {
                pads::disconnect();
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                arcticthunder::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "ballistics" => {
                pads::disconnect();
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                ballistics::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "bugbusters" => {
                pads::disconnect();
                // the CD-ROM check
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                bugbusters::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "vcthunder" => {
                pads::disconnect();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, vcthunder::key_held);
                vcthunder::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "ngun" => {
                pads::disconnect();
                // the dongle's checks
                wal_payload_common::patches::apply();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                wal_payload_common::crosshair::init();
                ngun::init();
            }
            _ if board.trim() == "spacewarp" => {
                pads::disconnect();
                wal_payload_common::input_filter::init(unity, spacewarp::key_held);
                spacewarp::init();
            }
            _ if board.trim() == "bullseye" => {
                pads::disconnect();
                bullseye::init(unity);
            }
            _ if board.trim() == "dcop" => {
                pads::disconnect();
                dcop::init(unity);
            }
            _ if board.trim() == "unrealio" => {
                pads::disconnect();
                // Unity 5.6: the player is the executable
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                unrealio::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "dosbox" => {
                pads::disconnect();
                dosbox::init();
            }
            #[cfg(target_arch = "x86_64")]
            _ if board.trim() == "hotd2" => {
                pads::reload_on_left_trigger();
                wal_payload_common::input_filter::init(unity, hotd2::key_held);
                hotd2::init(unity);
            }
            _ if board.trim() == "teamplay" => {
                pads::disconnect();
                // Unity 5.4: the player is the executable
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                teamplay::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "madbullets" => {
                pads::disconnect();
                madbullets::init();
            }
            #[cfg(target_arch = "x86")]
            _ if board.trim() == "popmoto" => {
                pads::disconnect();
                let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
                wal_payload_common::input_filter::init(exe, |_| false);
                popmoto::init();
            }
            _ if board.trim() == "subway" => {
                pads::disconnect();
                subway::init();
            }
            _ if board.trim() == "mame" => {
                pads::disconnect();
                mame::init();
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
                // Unity before 2017.2: the player is the executable
                let player = if unity != 0 { unity } else { unsafe { GetModuleHandleW(std::ptr::null()) as usize } };
                wal_payload_common::input_filter::init(player, demul::key_held);
                demul::start(port);
            }
            None => wal_payload_common::input_filter::init(unity, pads::key_held),
        }
    }
    1
}
