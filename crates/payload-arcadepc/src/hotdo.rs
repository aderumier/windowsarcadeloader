//! The House of the Dead: Overkill (The Typing of the Dead: Overkill's Steam build, `HOTD_NG.exe`,
//! BlitzTech, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=hotdo`.
//!
//! The game plays with the mouse: its pointer is the cursor (`GetCursorPos` and `ScreenToClient`
//! at each read, 0x67dd50), its buttons and keys DirectInput's system mouse and keyboard
//! (`GetDeviceState` into 0x9d180c / 0x9d1844). The payload's fake DirectInput
//! (`WAL_DINPUT_DISABLE=1`, `payload-common`'s `dinput.rs`) gives it a keyboard and a mouse from
//! player 1's virtual stick, and the executable's `GetCursorPos` answers the gun on the window's
//! picture (`input_filter::own_mouse`); the real keyboard and mouse never reach it.
//!
//! `WAL_HOTDO_MONEY=<cash>`: the gun shop's cash kept at that (default 999999, 0 the game's own).
//! `WAL_HOTDO_UNLOCK=1` (default): every level and game mode (Director's Cut) unlocked.
//!
//! `WAL_HOTDO_PLAYER=<n>`: the game is player n's (default 1): a second instance of the game, the
//! online co-op's other player over a LAN Steam emulator, plays player 2's gun.
//!
//! Natives: `trigger reload special` (the mouse's left, middle and right buttons: the dump's
//! AutoHotkey script swaps the last two), `pause enter`, `next_weapon previous_weapon` (the
//! player's actions, made by the payload: d-pad right / left).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, gun, input, input_filter, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_MOUSEMOVE};

/// DirectInput key codes.
const DIK_ESCAPE: u8 = 0x01;
const DIK_RETURN: u8 = 0x1C;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A mouse button: its number (0 left, 1 right, 2 middle).
    Button(u8),
    /// A keyboard key: its DirectInput code.
    Key(u8),
    /// A player's input action made on its object (no key of the PC's controls for it).
    Action(usize),
}

/// The players' input actions (`thiscall (player, pressed)`, as `0x4b9d90` calls them from a
/// local player's controls): 0xb the next weapon, 0xc the previous one, 5 the reload (the
/// weapon's +0x78f, reloaded by its update `0x5552a0` while the clip +0x760 is short of full),
/// 4 the special (+0x790, spending one of the player's +0x210).
pub const ACTION_NEXT_WEAPON_RVA: usize = 0x043460;
pub const ACTION_PREVIOUS_WEAPON_RVA: usize = 0x0de480;
pub const ACTION_RELOAD_RVA: usize = 0x0d5b80;
pub const ACTION_SPECIAL_RVA: usize = 0x197910;

/// Player `index`'s action (its object: [0x999688] + 0x24 + 4 * index).
pub fn player_action(index: usize, rva: usize) {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let Some(player) = read::<usize>(base + 0x599688).filter(|&p| p != 0).and_then(|p| read::<usize>(p + 0x24 + 4 * index)).filter(|&p| p != 0) else {
        return;
    };
    let action: unsafe extern "thiscall" fn(usize, u32) = unsafe { std::mem::transmute(base + rva) };
    unsafe { action(player, 1) };
    log!("hotdo: player {}'s action {rva:#x}", index + 1);
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Button(0)),
    ("reload", Native::Button(2)),
    ("special", Native::Button(1)),
    ("pause", Native::Key(DIK_ESCAPE)),
    ("enter", Native::Key(DIK_RETURN)),
    ("next_weapon", Native::Action(ACTION_NEXT_WEAPON_RVA)),
    ("previous_weapon", Native::Action(ACTION_PREVIOUS_WEAPON_RVA)),
];

const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("b2", "reload"), ("b3", "special"), ("start", "enter"), ("test", "pause"), ("right", "next_weapon"), ("left", "previous_weapon")];

/// The player whose gun and stick play the game (`WAL_HOTDO_PLAYER`, 0-based).
fn player() -> usize {
    static PLAYER: OnceLock<usize> = OnceLock::new();
    *PLAYER.get_or_init(|| std::env::var("WAL_HOTDO_PLAYER").ok().and_then(|n| n.trim().parse::<usize>().ok()).map_or(0, |n| n.saturating_sub(1)))
}

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The mouse's left button pressed by the payload (the menu's click at start).
static AUTO_CLICK: AtomicBool = AtomicBool::new(false);
/// The cursor's place set by the payload (the menu's click at start; picture fractions × 10000,
/// x in the high half; 0: the gun's).
static AUTO_CURSOR: AtomicU32 = AtomicU32::new(0);

/// The DirectInput mouse's buttons: player 1's (the payload's click at start).
fn buttons() -> u8 {
    if AUTO_CURSOR.load(Ordering::Relaxed) != 0 {
        return AUTO_CLICK.load(Ordering::Relaxed) as u8;
    }
    let mut held = 0;
    map().for_each_pressed(&input(player()), |n| {
        if let Native::Button(b) = n {
            held |= 1 << b;
        }
    });
    held
}

/// The DirectInput keyboard's keys: player 1's.
fn keyboard(keys: &mut [u8; 256]) {
    map().for_each_pressed(&input(player()), |n| {
        if let Native::Key(k) = n {
            keys[k as usize] = 0x80;
        }
    });
}

/// The cursor: the gun on the game window's picture (its last place off the picture), or the
/// payload's.
fn cursor_thread() {
    let mut last = (i32::MIN, i32::MIN);
    let mut actions_was: Vec<usize> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(4));
        // the actions, once per press
        let mut actions = Vec::new();
        map().for_each_pressed(&input(player()), |n| {
            if let Native::Action(rva) = n {
                actions.push(rva);
            }
        });
        for &rva in actions.iter().filter(|rva| !actions_was.contains(rva)) {
            player_action(0, rva);
        }
        actions_was = actions;
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let auto = AUTO_CURSOR.load(Ordering::Relaxed);
        let at = if auto != 0 {
            // a place of the game's 16:9 picture, letterboxed or pillarboxed in the window
            let (w, h) = (r.right as f32, r.bottom as f32);
            let (pw, ph) = if w * 9.0 < h * 16.0 { (w, w * 9.0 / 16.0) } else { (h * 16.0 / 9.0, h) };
            let (x, y) = ((auto >> 16) as f32 / 10000.0, (auto & 0xFFFF) as f32 / 10000.0);
            (((w - pw) / 2.0 + x * pw) as i32, ((h - ph) / 2.0 + y * ph) as i32)
        } else {
            let Some((x, y)) = gun::gun_of_player_in_picture(player()) else { continue };
            ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32)
        };
        if at != last {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, 0);
            last = at;
        }
    }
}

// The House of the Dead: Overkill at start: the Typing of the Dead's main menu's last item
// clicked by the payload (the choice is the frontend script's, never saved)

/// The script natives' table (RVA): 16-byte entries (create, name, exec, hash).
const NATIVES_RVA: usize = 0x5692d0;
const NATIVES_COUNT: usize = 511;
/// Where the main menu's "The House of the Dead: Overkill" item is (fractions of the 16:9
/// picture: the game letterboxes it on a 16:10 screen).
const OVERKILL_ITEM: (f32, f32) = (0.765, 0.86);
/// The main menus shown since start (each loads the frontend's sectors 7, 8, 2).
static MAIN_MENUS: AtomicU32 = AtomicU32::new(0);
static FORCE_SECTOR_LOADED: AtomicUsize = AtomicUsize::new(0);
/// The Typing of the Dead's menu hidden (black) from its first showing until Overkill's.
static HIDE_MENU: AtomicBool = AtomicBool::new(false);

/// The overlay (payload-common's Direct3D 9 `Present` hook): the back buffer cleared to black
/// while the Typing of the Dead's menu is up.
unsafe fn hide_menu(dev: *mut std::ffi::c_void, _window: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    if HIDE_MENU.load(Ordering::Relaxed) {
        // the back buffer filled black (IDirect3DDevice9::GetBackBuffer, ColorFill: whatever the
        // render target and states)
        const GET_BACK_BUFFER: usize = 18;
        const COLOR_FILL: usize = 35;
        type P = *mut std::ffi::c_void;
        let method = |slot: usize| unsafe { *(*(dev as *const *const usize)).add(slot) };
        let get: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> i32 = unsafe { std::mem::transmute(method(GET_BACK_BUFFER)) };
        let fill: unsafe extern "system" fn(P, P, *const u8, u32) -> i32 = unsafe { std::mem::transmute(method(COLOR_FILL)) };
        let mut back: P = std::ptr::null_mut();
        if unsafe { get(dev, 0, 0, 0, &mut back) } >= 0 && !back.is_null() {
            unsafe {
                fill(dev, back, std::ptr::null(), 0xff00_0000);
                // IUnknown::Release
                let release: unsafe extern "system" fn(P) -> u32 = std::mem::transmute(*(*(back as *const *const usize)).add(2));
                release(back);
            }
        }
    }
    None
}

unsafe fn hide_menu_reset(_dev: *mut std::ffi::c_void) {}

/// `faForceSectorLoaded`'s node (its sector at +0x3c): a main menu shown at sector 7.
extern "C" fn force_sector_loaded(node: usize) -> usize {
    if unsafe { *((node + 0x3c) as *const u32) } == 7 {
        let n = MAIN_MENUS.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 {
            HIDE_MENU.store(true, Ordering::Relaxed);
        }
        log!("hotdo: main menu shown ({n})");
    }
    let f: extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(FORCE_SECTOR_LOADED.load(Ordering::Relaxed)) };
    f(node)
}

/// Hooks `faForceSectorLoaded` (once the executable is unpacked: its window shown).
fn hook_natives() -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let table = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + NATIVES_RVA;
    for i in 0..NATIVES_COUNT {
        let entry = table + i * 16;
        let name = unsafe { std::ffi::CStr::from_ptr(*((entry + 4) as *const *const i8)) };
        if name.to_bytes() != b"faForceSectorLoaded" {
            continue;
        }
        let mut old = 0;
        unsafe {
            VirtualProtect((entry + 8) as *const _, 4, PAGE_READWRITE, &mut old);
            FORCE_SECTOR_LOADED.store(*((entry + 8) as *const usize), Ordering::Relaxed);
            *((entry + 8) as *mut usize) = force_sector_loaded as *const () as usize;
            VirtualProtect((entry + 8) as *const _, 4, old, &mut old);
        }
        return true;
    }
    false
}

/// The first main menu's Overkill item clicked until the second main menu shows.
fn overkill_thread() {
    while gun::find_window().is_null() {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !hook_natives() {
        log!("hotdo: faForceSectorLoaded not found: the Typing of the Dead's menu stays");
        return;
    }
    wal_payload_common::screenshot::set_overlay(hide_menu, hide_menu_reset);
    while MAIN_MENUS.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    let at = ((OVERKILL_ITEM.0 * 10000.0) as u32) << 16 | (OVERKILL_ITEM.1 * 10000.0) as u32;
    // a few tries: the cursor is the player's again after them
    for _ in 0..5 {
        // the menu's opening transition, the cursor on the item, a click
        std::thread::sleep(Duration::from_millis(1500));
        if MAIN_MENUS.load(Ordering::Relaxed) > 1 {
            break;
        }
        AUTO_CURSOR.store(at, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        AUTO_CLICK.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(150));
        AUTO_CLICK.store(false, Ordering::Relaxed);
        log!("hotdo: Overkill item clicked");
    }
    std::thread::sleep(Duration::from_millis(300));
    AUTO_CURSOR.store(0, Ordering::Relaxed);
    HIDE_MENU.store(false, Ordering::Relaxed);
    log!("hotdo: the gun is the player's");
}

/// The profile kept once a second (the game's stats [0x9996c8], its profile at +0x228): the gun
/// shop's cash (`ProfileMoney`, a u32 at +0x178) at `WAL_HOTDO_MONEY` (default 999999, 0 the
/// game's own), every level and game mode (Director's Cut) unlocked with `WAL_HOTDO_UNLOCK=1`
/// (default).
fn money_thread() {
    let money: u32 = std::env::var("WAL_HOTDO_MONEY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(999_999);
    let unlock = std::env::var("WAL_HOTDO_UNLOCK").map_or(true, |v| v.trim() == "1");
    if money == 0 && !unlock {
        return;
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let mut logged = false;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(profile) = read::<usize>(base + 0x5996c8).filter(|&s| s != 0).and_then(|s| read::<usize>(s + 0x228)).filter(|&p| p != 0) else {
            continue;
        };
        if unlock {
            // every level unlocked (UnlockLevel 0x419070: the profile's +0x34 bit n)
            if read::<u32>(profile + 0x34).is_some_and(|l| l != u32::MAX) {
                unsafe { ((profile + 0x34) as *mut u32).write(u32::MAX) };
            }
            // every game mode available, Director's Cut's included: IsGameModeAvailable
            // (0x50a1f0) is the profile's byte +0x1b4 + mode (0-6; online mode 5: +0x1bb), the
            // current profile's or a slot's ([stats] + 0x7980 + 4 * slot)
            let stats = read::<usize>(base + 0x5996c8).unwrap_or(0);
            for p in [Some(profile), read::<usize>(stats + 0x7980), read::<usize>(stats + 0x7984)].into_iter().flatten().filter(|&p| p != 0) {
                if read::<[u8; 8]>(p + 0x1b4).is_some_and(|m| m != [1; 8]) {
                    unsafe { ((p + 0x1b4) as *mut [u8; 8]).write([1; 8]) };
                    log!("hotdo: the profile {p:#x}'s levels and game modes unlocked");
                }
            }
        }
        let Some(cash) = read::<u32>(profile + 0x178) else { continue };
        if cash < money {
            unsafe { ((profile + 0x178) as *mut u32).write(money) };
            if !logged {
                log!("hotdo: the profile's cash {cash} -> {money}");
                logged = true;
            }
        }
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    input_filter::init(exe, |_| false);
    own_mouse(exe);
    dinput::set_keyboard(keyboard);
    dinput::set_mouse_buttons(buttons);
    std::thread::spawn(cursor_thread);
    std::thread::spawn(overkill_thread);
    std::thread::spawn(money_thread);
    if std::env::var("WAL_HOTDO_PEER").is_ok_and(|v| v.trim() == "1") {
        crate::hotdo_peer::init();
    }
    log!("hotdo: the mouse and keyboard from player 1's gun and stick");
}

fn read<T: Copy>(at: usize) -> Option<T> {
    use windows_sys::Win32::System::Memory::IsBadReadPtr;
    if at < 0x10000 || unsafe { IsBadReadPtr(at as *const _, size_of::<T>()) } != 0 {
        return None;
    }
    Some(unsafe { (at as *const T).read_unaligned() })
}
