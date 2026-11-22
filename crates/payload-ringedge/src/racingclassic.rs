//! Sega Racing Classic's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`). Its
//! link (Sega's comm library, UDP only): node k (MAIN 0, a SUB its CAR NUMBER less one) is
//! 192.168.75.(1 + k); the SUBs write to MAIN's UDP 26666, MAIN answers with the nodes'
//! addresses, then each sends its frames to every node's UDP 26665. Its test menu's GAME
//! ASSIGNMENTS (START UP MODE 0 off / 1 MAIN / 2 SUB, CAR NUMBER, TOTAL NODES) are options of its
//! settings object (`0x834dd8`: a slot per option, each an object whose method 1 returns the
//! stored choice, an index in its table of values). Linked, those three options get a copy of
//! their method table whose method 1 returns the choice of the link's value (cabinet 1 MAIN of
//! `WAL_LINK_CABINETS` nodes, the others SUB, CAR NUMBER their number), so the test menu has
//! nothing to set; its 192.168.75 addresses are the cabinets' PCs and amNetwork gives it its own.

use std::time::{Duration, Instant};

use wal_payload_common::{link, log, patches};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

/// The settings object: its slots (each a pointer to an option object).
const SETTINGS: usize = 0x834DD8;
/// `mov eax, 0x834dd8; ret`: the settings object's getter, as the known build has it.
const GETTER: (usize, [u8; 6]) = (0x41B9F0, [0xB8, 0xD8, 0x4D, 0x83, 0x00, 0xC3]);
/// The slots of START UP MODE, CAR NUMBER, TOTAL NODES.
const MODE: usize = 0;
const NODE: usize = 4;
const TOTAL: usize = 0x14;

/// The option at `slot`: the choice (its index) whose value is `value` (the table: an option's
/// +0x8 / +0xC choices' bounds, +0x10 the values, 8 bytes each).
unsafe fn choice(option: usize, value: u32) -> Option<u8> {
    unsafe {
        let (lo, hi, table) = (*((option + 8) as *const u32), *((option + 12) as *const u32), *((option + 16) as *const usize));
        if table == 0 || hi <= lo || hi - lo > 64 {
            return None;
        }
        (lo..hi).find(|&i| *((table + i as usize * 8) as *const u32) == value).map(|i| i as u8)
    }
}

/// The option at `slot` made to return `index` as its choice: a copy of its method table, method
/// 1 a stub `mov al, index; ret`.
unsafe fn force(slot: usize, index: u8) -> bool {
    unsafe {
        let option = *((SETTINGS + slot) as *const usize);
        let vtable = *(option as *const usize);
        let page = VirtualAlloc(std::ptr::null(), 0x100, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut usize;
        if page.is_null() {
            return false;
        }
        // 16 methods copied; the stub after them
        std::ptr::copy_nonoverlapping(vtable as *const usize, page, 16);
        let stub = page.add(16) as *mut u8;
        std::ptr::copy_nonoverlapping([0xB0, index, 0xC3].as_ptr(), stub, 3);
        *page.add(1) = stub as usize;
        *(option as *mut usize) = page as usize;
        true
    }
}

pub(crate) fn init() {
    if !patches::exe_name().eq_ignore_ascii_case("d1a.exe")
        || !wal_payload_common::crash::readable(GETTER.0, GETTER.1.len())
        || unsafe { std::slice::from_raw_parts(GETTER.0 as *const u8, GETTER.1.len()) } != GETTER.1
    {
        return;
    }
    if link::init() == 0 {
        return;
    }
    let (n, k) = (link::cabinets().unwrap_or(2).min(4), link::cabinet());
    if k > n {
        log!("racingclassic: cabinet {k} of {n} (WAL_LINK_CABINETS): its GAME ASSIGNMENTS left as they are");
        return;
    }
    link::virtual_net([192, 168, 75], 1);
    crate::pcp::set_game_link_address(std::net::Ipv4Addr::new(192, 168, 75, k as u8));
    // its options are made at its start: forced as soon as they are there
    std::thread::spawn(move || {
        let start = Instant::now();
        let ready = || [MODE, NODE, TOTAL].iter().all(|s| unsafe { *((SETTINGS + s) as *const usize) } != 0);
        while !ready() {
            if start.elapsed() > Duration::from_secs(60) {
                return log!("racingclassic: its GAME ASSIGNMENTS never made: left as they are");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // the MAIN has no CAR NUMBER (node 0)
        let wanted: [(usize, u32, &str); 3] = [(MODE, if k == 1 { 1 } else { 2 }, "START UP MODE"), (NODE, k as u32 - 1, "CAR NUMBER"), (TOTAL, n as u32, "TOTAL NODES")];
        for (slot, value, name) in wanted.into_iter().filter(|&(slot, _, _)| slot != NODE || k > 1) {
            let option = unsafe { *((SETTINGS + slot) as *const usize) };
            match unsafe { choice(option, value) } {
                Some(i) if unsafe { force(slot, i) } => log!("racingclassic: {name} {value} (choice {i})"),
                _ => log!("racingclassic: {name} {value} not among its choices: left as it is"),
            }
        }
        log!("racingclassic: linked, {} of {n} nodes, 192.168.75.{k}", if k == 1 { "MAIN".to_string() } else { format!("SUB, CAR NUMBER {k}") });
    });
}
