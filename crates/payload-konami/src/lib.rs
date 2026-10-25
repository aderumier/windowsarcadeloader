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
//! Thrill Drive 4 has the PJEI board (switches, wheel and pedals) and, on a second bus
//! (`libtd4io.dll`, also the payload), the D44H board: the wheel's motor (`td4.rs`).
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
//! Castlevania: The Arcade loads it as `libbms.dll` too, from where `hcv.rs` answers its I/O
//! library, hcvACLib, and feeds the players' whips.
//!
//! Action Deka loads it as `libacio.dll` (`gdki.rs`: its action controllers), from where
//! `deka.rs` answers its libbms, its EZ-USB library and its debug input (the players' guns).
//!
//! Metal Gear Arcade (`mga.rs`) is `launch.dll`, run by `wal-avs-boot.exe`: its `system.dll`'s
//! `libacio.dll` (the I36G cabinet and gun boards, the card reader) and its gun's aim (COM1).
//!
//! Steel Chronicle Victroopers (`sc.rs`) is `arkkgg.dll`, run by `wal-avs-boot.exe`: its
//! `lib_acio.dll`'s `libacio.dll` (the HBHI board: two flight sticks, card reader).
//!
//! Hello! Pop'n Music (`hpm.rs`) is `popn.dll`, run by `wal-avs-boot.exe`: its `libacio.dll`
//! (the HBHI board: two players' start and four buttons).
//!
//! The Bishi Bashi loads it as `libacio.dll` (`ibb.rs`: its HBHI board, three players' four
//! buttons), its libbms imports pointed at the payload.
//!
//! Silent Hill: The Arcade loads it as `libbms.dll` too, from where `shg.rs` points the
//! game's I/O library, shaiolib, at the payload.
//!
//! Gashaaaan loads it as `iob.dll` (its I/O board library, `f24.rs`), which also answers the
//! game's camera board library, ballio.
//!
//! Lethal Enforcers 3 loads it as `iob.dll` too (`le3.rs`), beside its gun boards' gunio.dll;
//! its two players' monitors are `two_screens.rs`'s.
//!
//! Cooper's 9 loads it as `inpout32.dll` (`coopers9.rs`: its parallel port I/O board and HASP
//! dongle), started by wal-loader: the game is a memory dump whose imports the payload rebuilds.
//!
//! Hyper Sports 2011 Remake loads it through wal-loader (`hypersports.rs`): it answers the game's
//! gf.dll imports of its Python 4 I/O board and LED controller, its buttons are the fake
//! DirectInput keyboard.
//!
//! The 64-bit build (`wal_konami64.dll`) is Bishi Bashi Channel's `libacio.dll` and
//! `libacio_pix.dll` (`bsch.rs`: BMPU and PIX boards; `acio.rs`, `icca.rs`), and Chase Chase Jokers' AIO libraries (`ccj.rs`: the
//! BI2X TBS I/O board; `ccj_trackball.rs`: its raw input trackball; `ccj_pointer.rs`: its touch
//! screen; `ccj_mono.rs`: game settings through Mono), Rainbow Bomber Girl's `winhttp.dll`
//! (`bbg.rs`; `bbg_system.rs`: its BM-System library; `bbg_io.rs`: its I/O board and touch
//! screen through Mono, `mono.rs`; `bbg_screen.rs`: its two monitors on one screen), and the
//! `dxva2.dll` of Silent Scope: Bone Eater's widescreen runtime (`ndd.rs`: its keys, gun and
//! scope); the modules above are the 32-bit games'. `ark.rs` is the ARK engine games' (Steel
//! Chronicle, Bone Eater) shared fixes.

#![allow(non_snake_case)]

mod acio;
mod ark;
#[cfg(target_arch = "x86")]
mod avs;
#[cfg(target_arch = "x86")]
mod bms;
#[cfg(target_arch = "x86")]
mod contra;
#[cfg(target_arch = "x86")]
mod coopers9;
#[cfg(target_arch = "x86")]
mod crosshair;
#[cfg(target_arch = "x86")]
mod eamuse;
#[cfg(target_arch = "x86")]
mod deka;
#[cfg(target_arch = "x86")]
mod f24;
#[cfg(target_arch = "x86")]
mod f24_screen;
#[cfg(target_arch = "x86")]
mod gdki;
#[cfg(target_arch = "x86")]
mod gti_club;
#[cfg(target_arch = "x86")]
mod hbhi;
#[cfg(target_arch = "x86")]
mod hcv;
#[cfg(target_arch = "x86")]
mod hgth;
#[cfg(target_arch = "x86")]
mod hpm;
#[cfg(target_arch = "x86")]
mod hypersports;
#[cfg(target_arch = "x86")]
mod i36i;
#[cfg(target_arch = "x86")]
mod ibb;
mod icca;
mod kbt;
#[cfg(target_arch = "x86")]
mod ig2;
#[cfg(target_arch = "x86")]
mod le3;
#[cfg(target_arch = "x86")]
mod mga;
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
mod sc;
#[cfg(target_arch = "x86")]
mod screen;
#[cfg(target_arch = "x86")]
mod shg;
#[cfg(target_arch = "x86")]
mod td4;
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
mod bbg;
#[cfg(target_arch = "x86_64")]
mod bbg_io;
#[cfg(target_arch = "x86_64")]
mod bbg_screen;
#[cfg(target_arch = "x86_64")]
mod bbg_system;
#[cfg(target_arch = "x86_64")]
mod bsch;
#[cfg(target_arch = "x86_64")]
mod ccj;
#[cfg(target_arch = "x86_64")]
mod ccj_mono;
#[cfg(target_arch = "x86_64")]
mod ccj_pointer;
#[cfg(target_arch = "x86_64")]
mod ccj_trackball;
#[cfg(target_arch = "x86_64")]
mod mono;
#[cfg(target_arch = "x86_64")]
mod ndd;

use std::ffi::c_void;

#[cfg(target_arch = "x86")]
use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use acio::*;
#[cfg(target_arch = "x86")]
pub use bms::*;
#[cfg(target_arch = "x86")]
pub use gdki::*;
#[cfg(target_arch = "x86")]
pub use hbhi::*;
#[cfg(target_arch = "x86")]
pub use hgth::*;
#[cfg(target_arch = "x86")]
pub use i36i::*;
pub use icca::*;
pub use kbt::*;
#[cfg(target_arch = "x86")]
pub use pjec::*;
#[cfg(target_arch = "x86")]
pub use pjei::*;
#[cfg(target_arch = "x86")]
pub use td4::*;
#[cfg(target_arch = "x86")]
pub use touchpanel::*;
#[cfg(target_arch = "x86_64")]
pub use bbg_system::*;
#[cfg(target_arch = "x86_64")]
pub use bsch::*;
#[cfg(target_arch = "x86_64")]
pub use ccj::*;

#[cfg(target_arch = "x86")]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // Cooper's 9: the executable's imports are rebuilt before the common hooks look them up
        if coopers9::detected(module) {
            let (resolved, missing) = coopers9::imports();
            wal_payload_common::start("konami");
            coopers9::init(resolved, &missing);
            return TRUE;
        }
        // Hyper Sports 2011 Remake: the window, Direct3D and DirectInput are gf.dll's
        if hypersports::detected() {
            wal_payload_common::iat::set_game_module(hypersports::game_module());
            wal_payload_common::start("konami");
            hypersports::init();
            return TRUE;
        }
        // Steel Chronicle Victroopers: the common payload starts when arkkgg.dll loads gamekgg.dll
        if sc::ark_module() != 0 {
            sc::init(module);
            return TRUE;
        }
        // Road Fighters 3D: the game is jgt.dll (the common hooks go there), run by wal-avs-boot
        let jgt = rf3d::game_module();
        let we12 = we2012::game_module();
        let popn = hpm::game_module();
        let mga = mga::game_module();
        if jgt != 0 || we12 != 0 || popn != 0 || mga != 0 {
            wal_payload_common::iat::set_game_module(jgt.max(we12).max(popn).max(mga));
        }
        wal_payload_common::start("konami");
        if mga != 0 {
            mga::init(module);
            log!("konami: I36G cabinet and gun boards, gun aim and card reader emulated (Metal Gear Arcade)");
            return TRUE;
        }
        if popn != 0 {
            hpm::init(module);
            log!("konami: HBHI board and card reader emulated (Hello! Pop'n Music)");
            return TRUE;
        }
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
        if shg::detected(module) {
            shg::init();
            return TRUE;
        }
        if file_name(module).eq_ignore_ascii_case("libbms.dll") && file_name(std::ptr::null_mut()).eq_ignore_ascii_case("HCV.exe") {
            hcv::init();
            return TRUE;
        }
        if file_name(module).eq_ignore_ascii_case("libbms.dll") {
            ig2::init();
            crosshair::init();
            return TRUE;
        }
        if deka::detected() {
            deka::init(module);
            log!("konami: GDKI action controllers, card reader and BM-System emulated (Action Deka)");
            return TRUE;
        }
        if ibb::detected() {
            ibb::init(module);
            log!("konami: HBHI board, card reader and BM-System emulated (The Bishi Bashi)");
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
        if td4::detected() {
            td4::init(module);
            log!("konami: PJEI board, D44H wheel motor board and card reader emulated (Thrill Drive 4)");
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
/// runs it. Bishi Bashi Channel: its `libacio.dll` and `libacio_pix.dll`; the first runs it.
/// Rainbow Bomber Girl: its `winhttp.dll`. Silent Scope: Bone Eater: its runtime's `dxva2.dll`.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH && bbg::detected(module) {
        bbg_system::set_module(module as usize);
        bbg::init();
    } else if reason == DLL_PROCESS_ATTACH && ccj::detected(module) {
        ccj::init();
    } else if reason == DLL_PROCESS_ATTACH && bsch::detected(module) {
        bsch::init(module);
    } else if reason == DLL_PROCESS_ATTACH && ndd::detected(module) {
        ndd::init();
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
