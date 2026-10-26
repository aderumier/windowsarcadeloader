//! Bullseye Crack Shot (Unity 2019.4, Mono, 64-bit; its assembly's method bodies encrypted, the
//! dump's runtime decrypting them): `WAL_ARCADEPC_BOARD=bullseye`.
//!
//! The dump's game has no cabinet board left: `SerialPortComm` never opens its port and
//! `GameData.KeyBoard` is set, the developer's keyboard and mouse mode (`InputCtrl.Update`):
//! the left button shoots player 1 at the mouse, `1` inserts a coin, `U` enters the settings,
//! `Y` their menu, `]` back, `A` / `S` / `D` left, OK, right. The payload plays that mode with
//! the virtual sticks: the gun is the mouse (its place on the game window's picture), the
//! trigger its left button, the keys posted (`payload-common`'s `input_filter`: the real
//! keyboard and mouse never reach the game, moves included).
//!
//! The game asks a 450x800 window (`CompanyLogo`: `Screen.SetResolution(450, 800, false)`), but
//! its UI effects (the shots' trails, sparks, the screen's cracks: `GameUI.ShowFire`) are placed
//! at screen pixels on a 1080-wide canvas: off their place at any other width.
//! `Screen.SetResolution(int, int, bool)` is replaced (`Mono::replace`): the same aspect fitted
//! on the desktop's height, the profile's 1080x1920 fake desktop (`WAL_DESKTOP_SIZE`) fitted on
//! the screen.
//!
//! Natives: `trigger coin` (player 1), `settings menu back left ok right` (the operator's).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// `wParam`'s left button flag.
const MK_LBUTTON: u32 = 0x0001;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    /// A key of the keyboard mode: its virtual key.
    Key(u16),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("coin", Native::Key(0x31)),
    ("settings", Native::Key(0x55)),
    ("menu", Native::Key(0x59)),
    ("back", Native::Key(0xDD)),
    ("left", Native::Key(0x41)),
    ("ok", Native::Key(0x53)),
    ("right", Native::Key(0x44)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("coin", "coin"),
    ("test", "settings"),
    ("service", "menu"),
    ("left", "left"),
    ("right", "right"),
    ("b2", "ok"),
    ("start", "ok"),
    ("up", "back"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let (mut keys_was, mut trigger_was, mut last) = ([0u64; 4], false, (i32::MIN, i32::MIN));
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut keys = [0u64; 4];
        let mut trigger = false;
        map().for_each_pressed(&input(0), |n| match n {
            Native::Trigger => trigger = true,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        // the keys: their changes posted
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
        center(hwnd);
        // the gun: the mouse on the window's picture, in the game's own pixels (a fake
        // desktop's, when the window is shown at another size: Unity takes the cursor's client
        // coordinates as its screen's pixels)
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        match desktop::size() {
            Some((w, h)) => (r.right, r.bottom) = (w, h),
            None => {
                unsafe { GetClientRect(hwnd, &mut r) };
            }
        }
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => last,
        };
        let buttons = if trigger { MK_LBUTTON as usize } else { 0 };
        if at != last && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if trigger != trigger_was && last.0 != i32::MIN {
            post_mouse(hwnd, if trigger { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
        }
        trigger_was = trigger;
    }
}

/// The game's window centered on the screen (fitted on its height: a 9:16 window).
fn center(hwnd: windows_sys::Win32::Foundation::HWND) {
    let mut w = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(hwnd, &mut w) };
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let (ww, wh) = (w.right - w.left, w.bottom - w.top);
    if desktop::size().is_some() || wh != sh {
        return;
    }
    let (x, y) = ((sw - ww) / 2, 0);
    if (w.left, w.top) != (x, y) {
        unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }
}

// The resolution: the game's window fitted on the screen

/// `Screen.SetResolution(int, int, bool, int)`'s compiled code.
static SET_RESOLUTION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `Screen.SetResolution(int width, int height, bool fullscreen)`: the same aspect fitted on
/// the (fake) desktop's height: windowed on the screen, full screen on a fake desktop (Unity
/// makes no window taller than the real screen).
extern "C" fn set_resolution(width: i32, height: i32, _fullscreen: u8) {
    let sh = desktop::size().map_or_else(|| unsafe { GetSystemMetrics(SM_CYSCREEN) }, |d| d.1);
    let w = if height > 0 { (sh as i64 * width as i64 / height as i64) as i32 } else { width };
    log!("bullseye: SetResolution({width}, {height}) -> {w}x{sh}");
    let f: extern "C" fn(i32, i32, u8, i32) = unsafe { std::mem::transmute(SET_RESOLUTION.load(Ordering::Relaxed)) };
    f(w, sh, desktop::size().is_some() as u8, 0);
}

/// Replaces `Screen.SetResolution(int, int, bool)` once UnityEngine's assembly is loaded.
fn replace_set_resolution() {
    type P = *mut std::ffi::c_void;
    use wal_payload_common::mono::mono;
    for _ in 0..20000 {
        std::thread::sleep(Duration::from_millis(1));
        let Some(m) = mono() else { continue };
        let image = m.image("UnityEngine.CoreModule");
        if image.is_null() || !m.attach() {
            continue;
        }
        let screen = m.class(image, "UnityEngine", "Screen");
        let module = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) };
        let get = |n: &[u8]| unsafe { GetProcAddress(module, n.as_ptr()) }.map(|f| f as usize).unwrap_or(0);
        let (desc_new, desc_search, compile) = (get(b"mono_method_desc_new\0"), get(b"mono_method_desc_search_in_class\0"), get(b"mono_compile_method\0"));
        if screen.is_null() || desc_new == 0 || desc_search == 0 || compile == 0 {
            log!("bullseye: Screen.SetResolution not found");
            return;
        }
        let find = |sig: &[u8]| -> P {
            unsafe {
                let desc_new: extern "C" fn(*const u8, i32) -> P = std::mem::transmute(desc_new);
                let desc_search: extern "C" fn(P, P) -> P = std::mem::transmute(desc_search);
                desc_search(desc_new(sig.as_ptr(), 0), screen)
            }
        };
        let (three, four) = (find(b":SetResolution(int,int,bool)\0"), find(b":SetResolution(int,int,bool,int)\0"));
        if three.is_null() || four.is_null() {
            log!("bullseye: Screen.SetResolution overloads not found");
            return;
        }
        let compile: extern "C" fn(P) -> usize = unsafe { std::mem::transmute(compile) };
        SET_RESOLUTION.store(compile(four), Ordering::Relaxed);
        let ok = m.replace(three, set_resolution as *const () as usize);
        log!("bullseye: Screen.SetResolution fitted on the screen ({ok})");
        return;
    }
}

pub fn init(unity: usize) {
    std::thread::spawn(replace_set_resolution);
    wal_payload_common::input_filter::init(unity, key_held);
    own_mouse(unity);
    std::thread::spawn(input_thread);
    log!("bullseye: the keyboard mode played by the guns");
}
