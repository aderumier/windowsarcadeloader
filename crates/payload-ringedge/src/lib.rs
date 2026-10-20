//! Sega RingEdge payload, loaded by `wal-loader` (the games link Sega's amLib statically and
//! import no driver DLL).
//!
//! * The kernel drivers amLib opens: SRAM, SMBus (EEPROM, DIP switches), super I/O, DMI
//!   (`devices.rs`), saved in `WAL_RINGEDGE_SAVE` (default `sv`).
//! * The system services it asks over PCP: keychip and master (`pcp.rs`).
//! * The JVS I/O board (Sega 837-14572) on the JVS driver `\\.\mxjvs` (`WAL_TYPEX_JVS_PORT`:
//!   `COM4` for games using the serial port), the board shared with Type X (payload-common
//!   `jvs_board.rs`).
//! * amLib's own log (`WAL_RINGEDGE_AMLOG`, `amlog.rs`).
//! * The card daemon and its magnetic card reader (`WAL_RINGEDGE_CARD_DAEMON`, `card.rs`).
//! * A CTR-251500 serial touch panel (`WAL_RINGEDGE_TOUCH`, `touch.rs`), a 3M MicroTouch one
//!   (`WAL_RINGEDGE_MICROTOUCH`, `microtouch.rs`).
//! * Medal cabinet boards: Medal no Gunman's (`gunman.rs`), Lady Luck's (`satellite.rs`).
//! * Profile code patches (`WAL_PATCHES`); `WAL_RINGEDGE_CWD`: the game's working directory,
//!   relative to its own.

#![allow(non_snake_case)]

mod amlog;
mod card;
mod cg;
mod devices;
mod ffb;
mod happy_playground;
mod glscale;
mod gunman;
mod microtouch;
mod nvasm;
mod pcp;
mod rfid;
mod satellite;
mod touch;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use wal_payload_common::{log, patches, serial};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // the game's card daemon (picodaemon.exe): its card reader only
        if card::daemon() {
            return TRUE;
        }
        wal_payload_common::start("ringedge");
        // the game's working directory, relative to its own (its files are found from there)
        if let Ok(dir) = std::env::var("WAL_RINGEDGE_CWD") {
            let ok = std::env::set_current_dir(dir.trim()).is_ok();
            log!("ringedge: working directory {} ({:?})", dir.trim(), if ok { std::env::current_dir().ok() } else { None });
        }
        patches::apply();
        zero_heap();
        // WAL_RINGEDGE_OFFLINE=1: the game's network check, ALL.Net and the in-shop network
        // skipped, with its own patches (WAL_RINGEDGE_OFFLINE_PATCHES, the WAL_PATCHES format)
        if std::env::var("WAL_RINGEDGE_OFFLINE").is_ok_and(|v| v.trim() == "1")
            && let Ok(list) = std::env::var("WAL_RINGEDGE_OFFLINE_PATCHES")
        {
            log!("ringedge: offline cabinet");
            patches::apply_list(&list);
        }
        amlog::init();
        // JVS sense line: amJvs reads DSR (0x20) as "board addressed"
        serial::MODEM_STATUS[0].store(0x10, Ordering::Relaxed);
        serial::MODEM_STATUS[1].store(0x30, Ordering::Relaxed);
        wal_payload_common::jvs_board::init("mxjvs");
        happy_playground::init();
        // ports the game opens beside its I/O board, present but silent
        if let Ok(ports) = std::env::var("WAL_RINGEDGE_SILENT_PORTS") {
            serial::install_sink(&ports);
        }
        devices::init();
        cg::init();
        ffb::init();
        card::start_daemon();
        touch::init();
        gunman::init();
        satellite::init();
        microtouch::init();
        pcp::start();
        log!("ringedge: initialized");
    }
    TRUE
}

static ORIG_ALLOCATE_HEAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `WAL_RINGEDGE_ZERO_HEAP=1` (debugging): every heap block zeroed (`HEAP_ZERO_MEMORY` added to
/// `RtlAllocateHeap`), for games reading fields they never set.
fn zero_heap() {
    if !std::env::var("WAL_RINGEDGE_ZERO_HEAP").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    match unsafe { cg::hotpatch(c"ntdll.dll", c"RtlAllocateHeap", allocate_heap as *const () as usize) } {
        Some(o) => {
            ORIG_ALLOCATE_HEAP.store(o, Ordering::Relaxed);
            log!("ringedge: heap blocks zeroed");
        }
        None => log!("ringedge: ntdll.dll!RtlAllocateHeap not hot patchable"),
    }
}

unsafe extern "system" fn allocate_heap(heap: *mut c_void, flags: u32, size: usize) -> *mut c_void {
    let orig: unsafe extern "system" fn(*mut c_void, u32, usize) -> *mut c_void = unsafe { std::mem::transmute(ORIG_ALLOCATE_HEAP.load(Ordering::Relaxed)) };
    // HEAP_ZERO_MEMORY
    unsafe { orig(heap, flags | 0x8, size) }
}
