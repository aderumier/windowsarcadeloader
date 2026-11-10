//! Pull The Trigger (Digital Silkroad, 2003, `PTW-BootTest.exe`: the decrypted executable of
//! the WibuKey-protected `PTW.exe`, OpenGL, 32-bit): `WAL_ARCADEPC_BOARD=pullthetrigger`.
//!
//! A one-gun shooting game (two players: two cabinets linked). The cabinet's gun board talks
//! through COM2 (MarshallSoft `wsc32.dll`: the aim, the trigger, the start button), but the
//! game's window procedure takes the same from the mouse and the keyboard, and goes on without
//! the board ("iolink setup fail"). The payload plays that mouse and keyboard with the virtual
//! sticks (`payload-common`'s `input_filter`: the real keyboard and mouse never reach the game):
//!
//! * the gun is the mouse on the game window's client area (the dump's community
//!   `PttBootTest.dll` stretches the 640x480 window to the screen's 4:3 and scales its mouse
//!   back), the trigger its left button: a shot, and the game start after a start press;
//! * `start` S (the start button), `coin` Insert (a credit), `cover` Space held (the cabinet's
//!   pedal: behind cover, no shots), `test` Home (the operator menu, at the title), the menu's
//!   keys Page Up / Page Down / Delete / End from the directions.
//!
//! The community library makes the game's window its monitor's size and draws the 4:3 picture
//! in its middle (black sides). Its monitor (`GetMonitorInfoW`) is the screen's centered 4:3
//! rectangle instead: the window is the picture, the dump's bezel (`WAL_BEZEL`) around it.
//!
//! The common crosshair option (`WAL_CROSSHAIR`): the gun's crosshair drawn before the
//! executable's `SwapBuffers` (the community library's hook of it after, its FREEPLAY over it).
//!
//! Free play (`WAL_FREEPLAY`): the price of a credit (`settings\coinsetting.txt`, the game
//! object's +0x120, 0x4cc340) kept at 0: start then a shot begins a game without coins.
//!
//! Natives: `trigger`, `cover`, `start`, `coin`, `test`, `menu_up`, `menu_down`, `menu_left`,
//! `menu_right`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{HMONITOR, MONITORINFO};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

/// The price of a credit (a 16-bit value: the game object's +0x120).
const PRICE: usize = 0x4cc340;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    /// A key: its virtual key.
    Key(u16),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Key(b'S' as u16)),
    ("coin", Native::Key(0x2D)),
    ("cover", Native::Key(0x20)),
    ("test", Native::Key(0x24)),
    ("menu_up", Native::Key(0x21)),
    ("menu_down", Native::Key(0x22)),
    ("menu_left", Native::Key(0x2E)),
    ("menu_right", Native::Key(0x23)),
];

/// The gun's second button and the pedal (keyboard C: b7) take cover.
const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "cover"),
    ("b7", "cover"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("up", "menu_up"),
    ("down", "menu_down"),
    ("left", "menu_left"),
    ("right", "menu_right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let (mut keys_was, mut trigger_was, mut last) = ([0u64; 4], false, (i32::MIN, i32::MIN));
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        // the game reads its settings at its window's creation (and from its operator menu)
        if free_play {
            let price = PRICE as *mut u16;
            if unsafe { price.read_volatile() } != 0 {
                unsafe { price.write_volatile(0) };
                log!("pullthetrigger: free play (a credit's price 0)");
            }
        }
        let mut keys = [0u64; 4];
        let mut trigger = false;
        map().for_each_pressed(&input(0), |n| match n {
            Native::Trigger => trigger = true,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        for vk in 0..256u16 {
            let (now, was) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, keys_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != was {
                post_key(hwnd, vk, now);
            }
        }
        for (h, k) in HELD.iter().zip(keys) {
            h.store(k, Ordering::Relaxed);
        }
        keys_was = keys;
        // the gun: the mouse on the window's picture (kept at its last place off it)
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => last,
        };
        let buttons = if trigger { MK_LBUTTON } else { 0 };
        // again every half second: a pointer that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if trigger != trigger_was && last.0 != i32::MIN {
            post_mouse(hwnd, if trigger { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
        }
        trigger_was = trigger;
    }
}

static ORIG_MONITOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn swap_buffers(dc: *mut std::ffi::c_void) -> i32 {
    let hwnd = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(dc) };
    if !hwnd.is_null() {
        unsafe { wal_payload_common::crosshair::draw_gl_window(hwnd) };
    }
    let orig: unsafe extern "system" fn(*mut std::ffi::c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(dc) }
}

/// A rectangle's centered 4:3 part (all of it when it is narrower).
fn four_three(r: &mut RECT) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w * 3 > h * 4 {
        let width = h * 4 / 3;
        r.left += (w - width) / 2;
        r.right = r.left + width;
    }
}

/// The community library's monitor: the screen's centered 4:3 rectangle.
unsafe extern "system" fn monitor_info(monitor: HMONITOR, info: *mut MONITORINFO) -> i32 {
    let orig: unsafe extern "system" fn(HMONITOR, *mut MONITORINFO) -> i32 = unsafe { std::mem::transmute(ORIG_MONITOR.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(monitor, info) };
    if ok != 0 && !info.is_null() {
        unsafe {
            four_three(&mut (*info).rcMonitor);
            four_three(&mut (*info).rcWork);
            let r = (*info).rcMonitor;
            log!("pullthetrigger: the window on the screen's 4:3 ({},{})-({},{})", r.left, r.top, r.right, r.bottom);
        }
    }
    ok
}

pub fn init() {
    // the game's message loop: the executable's
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let ptt = unsafe { GetModuleHandleW(windows_sys::w!("PttBootTest.dll")) } as usize;
    if ptt != 0 {
        if let Some(orig) = unsafe { wal_payload_common::iat::hook_module(ptt, "user32.dll", "GetMonitorInfoW", monitor_info as *const () as usize) } {
            ORIG_MONITOR.store(orig, Ordering::Relaxed);
        }
    }
    if wal_payload_common::crosshair::init_gl() {
        if let Some(orig) = unsafe { wal_payload_common::iat::hook_module(exe, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
            ORIG_SWAP.store(orig, Ordering::Relaxed);
        }
    }
    wal_payload_common::input_filter::init(exe, key_held);
    own_mouse(exe);
    std::thread::spawn(input_thread);
    log!("pullthetrigger: the mouse and keys played by the gun");
}
