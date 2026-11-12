//! Sega ALLS payload: the cabinet system the game and `amdaemon.exe` run on (Mario & Sonic at
//! the Tokyo 2020 Olympic Games Arcade, House of the Dead: Scarlet Dawn, Sega World Drivers
//! Championship, Initial D THE ARCADE).
//!
//! An ALLS game talks to `amdaemon.exe` (keychip, ALL.Net, the USB I/O board, credits), which
//! talks to the hardware. Both programs are started by `wal-loader` with this payload (the
//! profile's `background` list starts amdaemon first). In amdaemon (`daemon.rs`) it writes
//! the cabinet's system registry values, emulates the keychip driver, the hardware monitor and
//! the "Type 4" USB I/O board (`io4.rs`, fed by the launcher's sticks). In the game it filters
//! the real keyboard and pads and answers the LED board on COM1 (`led.rs`).
//!
//! Payload options (profile `env`):
//! - `WAL_ALLS_GAME_ID` (4 letters), `WAL_ALLS_PLATFORM_ID` (default `ACA1`),
//!   `WAL_ALLS_KEYCHIP` (default `A69E-01A88888888`), `WAL_ALLS_REGION` (1 Japan, 4 export,
//!   8 China; default 1), `WAL_ALLS_SUBNET` (default `192.168.149.0`): the keychip's values;
//!   `WAL_ALLS_PLATFORM_NAME` (default `ALLS HX2`): the platform's name
//! - `WAL_ALLS_DIPSW`: the system DIP switches 1-8 (bits 0-7, hex; default 01: cabinet 1),
//!   `WAL_ALLS_FREEPLAY` (else the common `WAL_FREEPLAY`, default on): in the system backup
//!   (`sysfile.rs`, from amdaemon's second start)
//! - `WAL_ALLS_ALLNET=1`: ALL.Net authentication accepted (`allnet.rs`; default refused: the game
//!   plays offline)
//! - `WAL_ALLS_CLOCK=0`: the real time zone (default Japan's, `clock.rs`); `WAL_ALLS_TIMEWARP=1`:
//!   the nightly maintenance hours (Japan 02:00-07:00) skipped
//! - `WAL_ALLS_BOARD`: the game's controls (`io4.rs`; `hotd`: also the gun sensor board on
//!   COM1, `gunsensor.rs`, instead of the LED board; `swdc`: the force feedback board on COM1,
//!   `ffb.rs`, the LED board on COM2, the dashboard on COM20, `dashboard.rs`, the main
//!   cabinet's LAN announcements, `udp.rs`, the English translation, `english.rs`; `idac`: the
//!   force feedback board on COM1, the 837-15070 LED board on COM2, the game server, `idac.rs`;
//!   `hkb`: Chrono Regalia's touch screen as the mouse, its game server, `hkb.rs`; `idz`:
//!   Initial D ARCADE STAGE Zero, a Nu cabinet: the force feedback board on COM1, the 837-15070
//!   LED board on COM11, the game server, `idz.rs`)
//! - `WAL_ALLS_BILLING=1`: ALL.Net's billing server for amdaemon's accounting (`billing.rs`)
//! - `WAL_ALLS_CHILDREN`: programs the game starts, started with the payload (`child.rs`)
//! - `WAL_ALLS_IDZ_ENGLISH=1`: Initial D ARCADE STAGE Zero in English (`idzenglish.rs`)
//! - `WAL_ALLS_INI_FILE`: a settings file read as an edited copy (`inifile.rs`)
//! - `WAL_ALLS_STAT_BYPASS`: files whose C runtime `_stat64i32` succeeds (`stat.rs`)
//! - `WAL_ALLS_AIME_PORT` (`COM3`...): the Aime card reader amdaemon opens there (`aime.rs`),
//!   `WAL_ALLS_ACCESS_CODE` the player's card, `WAL_ALLS_AUTO_CARD=1` touched by itself after
//!   start (`WAL_ALLS_AUTO_CARD_DELAY_MS` later)
//! - `WAL_ALLS_HOST`, `WAL_ALLS_ROUTER`: the cabinet's and the router's last address bytes
//!   (default .205, .1), `WAL_ALLS_GAME_ADAPTER=1`: the game sees that adapter too (`netenv.rs`)
//! - `WAL_ALLS_TITLE_URI`, `WAL_ALLS_TITLE_HOST`: ALL.Net's uri and host for the game server
//! - `WAL_ALLS_JVS=1`: a Nu cabinet: amdaemon reads the JVS I/O board on the AMEX board
//!   (payload-common's `amex`, its EEPROM and SRAM in `Y:\amex`) instead of the USB I/O board
//! - `WAL_ALLS_LED_PORT`: the LED board's serial port (default `COM1`)
//! - `WAL_ALLS_UDP=1`: the game hears its own datagrams to the subnet (`udp.rs`; `swdc` sets it)
//! - `WAL_ALLS_CTS=1`: Sega's CTS game server (`cts.rs`: rankings, the Aime players' names and
//!   saves; Mario & Sonic at Rio 2016)
//! - `WAL_ALLS_BOARD=chuni`: CHUNITHM, a Nu game with amLib linked in and no amdaemon: the
//!   system's devices (`daemon.rs`: keychip, AMEX board with `WAL_ALLS_JVS=1`, ALL.Net) in the
//!   game's own process with its slider, air sensors, LED boards and title server (`chuni.rs`);
//!   in `aimeReaderHost.exe` the Aime reader only (`WAL_ALLS_AIME_PORT`, set for that program
//!   with `WAL_EXE_ENV`)
//! - `WAL_ALLS_BOARD=cxb`: crossbeats REV. SUNRiSE, a Nu game with amLib linked in and no
//!   amdaemon (as `chuni`): its REV I/O and LED libraries, touch screen (`cxb.rs`)
//! - `WAL_ALLS_BOARD=chusan`: CHUNITHM SUPERSTAR (amdaemon, IO4: the air sensors; the game:
//!   slider, LED boards; the title server in amdaemon's ALL.Net server, `chuni.rs`)
//! - `WAL_ALLS_LED_BOARD`: the LED boards' identity (`led.rs`)
//! - `WAL_ALLS_AMLOG`, `WAL_ALLS_AMLOG_LEVELS`: the game's linked amLib's log (CHUNITHM;
//!   payload-common's `amlog`)
//! - `WAL_ALLS_DAEMON_PATCHES`: amdaemon's patches (`<rva>:<hex bytes>,...`)
//! - `WAL_PATCHES`: game code patches (same format)
//!
//! Network (`netenv.rs`, `allnet.rs`): one adapter in the keychip's subnet, the store router's
//! names resolved, ALL.Net answered by the payload's server.

mod aime;
mod aimedb;
mod allnet;
mod billing;
mod child;
mod chuni;
mod clock;
mod cts;
mod cxb;
mod daemon;
mod dashboard;
mod english;
mod ffb;
mod gore;
mod gs;
// House of the Dead: Scarlet Dawn's (a 64-bit game: its hook is x86-64 code)
#[cfg(target_arch = "x86_64")]
mod gunsensor;
mod hkb;
mod hotd;
mod icf;
mod idac;
mod idz;
mod idzenglish;
mod indrun;
// IndRun.dll is 64-bit
#[cfg(target_arch = "x86_64")]
mod indstate;
mod inifile;
mod io4;
mod led;
mod netenv;
mod pki;
mod registry;
mod stat;
mod sysfile;
mod udp;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static mut MODULE: HINSTANCE = std::ptr::null_mut();

/// This payload's own module (amdaemon loads its video library from it: `amvideo`).
pub(crate) fn module() -> HINSTANCE {
    unsafe { MODULE }
}

fn is_daemon() -> bool {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).is_some_and(|n| n == "amdaemon.exe")
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { MODULE = module };
        // a packed program (CHUNITHM SUPERSTAR): started once it is decrypted
        if !wal_payload_common::packed::defer(start) {
            start();
        }
    }
    1
}

/// The payload's start: the system's hooks in amdaemon or the game.
fn start() {
    {
        wal_payload_common::start("alls");
        if std::env::var("WAL_ALLS_CLOCK").map_or(true, |v| v.trim() != "0") { clock::init(); }
        stat::init();
        if is_daemon() {
            daemon::init();
            // ALL.Net's billing server (amdaemon's accounting: Initial D ARCADE STAGE Zero plays
            // with Aime cards only after it checked in)
            if std::env::var("WAL_ALLS_BILLING").is_ok_and(|v| v.trim() == "1") {
                allnet::serve_billing();
            }
            // Initial D ARCADE STAGE Zero: its start button waits for the game's configuration
            if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "idz") {
                idz::hold_start();
            }
        } else if child::is_child() {
            // a program the game started (Initial D ARCADE STAGE Zero's store server): the
            // cabinet's network only, its windows hidden
            child::hide_windows();
            child::peers_from_cabinet();
            udp::init();
            netenv::init_game();
            allnet::init_redirect();
        } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "chuni") {
            chuni_process();
        } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "cxb") {
            cxb_process();
        } else {
            let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
            // the cabinet's input comes from amdaemon's I/O board: the real keyboard and pads
            // never reach the game
            wal_payload_common::input_filter::init(exe, |_| false);
            wal_payload_common::patches::apply();
            if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "hotd") {
                // COM1: the gun sensor board; COM22 the LED controller, absent
                #[cfg(target_arch = "x86_64")]
                gunsensor::init();
                hotd::init();
                gore::init();
                wal_payload_common::serial::install_sink("!COM22");
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "swdc") {
                // the main cabinet (StartApp.bat's marker, read by the game too: else it waits
                // for a main cabinet's announcement on the LAN)
                let _ = std::fs::create_dir_all("Y:\\SDDS");
                if !std::path::Path::new("Y:\\SDDS\\LanServer.dat").exists() {
                    let _ = std::fs::write("Y:\\SDDS\\LanServer.dat", b"");
                }
                // the main cabinet's announcements on the LAN, which it must hear itself
                udp::init();
                english::init();
                gs::init();
                // COM1: the force feedback board, COM2 the LED board, COM20 the dashboard
                ffb::init();
                led::init("COM2");
                dashboard::init();
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "idac") {
                // the game server (Aime card players), the cabinet's Windows (segatools' indrun)
                idac::init();
                indrun::init();
                // the game server's echo servers (its NAT check)
                udp::init_echo();
                // COM1: the force feedback board (Sega World Drivers Championship's), COM2 the
                // 837-15070 LED board
                ffb::init();
                led::init_15070("COM2");
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "idz") {
                // Initial D ARCADE STAGE Zero (segatools' idzhook): its game server (Aime card
                // players), COM1 the force feedback board (Sega World Drivers Championship's),
                // COM11 the 837-15070 LED board
                idz::init();
                // its settings file's screen size, the English texts and pictures
                inifile::init();
                idzenglish::init();
                // its store server (cabinet 1) started with the payload
                child::init();
                // its store server found on the subnet, its game server's echo servers (minime's
                // ping ports), its news pages on the router's port 80
                udp::init_with_echo();
                gs::init_plain();
                ffb::init();
                led::init_15070("COM11");
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "hkb") {
                // Chrono Regalia: the touch screen (the mouse), the touch panel and LED board
                // off in its configuration; the game server (Aime card players), Mono's clock
                hkb::init();
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "chusan") {
                // CHUNITHM SUPERSTAR (amdaemon's IO4 and Aime reader): the slider on COM1, the
                // LED boards on COM2 / COM3 (a CVT cabinet)
                chuni::init_chusan();
            } else {
                // the LED board's port (Mario & Sonic at Rio 2016: COM12)
                led::init(std::env::var("WAL_ALLS_LED_PORT").as_deref().map_or("COM1", str::trim));
            }
            if std::env::var("WAL_ALLS_CTS").is_ok_and(|v| v.trim() == "1") {
                cts::init();
            }
            // the cabinet's own LAN announcements heard (Mario & Sonic at Rio 2016's main cabinet
            // on UDP 20199)
            if std::env::var("WAL_ALLS_UDP").is_ok_and(|v| v.trim() == "1") {
                udp::init();
            }
            netenv::init_game();
            if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "chusan") {
                // its title server is amdaemon's (in ALL.Net's server)
                allnet::init_title_redirect();
            } else {
                allnet::init_redirect();
            }
        }
    }
}

/// CHUNITHM's programs: the game (its devices first: the system's hooks pass the serial calls
/// on to them) and the Aime reader's host.
fn chuni_process() {
    let exe_name = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
    if exe_name == "aimereaderhost.exe" {
        aime::init();
        // it reaches AimeDB itself (`aime.naominet.jp`: the router's port 22345, the game's
        // AimeDB server)
        netenv::init_game();
        allnet::init_redirect();
        return;
    }
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    wal_payload_common::patches::apply();
    wal_payload_common::amlog::init("ALLS");
    chuni::init();
    daemon::init();
}

/// crossbeats REV.'s game: its REV I/O and LED libraries and touch screen (`cxb.rs`), the
/// system's devices (the Aime reader on COM12 among them) in its own process.
fn cxb_process() {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    wal_payload_common::patches::apply();
    wal_payload_common::amlog::init("ALLS");
    cxb::init();
    daemon::init();
    // its amLib checks in with ALL.Net's billing server itself (else error 8113, the billing
    // mode unknown)
    allnet::serve_billing();
}

// amvideo: the cabinet's video driver library amdaemon loads (registry `amVideo\name`): the
// screen mode is the launcher's

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoOpen(_ctx: *mut c_void) -> i32 {
    wal_payload_common::log!("alls: amDllVideoOpen");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoClose(_ctx: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoSetResolution(_ctx: *mut c_void, _param: *mut c_void) -> i32 {
    wal_payload_common::log!("alls: amDllVideoSetResolution");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoGetVBiosVersion(_ctx: *mut c_void, dest: *mut u8, size: usize) -> i32 {
    let version = b"01.02.03.04.05\0";
    if !dest.is_null() && size >= version.len() {
        unsafe { std::ptr::copy_nonoverlapping(version.as_ptr(), dest, version.len()) };
    }
    0
}
