//! Konami e-Amusement PC payload (Nobiteke! Byon Byon Daisakusen, Gashaaaan Refill!).
//!
//! The game reaches its cabinet I/O through `libacio.dll`, Konami's ACIO serial bus driver:
//! the HBHI I/O board (switches, two controllers of three analog axes each, coin slots, coin
//! counters, lamps, the security plugs) and the ICCA e-Amusement card reader. This DLL takes its
//! place and answers the functions the game imports, fed by the virtual arcade sticks.
//!
//! * `hbhi.rs`: the I/O board status buffer, its security plugs, coin and lamp outputs.
//! * `icca.rs`: the card reader (the `card` control inserts / ejects a card).
//! * `acio.rs`: the bus itself (start, devices present, versions).
//! * `net.rs`: the cabinet's own network address setup, answered.
//! * `eamuse.rs`: e-Amusement offline (the boot's connection check).
//! * `screen.rs`: the picture at the screen's resolution, a borderless window.
//! * `avs.rs`: the AVS log, on request.
//!
//! Otomedius loads it as `libacio.dll` too (`pjei.rs`, the PJEI board), its libbms and
//! libtouchpanel imports pointed at the payload (`otomedius.rs`, `touchpanel.rs`).
//!
//! Winning Eleven 2008 also has the PJEI board, a PS2 pad port (`pjec.rs`) and the touch screen
//! (`winning_eleven.rs`). Winning Eleven 2012 (`we2012.rs`) is `weac12_bootstrap_release.dll`,
//! run by `wal-avs-boot.exe`: the same boards and a PS3 pad port (`i36i.rs`).
//!
//! GTI Club: Supermini Festa! has the HBHI board and, on a second bus (`dllibacio.dll`, also the
//! payload), the HGTH wheel board (`gti_club.rs`, `hgth.rs`): wheel, pedals, the wheel's motor.
//!
//! Road Fighters 3D (`rf3d.rs`) is `jgt.dll`, run by `wal-avs-boot.exe`: the same two boards
//! (`libacioEx.dll`, its HGTH bus, also the payload).
//!
//! Contra: Evolution imports it under its functions' MSVC names (`libacio.def`) and never calls
//! them: `contra.rs` answers its control panel (COM1), DirectInput keyboard and clock.
//!
//! Wartran Troopers loads it as `panicdialog.dll` (`wartran.rs`: its serial I/O chain and gun
//! boards; `two_screens.rs`: its two monitors in one window).
//!
//! Gashaaaan Refill! loads it as `libbms.dll` (its BM-System client, `bms.rs`), from where
//! `ig2.rs` answers the game's own I/O library, ig2ACLib; `crosshair.rs` shows the players' aim.
//!
//! Gashaaaan loads it as `iob.dll` (its I/O board library, `f24.rs`), which also answers the
//! game's camera board library, ballio.
//!
//! Lethal Enforcers 3 loads it as `iob.dll` too (`le3.rs`), beside its gun boards' gunio.dll;
//! its two players' monitors are `two_screens.rs`'s.
//!
//! The 64-bit build (`wal_konami64.dll`) is Chase Chase Jokers' AIO libraries (`ccj.rs`: the
//! BI2X TBS I/O board; `ccj_trackball.rs`: its raw input trackball; `ccj_pointer.rs`: its touch
//! screen; `ccj_mono.rs`: game settings through Mono); the modules above are the 32-bit games'.

#![allow(non_snake_case)]

#[cfg(target_arch = "x86")]
mod acio;
#[cfg(target_arch = "x86")]
mod avs;
#[cfg(target_arch = "x86")]
mod bms;
#[cfg(target_arch = "x86")]
mod contra;
#[cfg(target_arch = "x86")]
mod crosshair;
#[cfg(target_arch = "x86")]
mod eamuse;
#[cfg(target_arch = "x86")]
mod f24;
#[cfg(target_arch = "x86")]
mod f24_screen;
#[cfg(target_arch = "x86")]
mod gti_club;
#[cfg(target_arch = "x86")]
mod hbhi;
#[cfg(target_arch = "x86")]
mod hgth;
#[cfg(target_arch = "x86")]
mod i36i;
#[cfg(target_arch = "x86")]
mod icca;
#[cfg(target_arch = "x86")]
mod ig2;
#[cfg(target_arch = "x86")]
mod le3;
#[cfg(target_arch = "x86")]
mod net;
#[cfg(target_arch = "x86")]
mod otomedius;
#[cfg(target_arch = "x86")]
mod pjec;
#[cfg(target_arch = "x86")]
mod pjei;
#[cfg(target_arch = "x86")]
mod rf3d;
#[cfg(target_arch = "x86")]
mod screen;
#[cfg(target_arch = "x86")]
mod touchpanel;
#[cfg(target_arch = "x86")]
mod two_screens;
#[cfg(target_arch = "x86")]
mod wartran;
#[cfg(target_arch = "x86")]
mod we2012;
#[cfg(target_arch = "x86")]
mod winning_eleven;
#[cfg(target_arch = "x86_64")]
mod ccj;
#[cfg(target_arch = "x86_64")]
mod ccj_mono;
#[cfg(target_arch = "x86_64")]
mod ccj_pointer;
#[cfg(target_arch = "x86_64")]
mod ccj_trackball;

use std::ffi::c_void;

#[cfg(target_arch = "x86")]
use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[cfg(target_arch = "x86")]
pub use acio::*;
#[cfg(target_arch = "x86")]
pub use bms::*;
#[cfg(target_arch = "x86")]
pub use hbhi::*;
#[cfg(target_arch = "x86")]
pub use hgth::*;
#[cfg(target_arch = "x86")]
pub use i36i::*;
#[cfg(target_arch = "x86")]
pub use icca::*;
#[cfg(target_arch = "x86")]
pub use pjec::*;
#[cfg(target_arch = "x86")]
pub use pjei::*;
#[cfg(target_arch = "x86")]
pub use touchpanel::*;
#[cfg(target_arch = "x86_64")]
pub use ccj::*;

#[cfg(target_arch = "x86")]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // Road Fighters 3D: the game is jgt.dll (the common hooks go there), run by wal-avs-boot
        let jgt = rf3d::game_module();
        let we12 = we2012::game_module();
        if jgt != 0 || we12 != 0 {
            wal_payload_common::iat::set_game_module(jgt.max(we12));
        }
        wal_payload_common::start("konami");
        if we12 != 0 {
            we2012::init(module);
            log!("konami: PJEI board, PS2 and PS3 pad ports, card reader and touch screen emulated (Winning Eleven 2012)");
            return TRUE;
        }
        if jgt != 0 {
            rf3d::init(module);
            log!("konami: HBHI board, HGTH wheel board and card reader emulated (Road Fighters 3D)");
            return TRUE;
        }
        if wartran::detected(module) {
            wartran::init();
            return TRUE;
        }
        if le3::detected(module) {
            le3::init();
            return TRUE;
        }
        if f24::detected(module) {
            f24::init();
            return TRUE;
        }
        if file_name(module).eq_ignore_ascii_case("libbms.dll") {
            ig2::init();
            crosshair::init();
            return TRUE;
        }
        if file_name(std::ptr::null_mut()).eq_ignore_ascii_case("otomedius.exe") {
            otomedius::init(module);
            log!("konami: PJEI board, touch screen and BM-System emulated (Otomedius)");
            return TRUE;
        }
        if contra::detected() {
            contra::init();
            log!("konami: control panel, keyboard and clock answered (Contra: Evolution)");
            return TRUE;
        }
        if gti_club::detected() {
            gti_club::init(module);
            log!("konami: HBHI board, HGTH wheel board and card reader emulated (GTI Club)");
            return TRUE;
        }
        if winning_eleven::detected() {
            winning_eleven::init(module);
            log!("konami: PJEI board, PS2 pad port and touch screen emulated (Winning Eleven 2008)");
            return TRUE;
        }
        net::init();
        screen::init();
        avs::init();
        eamuse::init();
        log!("konami: ACIO board and card reader emulated (libacio.dll)");
    }
    TRUE
}

/// Chase Chase Jokers (64-bit): the payload is its three AIO libraries; the I/O board's copy
/// runs it.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH && ccj::detected(module) {
        ccj::init();
    }
    TRUE
}

/// This DLL's file name (what it was installed as), or the executable's for a null module.
fn file_name(module: HINSTANCE) -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}
