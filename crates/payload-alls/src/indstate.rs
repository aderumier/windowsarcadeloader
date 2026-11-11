//! Initial D THE ARCADE Season 5: Story races ended seconds after the start.
//!
//! `cUE4Interface::RetireRace(eRetireRaceType)` (`IndRun.dll+0x5ede0`): its type 1 stores -1 into
//! the race's remaining time (`+0x538` of the race object). The game calls it before the lights,
//! so the clock updater (`0x7b820`) finished the race on its first tick (`+0x668 = 6`, end type 1)
//! with over 100 s left. On entry to the racing state the remaining time is refunded to the
//! largest value seen for the race (the budget the game seeded, `0x7b2e0`); the elapsed stamps are
//! left alone (`GetRaceTime`, `0x5ee60`, subtracts them: rewritten, the game ended the race
//! itself). RetireRace(1) during the race is left alone: it is how the game ends a race at its
//! goal (a Story race's second part, after `RaceReStart`, finishes with it). Season 5's IndRun
//! only (its RetireRace reads the race pointer at `+0x21c720`; Season 3's fields are 8 bytes
//! further on).
//!
//! From Doug Maupin's Season 5 work (github.com/douggle/windowsarcadeloader).

use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};

/// `cUE4Interface::RetireRace`: its first instruction `mov rax,[rip+disp32]` (7 bytes) reads the
/// race pointer (the Season 5 check).
const RETIRE_RVA: usize = 0x5ede0;
const RETIRE_PROLOGUE: usize = 7;
/// The race pointer (`DAT_18021c720`): race object = `[[[[it]+8]]+8]`.
const RACE_RVA: usize = 0x21c720;
/// The race object's state (4 racing) and remaining time (ms).
const STATE: usize = 0x668;
const REMAINING: usize = 0x538;
const RACING: u32 = 4;
unsafe fn readable(p: usize, size: usize) -> bool {
    if p < 0x10000 || p >= 0x8000_0000_0000 {
        return false;
    }
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQuery(p as *const _, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
    if n == 0 || mbi.State != MEM_COMMIT || (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD)) != 0 {
        return false;
    }
    p + size <= mbi.BaseAddress as usize + mbi.RegionSize
}

unsafe fn rd_ptr(p: usize) -> Option<usize> {
    if !unsafe { readable(p, 8) } {
        return None;
    }
    let v = unsafe { std::ptr::read_volatile(p as *const usize) };
    (v >= 0x10000 && v < 0x8000_0000_0000).then_some(v)
}

unsafe fn race_object(race: usize) -> Option<usize> {
    let p0 = unsafe { rd_ptr(race) }?;
    let p1 = unsafe { rd_ptr(p0 + 8) }?;
    let p2 = unsafe { rd_ptr(p1) }?;
    let p3 = unsafe { rd_ptr(p2 + 8) }?;
    unsafe { readable(p3 + STATE, 4) }.then_some(p3)
}

unsafe fn word(p3: usize, off: usize) -> i32 {
    unsafe { std::ptr::read_volatile((p3 + off) as *const i32) }
}

/// `base` is the loaded IndRun.dll.
pub(crate) fn init(base: usize) {
    let entry = base + RETIRE_RVA;
    let race = base + RACE_RVA;
    // Season 5's RetireRace: mov rax,[rip+disp32] reading the race pointer
    let season5 = unsafe { readable(entry, RETIRE_PROLOGUE) } && unsafe {
        let head = std::slice::from_raw_parts(entry as *const u8, RETIRE_PROLOGUE);
        let disp = i32::from_le_bytes([head[3], head[4], head[5], head[6]]);
        head[..3] == [0x48, 0x8B, 0x05] && (entry + RETIRE_PROLOGUE).wrapping_add_signed(disp as isize) == race
    };
    if !season5 {
        return;
    }
    log!("indstate: Season 5 race clock refund on");
    std::thread::spawn(move || {
        let mut last = 0u32;
        let mut seed = 0i32;
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let Some(p3) = (unsafe { race_object(race) }) else {
                last = 0;
                seed = 0;
                continue;
            };
            let state = unsafe { word(p3, STATE) } as u32;
            let rem = unsafe { word(p3, REMAINING) };
            if state <= RACING && rem > seed {
                seed = rem;
            }
            if state == RACING && last != RACING && seed > rem {
                unsafe { std::ptr::write_volatile((p3 + REMAINING) as *mut i32, seed) };
                log!("indstate: green light, remaining time {rem} -> {seed} ms");
            }
            if state != RACING && last == RACING {
                seed = 0;
            }
            last = state;
        }
    });
}
