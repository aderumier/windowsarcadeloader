//! Initial D THE ARCADE Season 5: Story races ended seconds after the start.
//!
//! `cUE4Interface::RetireRace(eRetireRaceType)` (`IndRun.dll+0x5ede0`): its type 1 stores -1 into
//! the race's remaining time (`+0x538` of the race object). The game calls it before the lights,
//! so the clock updater (`0x7b820`) finished the race on its first tick (`+0x668 = 6`, end type 1)
//! with over 100 s left. Two fixes, on Season 5's IndRun only (its RetireRace reads the race
//! pointer at `+0x21c720`; Season 3's fields are 8 bytes further on):
//! - on entry to the racing state the remaining time is refunded to the largest value seen for
//!   the race (the budget the game seeded, `0x7b2e0`); the elapsed stamps are left alone
//!   (`GetRaceTime`, `0x5ee60`, subtracts them: rewritten, the game ended the race itself);
//! - RetireRace(1) is ignored while the race is racing: the game sends it at random moments of a
//!   race (1.9 s to 58 s after the lights seen, nothing in its log; a Time Attack ended at its
//!   TIME UP, a Story race's second part never reached its line). The race ends by IndRun itself:
//!   its goal (state 5) or its clock (state 6). Calls outside the racing state and types 2 / 3
//!   go to the original.
//!
//! From Doug Maupin's Season 5 work (github.com/douggle/windowsarcadeloader).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS,
    VirtualAlloc, VirtualProtect, VirtualQuery,
};

/// `cUE4Interface::RetireRace`; its first instruction `mov rax,[rip+disp32]` (7 bytes) reads the
/// race pointer.
const RETIRE_RVA: usize = 0x5ede0;
const RETIRE_PROLOGUE: usize = 7;
/// The race pointer (`DAT_18021c720`): race object = `[[[[it]+8]]+8]`.
const RACE_RVA: usize = 0x21c720;
/// The race object's state (4 racing) and remaining time (ms).
const STATE: usize = 0x668;
const REMAINING: usize = 0x538;
const RACING: u32 = 4;

static RETIRE_ORIG: AtomicUsize = AtomicUsize::new(0);
static RACE_STATE: AtomicU32 = AtomicU32::new(0);

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

unsafe extern "system" fn retire_hook(this: usize, kind: u32) {
    let state = RACE_STATE.load(Ordering::Relaxed);
    let ignore = kind == 1 && state == RACING;
    log!("indstate: RetireRace({kind}) state {state}: {}", if ignore { "ignored" } else { "passed" });
    if ignore {
        return;
    }
    let orig: unsafe extern "system" fn(usize, u32) = unsafe { std::mem::transmute(RETIRE_ORIG.load(Ordering::Relaxed)) };
    unsafe { orig(this, kind) }
}

/// An executable page within ±2 GiB of `base`, for the entry's `jmp rel32`.
unsafe fn alloc_near(base: usize) -> Option<usize> {
    let mut delta = 0x0100_0000usize;
    while delta < 0x6000_0000 {
        for cand in [base.checked_sub(delta), base.checked_add(delta)].into_iter().flatten() {
            let p = unsafe { VirtualAlloc(cand as *const _, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) };
            if !p.is_null() {
                return Some(p as usize);
            }
        }
        delta += 0x0100_0000;
    }
    None
}

/// RetireRace's entry to `retire_hook`; the trampoline runs the replaced prologue as
/// `mov rax,[moffs64]` and jumps back.
unsafe fn hook_retire(base: usize, race: usize) -> bool {
    let entry = base + RETIRE_RVA;
    let Some(page) = (unsafe { alloc_near(base) }) else { return false };
    unsafe {
        let mut t = [0u8; 15];
        t[0] = 0x48;
        t[1] = 0xA1;
        t[2..10].copy_from_slice(&(race as u64).to_le_bytes());
        t[10] = 0xE9;
        let back = (entry + RETIRE_PROLOGUE) as isize - (page + 15) as isize;
        t[11..15].copy_from_slice(&(back as i32).to_le_bytes());
        std::ptr::copy_nonoverlapping(t.as_ptr(), page as *mut u8, t.len());
        // mov rax, retire_hook; jmp rax
        let stub = page + 0x40;
        let mut s = [0u8; 12];
        s[0] = 0x48;
        s[1] = 0xB8;
        s[2..10].copy_from_slice(&(retire_hook as *const () as u64).to_le_bytes());
        s[10] = 0xFF;
        s[11] = 0xE0;
        std::ptr::copy_nonoverlapping(s.as_ptr(), stub as *mut u8, s.len());
        RETIRE_ORIG.store(page, Ordering::Relaxed);

        let dist = stub as isize - (entry + 5) as isize;
        if dist < i32::MIN as isize || dist > i32::MAX as isize {
            return false;
        }
        let mut old = 0;
        if VirtualProtect(entry as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::write_volatile(entry as *mut u8, 0xE9);
        std::ptr::write_unaligned((entry + 1) as *mut i32, dist as i32);
        VirtualProtect(entry as *const _, 5, old, &mut old);
    }
    true
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
    if !unsafe { hook_retire(base, race) } {
        log!("indstate: RetireRace not hooked");
        return;
    }
    log!("indstate: Season 5 race clock fixes on");
    std::thread::spawn(move || {
        let mut last = 0u32;
        let mut seed = 0i32;
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let Some(p3) = (unsafe { race_object(race) }) else {
                last = 0;
                seed = 0;
                RACE_STATE.store(0, Ordering::Relaxed);
                continue;
            };
            let state = unsafe { word(p3, STATE) } as u32;
            let rem = unsafe { word(p3, REMAINING) };
            if state != last {
                // the goal test (+0x7abb6) needs +0x1000 == +0x10f4 (10 each from a clean verdict)
                let (goal, need, verdict) = unsafe { (word(p3, 0x1000), word(p3, 0x10f4), word(p3, 0x1120)) };
                log!("indstate: race {p3:#x} state {last} -> {state}, remaining {rem} ms, goal counters {goal}/{need}, verdict {verdict}");
            }
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
            RACE_STATE.store(state, Ordering::Relaxed);
        }
    });
}

