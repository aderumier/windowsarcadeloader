//! Taito Type X payload, loaded by `wal-loader` (Type X games import no driver DLL).
//!
//! * JVS I/O board on the serial port (`WAL_TYPEX_JVS_PORT`, default `COM2`), fed by the
//!   virtual arcade sticks (payload-common `jvs_board.rs`, in Taito
//!   stick mode).
//! * `D:\` redirected to a game folder (`WAL_TYPEX_DDRIVE`, default `WindowsLoader`).
//! * Profile code patches (`WAL_PATCHES`).
//! * Lightgun games (`WAL_TYPEX_GUNS`): guns written in the game's memory (`guns.rs`).
//! * Touch screen on the JVS screen position input (`WAL_TYPEX_JVS_TOUCH`, Harikiri Professional
//!   Baseball), its pointer shown with the common crosshair (`WAL_CROSSHAIR`).
//! * Block King Ball Shooter's touch sensor driver (`WAL_TYPEX_LSDRV`, `touch.rs`).
//! * The medal games' TXE001 backup SRAM board (`TxedLap.dll` imports, `sram.rs`).
//! * Medal I/O board on a serial port (`WAL_TYPEX_MEDAL_PORT`, `medal.rs`).
//! * Steering (wheel motor) board of driving games on a serial port (`WAL_TYPEX_WHEEL_PORT`,
//!   `wheel.rs`).
//! * Force feedback computed from the game's memory (`WAL_TYPEX_FFB`, `ffb.rs`).
//! * Mahjong panel read as a keyboard (`WAL_TYPEX_MAHJONG`, `mahjong.rs`).
//! * Elevator Action Death Parade's attraction doors drawn over the picture (`WAL_TYPEX_DOORS`,
//!   `doors.rs`).
//! * K-On! After School Rhythm Selection's card scanner: a virtual card picked on an overlay
//!   (`WAL_TYPEX_KON_CARDS`, `kon_cards.rs`).
//! * Won!Tertainment Music Channel's Idol Puppy toy, barcode card scanner (a virtual card picked
//!   on an overlay) and touch screen (`WAL_TYPEX_WONTERTAINMENT`, `wontertainment.rs`).
//! * Sonic Blast Heroes' punching pad from a gamepad trigger (`WAL_TYPEX_PUNCH`, `punch.rs`).
//! * Video for Windows codecs shipped with the game (`WAL_VFW_CODECS`).
//! * Servers the cabinet starts beside the game (`WAL_TYPEX_SERVERS`, `servers.rs`).
//! * The cabinet's program switcher (`WAL_TYPEX_SWITCHER`, `switcher.rs`): game <-> test menu
//!   through the launcher.

#![allow(non_snake_case)]

mod doors;
mod ffb;
mod guns;
mod immersion;
mod kon_cards;
mod mahjong;
mod medal;
mod punch;
mod servers;
mod sram;
mod switcher;
mod touch;
mod wheel;
mod wontertainment;
mod wontertainment_cards;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("typex");
        // the program switches (switch.exe): the switcher itself does nothing else
        switcher::init(module);
        if switcher::is_switcher() {
            return TRUE;
        }
        patches::apply();
        drive::init("WAL_TYPEX_DDRIVE", "WindowsLoader");
        wal_payload_common::jvs_board::init("COM2");
        // the JVS touch screen's pointer (the gun games draw their own crosshairs)
        if std::env::var("WAL_TYPEX_JVS_TOUCH").is_ok() {
            wal_payload_common::crosshair::init();
        }
        medal::init();
        wheel::init();
        mahjong::init();
        guns::init();
        doors::init();
        ffb::init();
        touch::init();
        punch::init();
        kon_cards::init();
        wontertainment::init();
        sram::init();
        servers::init();
        wal_payload_common::vfw::init();
        // Taito crypto service (KOF XIII Climax): only for a profile naming its key
        if wal_payload_common::crypto::key_configured() {
            wal_payload_common::crypto::start();
        }
        log!("typex: initialized");
    }
    TRUE
}
