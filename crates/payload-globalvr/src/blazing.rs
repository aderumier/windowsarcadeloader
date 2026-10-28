//! Blazing Angels: Squadrons of WWII (`MainR.exe`): the cabinet's Hercules monitors.
//!
//! The game asks the cabinet's monitor programs over shared memory messages and waits up to 1 s
//! for each answer: without them every frame waited several seconds (the attract drew a frame
//! every 5 s and read the controls as rarely). Its client functions (cdecl, a status returned:
//! 0 ok, 1 not enough coins) are replaced:
//! * the coin monitor (`GVRCoinMonitor`): the coins available (0x7da), taken at each start or
//!   continue (0x7d4), its audit counters (0x7fb current, 0x7fd lifetime, 0x7fc reset), board
//!   version (0x7fa), service coins (0x7d5): a coin bank fed by the virtual sticks' coin button;
//! * the volume monitor (`GVRVolumeMonitor`): the volume the game sets, read back.
//!
//! The players are the game's PC profiles (folders of `Documents\BA\UserDataRegion`, made by its
//! PC name entry page): the arcade game takes the first one as its player, none made a start
//! crash (no player). The payload makes an `Arcade` profile when there is none, and a profile
//! whose save (`user.cab`) is missing is taken as a new one (it read as "damaged" and stopped the
//! game): the game writes the save at the end of a mission.
//!
//! The HASP dongle (its API linked in the game, read when a game starts, else "Valid dongle not
//! present!"): the reader of its memory (0x6275e0) fills the dongle record the game checks.
//!
//! The controls are the GVRIO board, a DirectInput joystick (profile `WAL_DINPUT_WHEEL`). The PC
//! game also reads the real keyboard (`GetAsyncKeyState`, its window messages), a key pressed
//! made it drop the joystick: the input filter hides the real keyboard.
//!
//! `WAL_GLOBALVR_BLAZING_TRACE=1`: the controller's input values the game reads (debug).

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use wal_payload_common::log;
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::Shell::{CSIDL_PERSONAL, SHGetFolderPathW};

const IMAGE_SIZE: usize = 0x422000;

/// The coins inserted, not taken yet.
static BANK: AtomicI32 = AtomicI32::new(0);
/// The coin monitor's counters (coins, bills, service...): current and lifetime.
static CURRENT: [AtomicI32; 5] = [const { AtomicI32::new(0) }; 5];
static LIFETIME: [AtomicI32; 5] = [const { AtomicI32::new(0) }; 5];
/// The volume monitor's volume (0..=1000), 100 % until the game sets it.
static VOLUME: AtomicI32 = AtomicI32::new(1000);
/// The board version answered (shown by the operator menu).
const BOARD_VERSION: i32 = 0x0303;

const OK: i32 = 0;
const INSUFFICIENT_COINS: i32 = 1;

unsafe fn put(out: *mut i32, value: i32) {
    if !out.is_null() {
        unsafe { out.write(value) };
    }
}

fn counter(index: i32) -> Option<usize> {
    usize::try_from(index).ok().filter(|&i| i < CURRENT.len())
}

/// 0x7da: the coins available.
extern "C" fn coins(out: *mut i32) -> i32 {
    unsafe { put(out, BANK.load(Ordering::Relaxed)) };
    OK
}

/// 0x7d4: takes `n` coins (a start or a continue).
extern "C" fn take(n: i32) -> i32 {
    let taken = BANK.try_update(Ordering::Relaxed, Ordering::Relaxed, |bank| (bank >= n).then(|| bank - n));
    log!("blazing: {n} coin(s) taken{}", if taken.is_ok() { "" } else { ": not enough" });
    if taken.is_ok() { OK } else { INSUFFICIENT_COINS }
}

/// 0x7d5: the service coins (none).
extern "C" fn service_coins(out: *mut i32) -> i32 {
    unsafe { put(out, 0) };
    OK
}

/// 0x7fa: the I/O board version.
extern "C" fn version(out: *mut i32) -> i32 {
    unsafe { put(out, BOARD_VERSION) };
    OK
}

/// 0x7f9: the meter counters (none).
extern "C" fn meter(_index: i32, out: *mut i32) -> i32 {
    unsafe { put(out, 0) };
    OK
}

/// 0x7fb: a current counter.
extern "C" fn current(index: i32, out: *mut i32) -> i32 {
    unsafe { put(out, counter(index).map_or(0, |i| CURRENT[i].load(Ordering::Relaxed))) };
    OK
}

/// 0x7fd: a lifetime counter.
extern "C" fn lifetime(index: i32, out: *mut i32) -> i32 {
    unsafe { put(out, counter(index).map_or(0, |i| LIFETIME[i].load(Ordering::Relaxed))) };
    OK
}

/// 0x7fc: resets a current counter (the audits reset).
extern "C" fn reset(index: i32, out: *mut i32) -> i32 {
    if let Some(i) = counter(index) {
        CURRENT[i].store(0, Ordering::Relaxed);
    }
    unsafe { put(out, 0) };
    OK
}

/// The volume monitor: the volume read.
extern "C" fn get_volume(out: *mut i32) -> i32 {
    unsafe { put(out, VOLUME.load(Ordering::Relaxed)) };
    OK
}

/// The volume monitor: the volume set.
extern "C" fn set_volume(volume: *const i32) -> i32 {
    if !volume.is_null() {
        VOLUME.store(unsafe { volume.read() }, Ordering::Relaxed);
    }
    OK
}

/// The dongle record: game, version, region, country, cabinet label and type, each field
/// followed by a NUL (the game compares them with these values).
const DONGLE: [u8; 27] = *b"\0BLAZE \x001.0\0US\0\0CTRY\0CAB\x000\0";

/// Reads the dongle into the game's record: present.
extern "C" fn read_dongle(record: *mut u8) -> bool {
    if !record.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(DONGLE.as_ptr().add(1), record.add(1), DONGLE.len() - 1) };
    }
    log!("blazing: dongle read");
    true
}

/// The profile loader: its branches taken when a save section is missing (to its end, skipping
/// the "loaded" mark) to the mark instead (`je` offsets), and loaded returned (`mov al, 1`).
const PROFILE_LOADED: [(usize, &[u8]); 6] = [
    (0x43435e, &[0x54]),
    (0x434373, &[0x3f]),
    (0x434388, &[0x2a]),
    (0x43439d, &[0x15]),
    (0x4343b2, &[0x00]),
    (0x4343ba, &[0xb0, 0x01]),
];
const PROFILE: &str = "Arcade";

/// The `Arcade` profile folder, when there is no profile.
fn make_profile() {
    let mut buf = [0u16; 260];
    let hr = unsafe { SHGetFolderPathW(std::ptr::null_mut(), CSIDL_PERSONAL as i32, std::ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if hr != 0 {
        return;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let profiles = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len])).join("BA").join("UserDataRegion");
    let any = std::fs::read_dir(&profiles).is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir())));
    if !any {
        let made = std::fs::create_dir_all(profiles.join(PROFILE));
        log!("blazing: profile {PROFILE} made in {}: {made:?}", profiles.display());
    }
}

/// The game's client functions and their replacements.
fn clients() -> [(usize, usize); 11] {
    [
        (0x6275e0, read_dongle as *const () as usize),
        (0x56b3c0, coins as *const () as usize),
        (0x56b2e0, take as *const () as usize),
        (0x56b350, service_coins as *const () as usize),
        (0x56b730, version as *const () as usize),
        (0x56b7a0, meter as *const () as usize),
        (0x56b820, current as *const () as usize),
        (0x56b8a0, lifetime as *const () as usize),
        (0x56b920, reset as *const () as usize),
        (0x56bc70, get_volume as *const () as usize),
        (0x56bce0, set_volume as *const () as usize),
    ]
}

/// A `jmp` from `at` to `to`.
unsafe fn jump(at: usize, to: usize) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        (at as *mut u8).write(0xE9);
        ((at + 1) as *mut i32).write_unaligned(to.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}

/// The coin buttons of the virtual sticks, each press a coin in the bank and its counter.
fn coin_thread() {
    std::thread::spawn(|| {
        let mut previous = 0u32;
        loop {
            let mut pressed = 0u32;
            for player in 0..2 {
                if wal_payload_common::input(player).pressed(button::COIN) {
                    pressed |= 1 << player;
                }
            }
            let new = (pressed & !previous).count_ones() as i32;
            if new > 0 {
                let bank = BANK.fetch_add(new, Ordering::Relaxed) + new;
                CURRENT[0].fetch_add(new, Ordering::Relaxed);
                LIFETIME[0].fetch_add(new, Ordering::Relaxed);
                log!("blazing: coin, {bank} in the bank");
            }
            previous = pressed;
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    });
}

/// The controller's (`DevXbox`) vtable slot of `GetValue(input index)`.
const GET_VALUE_SLOT: usize = 0x6e5fd8;
static ORIG_GET_VALUE: AtomicUsize = AtomicUsize::new(0);

type GetValue = unsafe extern "thiscall" fn(usize, u32) -> f32;

unsafe extern "thiscall" fn get_value(this: usize, index: u32) -> f32 {
    let orig: GetValue = unsafe { std::mem::transmute(ORIG_GET_VALUE.load(Ordering::Relaxed)) };
    let v = unsafe { orig(this, index) };
    // each change of an input (to a tenth)
    static LAST: [AtomicI32; 256] = [const { AtomicI32::new(i32::MIN) }; 256];
    if index < 256 && LAST[index as usize].swap((v * 10.0) as i32, Ordering::Relaxed) != (v * 10.0) as i32 {
        let code = if index < 32 { unsafe { *((this + 0x10 + 4 * index as usize) as *const u32) } } else { 0 };
        log!("blazing: input {index} (code {code:#x}) = {v}");
    }
    v
}

pub(crate) fn init() {
    if crate::frontend_fit::exe_name() != "mainr.exe" || crate::exe_size() != IMAGE_SIZE {
        return;
    }
    for (at, to) in clients() {
        unsafe { jump(at, to) };
    }
    for (at, bytes) in PROFILE_LOADED {
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
            VirtualProtect(at as *const _, bytes.len(), old, &mut old);
        }
    }
    // The PC game also reads the real keyboard (GetAsyncKeyState, its window messages) and then
    // drops the joystick for it: no real key reaches it.
    wal_payload_common::input_filter::init(unsafe { GetModuleHandleW(std::ptr::null()) } as usize, |_| false);
    make_profile();
    coin_thread();
    log!("blazing: coin and volume monitors answered");
    if std::env::var("WAL_GLOBALVR_BLAZING_TRACE").is_ok_and(|v| v == "1") {
        let slot = GET_VALUE_SLOT as *mut usize;
        let mut old = 0;
        unsafe {
            VirtualProtect(slot.cast(), 4, PAGE_READWRITE, &mut old);
            ORIG_GET_VALUE.store(slot.read(), Ordering::Relaxed);
            slot.write(get_value as *const () as usize);
            VirtualProtect(slot.cast(), 4, old, &mut old);
        }
        log!("blazing: controller reads traced");
    }
}
