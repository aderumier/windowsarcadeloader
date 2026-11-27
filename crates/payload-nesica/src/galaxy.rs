//! Hoshi to Tsubasa no Paradox (Square Enix / Byking, NESiCAxLive 2, 64-bit Unreal Engine 4,
//! `AcrGame-Win64-Shipping.exe`): the cabinet's two USB I/O boards (main and sub) are reached
//! through `AcrGame\Binaries\Win64\GALAXYIO.dll` (loaded by the game's `UGALAXYIOWrapper`,
//! functions found with `GetProcAddress`): replaced by this DLL (profile `payload_install`,
//! started by wal-loader).
//!
//! `GALAXYIO_Update(IoBuffer *, int *error)` (the PDB's `IoBuffer`, 248 bytes): +0x00 `InPort`,
//! the 28 switches of `EIoIN` from bit 0: SELECT, TEST, SERVICE, ENTER, -, -, COIN2, COIN1,
//! WHEEL_ENC_A/B, LTHUMB_PUSH, RTHUMB_PUSH, LTRIGGER_1/2, RTRIGGER_1/2, RESPAWN, SEAT_BELT,
//! SEAT_SENS, EMSTOP_SW_1, SAFTY_A/B/C, -, LPEDAL_1/2, RPEDAL_1/2; the game inverts bits 20, 21
//! and 24-27 (`UpdateDigitalInput` XORs 0x0F300000: the safety sensors and pedals are active
//! low). +0x04 `InAnalog[16]` (u16): the game reads the sub board's eight, [8..15] = left thumb
//! stick x/y, right thumb stick x/y, left stick x/y, right stick x/y (`GetStick`: the two sticks
//! read inverted, 0xFFFF - value, then the test menu's calibration applied: base, min, max in
//! `D:\Saved\ACRSaved\TestMode\Stick\StickData.json`; a thumb stick's base is its first value).
//! +0x24 `InEncoder[8]`: [4] the right lever's wheel (a step counter).
//! +0xC0 `Coin[2]` (14 bits, the top two an error state), +0xC4 `CoinDec[2]` (the game's
//! decrements), +0xC8 `NESiCA_Command` (0xC2 while the game reads a card), +0xC9
//! `NESiCA_Status` (0x10 read + type 1 NESiCA / 2 AICC; 0x80 an error, 0x20 no card),
//! +0xCA `NESiCA_UID[7]`, +0xD1 `NESiCA_Serial[16]` (the card id), +0xE1 `AICC_Accesscode[20]`,
//! +0xF5 `MachineID` (0 and 2-4: seat cabinets). `GALAXYIO_GetStatus(main)` 0x200: running.
//!
//! Natives for `WAL_MAP`: `select test service enter coin lthumb rthumb ltrigger1 ltrigger2
//! rtrigger1 rtrigger2 respawn lpedal1 lpedal2 rpedal1 rpedal2 wheel lthumbup lthumbdown
//! lthumbleft lthumbright rthumbup rthumbdown rthumbleft rthumbright` and `touch` (the sub
//! monitor's touch screen, at the pointer).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input_filter, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

pub(crate) fn detected(module: HMODULE) -> bool {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().is_some_and(|name| name.eq_ignore_ascii_case("GALAXYIO.dll"))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Native {
    /// An `InPort` switch (the game's view, before its inversion of the active low ones).
    Switch(u8),
    Coin,
    /// A thumb stick pushed all the way (0 left, 1 right; x, y).
    Thumb(usize, i32, i32),
    /// A finger on the sub monitor (the touch screen), at the pointer (`pointer`).
    Touch,
    /// The right lever's wheel turned a notch (a weapon change: the game counts the encoder's
    /// steps, either way, more than 8 at once).
    Wheel,
    /// The sub monitor's picture over the main one shown or hidden, a press each.
    SubToggle,
}

const NATIVES: &[(&str, Native)] = &[
    ("select", Native::Switch(0)),
    ("test", Native::Switch(1)),
    ("service", Native::Switch(2)),
    ("enter", Native::Switch(3)),
    ("coin", Native::Coin),
    ("lthumb", Native::Switch(10)),
    ("rthumb", Native::Switch(11)),
    ("ltrigger1", Native::Switch(12)),
    ("ltrigger2", Native::Switch(13)),
    ("rtrigger1", Native::Switch(14)),
    ("rtrigger2", Native::Switch(15)),
    ("respawn", Native::Switch(16)),
    ("lpedal1", Native::Switch(24)),
    ("lpedal2", Native::Switch(25)),
    ("rpedal1", Native::Switch(26)),
    ("rpedal2", Native::Switch(27)),
    ("lthumbup", Native::Thumb(0, 0, -1)),
    ("lthumbdown", Native::Thumb(0, 0, 1)),
    ("lthumbleft", Native::Thumb(0, -1, 0)),
    ("lthumbright", Native::Thumb(0, 1, 0)),
    ("rthumbup", Native::Thumb(1, 0, -1)),
    ("rthumbdown", Native::Thumb(1, 0, 1)),
    ("rthumbleft", Native::Thumb(1, -1, 0)),
    ("rthumbright", Native::Thumb(1, 1, 0)),
    ("touch", Native::Touch),
    ("wheel", Native::Wheel),
    ("subtoggle", Native::SubToggle),
];

/// Pad layout (b1-b4 = X Y R1 L1, b5-b8 = A B R2 L2): the two levers on the pad's sticks, their
/// triggers (the weapons: the game's ShotL / ShotR) on L1 / R1, their second switches (LButton
/// and Step / RButton) on X / Y, the pedals on L2 / R2 and A / B (left / right, both sensors of
/// each), the left thumb button (the AR skill) on the d-pad's right (the right one is a second
/// RButton: on Y too), the wheel on the d-pad's up and down, respawn on its left; START touches
/// the sub monitor (the touch screen: the cabinet starts there) where the mouse points; `kp1`
/// (the profile's pad hotkey) shows / hides its picture.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "wheel"),
    ("down", "wheel"),
    ("left", "respawn"),
    ("right", "lthumb"),
    ("b4", "ltrigger1"),
    ("b3", "rtrigger1"),
    ("b1", "ltrigger2"),
    ("b2", "rtrigger2+rthumb"),
    ("b5", "lpedal1+lpedal2"),
    ("b8", "lpedal1+lpedal2"),
    ("b6", "rpedal1+rpedal2"),
    ("b7", "rpedal1+rpedal2"),
    ("start", "touch"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("kp0", "touch"),
    ("kp1", "subtoggle"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

/// The `InPort` bits the game inverts (active low on the boards).
const ACTIVE_LOW: u32 = 0x0f30_0000;


/// A stick's calibration (the game's values: base, offsets of the extremes).
#[derive(Clone, Copy)]
struct Calibration {
    base: [i32; 2],
    min: [i32; 2],
    max: [i32; 2],
}

/// The thumb sticks: centred, a range inside the game's default calibration.
const THUMB: Calibration = Calibration { base: [0x8000; 2], min: [-0x3000; 2], max: [0x3000; 2] };

/// The two sticks' calibration, as the test menu saved it (else a centred default).
static STICKS: OnceLock<[Calibration; 2]> = OnceLock::new();

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // the real pads: the game's XInput (Unreal's) sees none, only the I/O boards play
    for (ordinal, hook) in [(2, xinput_get_state as *const () as usize), (3, xinput_set_state as *const () as usize)] {
        if unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", ordinal, hook) }.is_none() {
            log!("galaxy: XInput ordinal {ordinal} not imported");
        }
    }
    // Windows 8's touch feedback, absent from Wine (the game aborted at its window's creation)
    unsafe { iat::hook("user32.dll", "SetWindowFeedbackSetting", set_window_feedback_setting as *const () as usize) };
    // the real keyboard and mouse: only the launcher's input
    input_filter::init(exe, |_| false);
    // the game's cursor: the touch screen's (`windows::touch`)
    input_filter::own_mouse(exe);
    windows::init();
    windows::slate_moves(exe);
    hover_on_press(exe);
    all_day(exe);
    // the game server (card play: the profile's `WAL_NESICA_NESYS_HOST` names it)
    if !std::env::var("WAL_GALAXY_SERVER").is_ok_and(|v| v.trim() == "0") {
        crate::galaxy_server::start();
    }
    cabinet(exe);
    free_play();
    demo_sound();
    graphics();
    log!("galaxy: I/O boards emulated");
}

/// The game's two windows: the main monitor's and the sub monitor's (the cabinet's touch screen
/// under the main one, a child window of 1024x768). The sub monitor's window is kept off the
/// screen (right of it) at its size; its picture is drawn in a corner of the main window's
/// (`wal_payload_common::subwindow_d3d11`), and touches there are the sub monitor's: the touch
/// panel's mouse at the matching place of the sub window (`touch`).
///
/// `WAL_GALAXY_SUB_SIZE`: the picture's height in percent of the screen's (default 34);
/// `WAL_GALAXY_SUB_PLACE`: its place, `top-left` (default), `top-right`, `bottom-left`,
/// `bottom-right`, `bottom`, or `window`: the sub window left where the game puts it (a second
/// monitor's window).
mod windows {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use wal_payload_common::subwindow_d3d11::{self, Layout};
    use wal_payload_common::{iat, input_filter, log};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN};

    type P = *mut u8;

    static CREATE: AtomicUsize = AtomicUsize::new(0);
    static SET_POS: AtomicUsize = AtomicUsize::new(0);
    static CLIENT_TO_SCREEN: AtomicUsize = AtomicUsize::new(0);
    static MOVED: AtomicUsize = AtomicUsize::new(0);
    /// Where Slate believes the sub window is (its place on the screen as the game made it,
    /// packed x, y): the window is off the screen, and Slate's hit test finds nothing there.
    static SLATE_AT: AtomicU64 = AtomicU64::new(0);
    /// The pointer drawn on the sub monitor's picture (packed u, v in 1/65536), 0: none.
    static POINTER_AT: AtomicU64 = AtomicU64::new(0);
    /// The main window, the sub monitor's.
    static MAIN: AtomicUsize = AtomicUsize::new(0);
    static SUB: AtomicUsize = AtomicUsize::new(0);

    /// The pointer to draw (fractions of the sub monitor), or none.
    pub(super) fn set_pointer(at: Option<(f32, f32)>) {
        let packed = at.map_or(0, |(u, v)| 1 << 63 | ((u * 65536.0) as u64) << 32 | (v * 65536.0) as u64);
        POINTER_AT.store(packed, Ordering::Relaxed);
    }

    fn pointer() -> Option<(f32, f32)> {
        let p = POINTER_AT.load(Ordering::Relaxed);
        (p != 0).then(|| (((p >> 32) & 0x7fff_ffff) as f32 / 65536.0, (p & 0xffff_ffff) as f32 / 65536.0))
    }

    /// The sub monitor's picture in the main window's: left, top, width, height (fractions).
    static PLACE: OnceLock<Option<[f32; 4]>> = OnceLock::new();

    pub(super) fn rect() -> Option<[f32; 4]> {
        *PLACE.get_or_init(|| {
            let place = std::env::var("WAL_GALAXY_SUB_PLACE").unwrap_or_default().trim().to_ascii_lowercase();
            let place = if place.is_empty() { "top-left".to_string() } else { place };
            if place == "window" {
                log!("galaxy: sub monitor window left as the game makes it");
                return None;
            }
            let height = std::env::var("WAL_GALAXY_SUB_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| (10.0..=100.0).contains(v)).unwrap_or(34.0) / 100.0;
            // 4:3 on the main window's 16:9 picture
            let width = height * (4.0 / 3.0) * (9.0 / 16.0);
            let x = match place.as_str() {
                "bottom-right" | "top-right" => 1.0 - width,
                "bottom" => (1.0 - width) / 2.0,
                _ => 0.0,
            };
            let y = if place.starts_with("top") { 0.0 } else { 1.0 - height };
            log!("galaxy: sub monitor drawn {place}, {:.0}% of the screen's height", height * 100.0);
            Some([x, y, width, height])
        })
    }

    /// The sub monitor's picture hidden (`subtoggle`; shown at start).
    static HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub(super) fn toggle_shown() {
        let hidden = !HIDDEN.fetch_xor(true, Ordering::Relaxed);
        log!("galaxy: sub monitor picture {}", if hidden { "hidden" } else { "shown" });
    }

    fn layout() -> Option<Layout> {
        let (source, target) = (SUB.load(Ordering::Relaxed), MAIN.load(Ordering::Relaxed));
        if source == 0 || target == 0 || HIDDEN.load(Ordering::Relaxed) {
            return None;
        }
        Some(Layout { source, target, rect: rect()?, pointer: pointer(), target_rect: None })
    }

    pub(super) fn init() {
        for (name, hook, slot) in [
            ("CreateWindowExW", create_window_ex_w as *const () as usize, &CREATE),
            ("SetWindowPos", set_window_pos as *const () as usize, &SET_POS),
            ("ClientToScreen", client_to_screen as *const () as usize, &CLIENT_TO_SCREEN),
        ] {
            match unsafe { iat::hook("user32.dll", name, hook) } {
                Some(o) => slot.store(o, Ordering::Relaxed),
                None => log!("galaxy: {name} not imported"),
            }
        }
        if rect().is_some() {
            subwindow_d3d11::init(layout);
        }
    }

    /// The sub window's place off the screen: right of the virtual screen.
    fn hidden_x() -> i32 {
        unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 64 }
    }

    fn wide(p: *const u16) -> String {
        if p.is_null() || (p as usize) < 0x10000 {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }

    #[allow(clippy::too_many_arguments)]
    unsafe extern "system" fn create_window_ex_w(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P) -> P {
        let unreal = wide(class) == "UnrealWindow";
        let real: unsafe extern "system" fn(u32, *const u16, *const u16, u32, i32, i32, i32, i32, P, P, P, P) -> P = unsafe { std::mem::transmute(CREATE.load(Ordering::Relaxed)) };
        let hwnd = unsafe { real(ex, class, title, style, x, y, w, h, parent, menu, inst, param) };
        if unreal {
            log!("galaxy: {} window {hwnd:?} '{}' {x},{y} {w}x{h}", if parent.is_null() { "main" } else { "sub monitor" }, wide(title));
            let slot = if parent.is_null() { &MAIN } else { &SUB };
            slot.store(hwnd as usize, Ordering::Relaxed);
            if !parent.is_null() {
                SLATE_AT.store(client_origin(hwnd), Ordering::Relaxed);
                hide(hwnd);
            }
        }
        hwnd
    }

    fn pack(x: i32, y: i32) -> u64 {
        (x as u32 as u64) << 32 | y as u32 as u64
    }

    /// A window's client area's top left on the screen (packed): Slate's place for it (its
    /// `WM_MOVE`), not the frame's (a window manager's frame on the desktop).
    fn client_origin(hwnd: P) -> u64 {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd.cast(), &mut pt) };
        pack(pt.x, pt.y)
    }

    fn slate_at() -> (i32, i32) {
        let at = SLATE_AT.load(Ordering::Relaxed);
        ((at >> 32) as u32 as i32, at as u32 as i32)
    }

    /// The game's own placements of the sub window: where Slate believes it is.
    unsafe extern "system" fn set_window_pos(hwnd: P, after: P, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32 {
        const SWP_NOMOVE: u32 = 2;
        let real: unsafe extern "system" fn(P, P, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_POS.load(Ordering::Relaxed)) };
        let ok = unsafe { real(hwnd, after, x, y, w, h, flags) };
        if hwnd as usize == SUB.load(Ordering::Relaxed) && flags & SWP_NOMOVE == 0 {
            SLATE_AT.store(client_origin(hwnd), Ordering::Relaxed);
            hide(hwnd);
        }
        ok
    }

    /// The sub window's client points in Slate's places (its mouse messages' places).
    unsafe extern "system" fn client_to_screen(hwnd: P, pt: *mut POINT) -> i32 {
        if hwnd as usize == SUB.load(Ordering::Relaxed) && rect().is_some() && !pt.is_null() {
            let (x, y) = slate_at();
            unsafe {
                (*pt).x += x;
                (*pt).y += y;
            }
            return 1;
        }
        let real: unsafe extern "system" fn(P, *mut POINT) -> i32 = unsafe { std::mem::transmute(CLIENT_TO_SCREEN.load(Ordering::Relaxed)) };
        unsafe { real(hwnd, pt) }
    }

    /// `FSlateApplication::OnMovedWindow` (+0x35024C0): the sub window's moves off the screen are
    /// not Slate's (its hit test finds no widget of a window off the screen).
    pub(super) fn slate_moves(exe: usize) {
        const AT: usize = 0x35024c0;
        // mov [rsp+8], rbx; push rdi; sub rsp, 0x30; mov edi, r8d
        const HEAD: [u8; 13] = [0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x30, 0x41, 0x8b, 0xf8];
        if rect().is_none() {
            return;
        }
        if unsafe { ((exe + AT) as *const [u8; 13]).read_unaligned() } != HEAD {
            log!("galaxy: unknown executable at +{AT:#x}: the sub monitor's touch screen may not work");
            return;
        }
        MOVED.store(super::detour(exe + AT, HEAD.len(), on_moved_window as *const () as usize), Ordering::Relaxed);
    }

    unsafe extern "C" fn on_moved_window(app: usize, window: usize, x: i32, y: i32) {
        // right of the screen (the client area's place: the frame's left plus its border)
        if x >= unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) } {
            return;
        }
        let real: unsafe extern "C" fn(usize, usize, i32, i32) = unsafe { std::mem::transmute(MOVED.load(Ordering::Relaxed)) };
        unsafe { real(app, window, x, y) }
    }

    /// The sub window off the screen, right of it: as soon as the game makes or places it, and
    /// each frame (`keep_hidden`). Moved behind the game's back (`SetWindowPos` from user32, not
    /// the executable's import); Slate keeps its own place for it (`on_moved_window`).
    fn hide(hwnd: P) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos};
        if rect().is_none() {
            return;
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd.cast(), &mut r) };
        let x = hidden_x();
        if r.left != x || r.top != 0 {
            log!("galaxy: sub monitor window moved off the screen ({},{} -> {x},0)", r.left, r.top);
            unsafe { SetWindowPos(hwnd.cast(), std::ptr::null_mut(), x, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }

    pub(super) fn keep_hidden() {
        let sub = SUB.load(Ordering::Relaxed) as P;
        if !sub.is_null() {
            hide(sub);
        }
    }

    // ---- the touch screen ----

    /// The finger down: its last place (u, v), or none.
    static FINGER: Mutex<Option<(f32, f32)>> = Mutex::new(None);

    /// The touch screen this frame: the finger at (`u`, `v`) of the sub monitor (fractions), or
    /// none. The cabinet's touch panel is a mouse to the game (its buttons, `SBumButton`, act on
    /// the cursor's pointer only, not on touch fingers): the left button's press, moves and
    /// release are posted to the sub window through the input filter, the payload's cursor (the
    /// executable's `GetCursorPos`, Slate's cursor: `input_filter::own_mouse`) at the finger.
    pub(super) fn touch(finger: Option<(f32, f32)>) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
        let mut last = FINGER.lock().unwrap();
        let message = match (*last, finger) {
            (None, None) => return,
            (None, Some(_)) => WM_LBUTTONDOWN,
            (Some(a), Some(b)) if (a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001 => return,
            (Some(_), Some(_)) => WM_MOUSEMOVE,
            (Some(_), None) => WM_LBUTTONUP,
        };
        let (u, v) = finger.or(*last).unwrap_or((0.5, 0.5));
        *last = finger;
        let sub = SUB.load(Ordering::Relaxed) as HWND;
        if sub.is_null() {
            return;
        }
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(sub, &mut client) };
        let x = (u * (client.right - client.left) as f32) as i32;
        let y = (v * (client.bottom - client.top) as f32) as i32;
        if message != WM_MOUSEMOVE {
            log!("galaxy: touch {} at {u:.2},{v:.2} of the sub monitor", if message == WM_LBUTTONDOWN { "down" } else { "up" });
        }
        // the cursor there first; MK_LBUTTON while the finger is down
        if message == WM_LBUTTONDOWN {
            input_filter::post_mouse(sub.cast(), WM_MOUSEMOVE, x, y, 0);
        }
        input_filter::post_mouse(sub.cast(), message, x, y, if message == WM_LBUTTONUP { 0 } else { 1 });
        // the cursor in Slate's places too
        let (sx, sy) = slate_at();
        input_filter::set_cursor(sx + x, sy + y);
    }
}

/// Sets the integer `key` of a test menu file (`D:\Saved\ACRSaved\TestMode\<file>`, read at
/// boot) to `value`. False when the file has no such key.
fn set_test_mode(file: &str, key: &str, value: i32) -> bool {
    let path = format!("{}\\Saved\\ACRSaved\\TestMode\\{file}", crate::drive::data_dir());
    let Ok(text) = std::fs::read_to_string(&path) else {
        log!("galaxy: no {path}");
        return false;
    };
    let Some(at) = text.find(&format!("\"{key}\"")) else {
        log!("galaxy: {path}: no {key}");
        return false;
    };
    let value_at = at + text[at..].find(':').map_or(0, |i| i + 1);
    let rest = &text[value_at..];
    let skip = rest.len() - rest.trim_start().len();
    let digits = rest.trim_start().find(|c: char| c != '-' && !c.is_ascii_digit()).unwrap_or(0);
    let new = format!("{}{value}{}", &text[..value_at + skip], &text[value_at + skip + digits..]);
    if new != text && let Err(e) = std::fs::write(&path, &new) {
        log!("galaxy: {path}: {e}");
        return false;
    }
    true
}

/// The common free play option (`WAL_NESICA_FREEPLAY` / `WAL_FREEPLAY`): the test menu's system
/// setting (`SystemFreePlay` of `System\System.json`). Unset: the cabinet's own setting.
fn free_play() {
    let Some(on) = wal_payload_common::free_play("WAL_NESICA_FREEPLAY") else { return };
    if set_test_mode("System\\System.json", "SystemFreePlay", i32::from(on)) {
        log!("galaxy: free play {}", if on { "on" } else { "off" });
    }
}

/// The attract's sound (`WAL_GALAXY_DEMO_VOLUME`, 0-100, the test menu's demo sound and its
/// volume in `Sound\Sound.json`; off and 0 in the dump). Unset: the cabinet's own setting.
fn demo_sound() {
    let Some(volume) = std::env::var("WAL_GALAXY_DEMO_VOLUME").ok().and_then(|v| v.trim().parse::<i32>().ok()) else { return };
    let volume = volume.clamp(0, 100);
    if set_test_mode("Sound\\Sound.json", "SoundDemoPlay", i32::from(volume > 0)) && set_test_mode("Sound\\Sound.json", "SoundDemoVolume", volume) {
        log!("galaxy: attract sound at {volume}");
    }
}

/// The picture's cost (`WAL_GALAXY_QUALITY`: 0-3, Unreal's scalability groups, 3 the cabinet's;
/// `WAL_GALAXY_SCREEN_PERCENTAGE`: the 3D scene's resolution in percent of the window's): the
/// user `Engine.ini`'s `[SystemSettings]` (outranks the game's `GameUserSettings.ini`).
fn graphics() {
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    let mut cvars: Vec<(String, String)> = Vec::new();
    if let Some(q) = std::env::var("WAL_GALAXY_QUALITY").ok().and_then(|v| v.trim().parse::<u8>().ok()) {
        for group in ["ViewDistance", "AntiAliasing", "Shadow", "PostProcess", "Texture", "Effects", "Foliage"] {
            cvars.push((format!("sg.{group}Quality"), q.min(3).to_string()));
        }
    }
    if let Some(p) = std::env::var("WAL_GALAXY_SCREEN_PERCENTAGE").ok().and_then(|v| v.trim().parse::<u32>().ok()) {
        cvars.push(("r.ScreenPercentage".into(), p.clamp(25, 100).to_string()));
    }
    let ini = std::path::PathBuf::from(format!("{local}\\AcrGame\\Saved\\Config\\WindowsNoEditor\\Engine.ini"));
    let refs: Vec<(&str, String)> = cvars.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    wal_payload_common::unreal_ini::system_settings(&ini, &refs);
}

/// The levers and buttons reach the game through Slate (the I/O wrapper's
/// `FSlateApplication::ProcessKeyDownEvent` / `ProcessAnalogInputEvent`), delivered to the
/// widget with the player's focus; a touch (a click in the sub window) takes it to the sub
/// monitor. A second after the last touch, the focus goes back to the game viewport
/// (`FSlateApplication::SetAllUserFocusToGameViewport`, +0x3512460, with the application of
/// `FSlateApplication::CurrentApplication`, +0x93B4060; called from the game thread).
fn game_viewport_focus() {
    const SET_FOCUS: usize = 0x3512460;
    const APPLICATION: usize = 0x93b4060;
    // mov [rsp+0x10], dl (the function's start, to recognize the executable)
    const SET_FOCUS_HEAD: [u8; 4] = [0x88, 0x54, 0x24, 0x10];
    static CHECKED: OnceLock<bool> = OnceLock::new();
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let known = *CHECKED.get_or_init(|| {
        let head = unsafe { ((exe + SET_FOCUS) as *const [u8; 4]).read_unaligned() };
        let ok = head == SET_FOCUS_HEAD;
        if !ok {
            log!("galaxy: unknown executable at +{SET_FOCUS:#x} ({head:02x?}): no game viewport focus");
        }
        ok
    });
    let app = unsafe { *((exe + APPLICATION) as *const usize) };
    if !known || app == 0 {
        return;
    }
    // EFocusCause::SetDirectly
    let set: unsafe extern "C" fn(usize, u8) = unsafe { std::mem::transmute(exe + SET_FOCUS) };
    unsafe { set(app, 2) };
}

/// The touch screen's window buttons (`UCPP_WindowButtonWidget`: card screen's yes / no...)
/// decide on a release only while hovered (+0x41B, set by `OnHoveredFunction`, +0x1E08890);
/// Slate never hovers them from the payload's mouse (its press and release reach them). A press
/// (`OnPressedFunction`, +0x1E08910, detour) of a button not hovered hovers it first, as a
/// finger on the cabinet's panel does.
fn hover_on_press(exe: usize) {
    const PRESSED: usize = 0x1e08910;
    const HOVERED: usize = 0x1e08890;
    const RELEASED: usize = 0x1e09480;
    const CLICKED: usize = 0x1e07c30;
    // push rbx; push rbp; push rsi; push rdi; sub rsp, 0xb8 / push rbx; sub rsp, 0x30
    let known = unsafe { ((exe + PRESSED) as *const [u8; 12]).read_unaligned() } == [0x40, 0x53, 0x55, 0x56, 0x57, 0x48, 0x81, 0xec, 0xb8, 0x00, 0x00, 0x00]
        && unsafe { ((exe + HOVERED) as *const [u8; 6]).read_unaligned() } == [0x40, 0x53, 0x48, 0x83, 0xec, 0x30];
    if !known {
        log!("galaxy: unknown executable at +{PRESSED:#x}: window buttons not hovered on press");
        return;
    }
    PRESSED_ORIGINAL.store(detour(exe + PRESSED, 12, button_pressed as *const () as usize), Ordering::Relaxed);
    RELEASED_ORIGINAL.store(detour(exe + RELEASED, 12, button_released as *const () as usize), Ordering::Relaxed);
    CLICKED_AT.store(exe + CLICKED, Ordering::Relaxed);
    HOVERED_AT.store(exe + HOVERED, Ordering::Relaxed);
}

static PRESSED_ORIGINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static HOVERED_AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static RELEASED_ORIGINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CLICKED_AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The button the payload's touch pressed (its release then clicks it).
static PRESSED_BUTTON: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn button_released(this: usize) {
    let released: unsafe extern "C" fn(usize) = unsafe { std::mem::transmute(RELEASED_ORIGINAL.load(Ordering::Relaxed)) };
    unsafe { released(this) };
    if PRESSED_BUTTON.swap(0, Ordering::Relaxed) == this {
        let clicked: unsafe extern "C" fn(usize) = unsafe { std::mem::transmute(CLICKED_AT.load(Ordering::Relaxed)) };
        unsafe { clicked(this) };
    }
}

unsafe extern "C" fn button_pressed(this: usize) {
    PRESSED_BUTTON.store(this, Ordering::Relaxed);
    if unsafe { *((this + 0x41b) as *const u8) } == 0 {
        let hovered: unsafe extern "C" fn(usize) = unsafe { std::mem::transmute(HOVERED_AT.load(Ordering::Relaxed)) };
        unsafe { hovered(this) };
    }
    let pressed: unsafe extern "C" fn(usize) = unsafe { std::mem::transmute(PRESSED_ORIGINAL.load(Ordering::Relaxed)) };
    unsafe { pressed(this) }
}

/// The cabinet check: `UCPP_GameInstance::OnViewportCreate` (+0x2804F60) sets the instance's
/// cabinet flag (+0x4B3, read as "USBIO exist" by `ACPP_GameModeBoot::InitHardware`) only for
/// two monitors (`CPP_MonitorResolutions::GetCaps`: `GetSystemMetrics(SM_CMONITORS)` of 2, a
/// 1024x768 one, the main one 1920x1080). Without it the I/O boards' tick never starts: the
/// levers and switches never become the game's input. The flag is always set. And no motion
/// seat: `UGALAXYIOWrapper::IsMachineSeat` (+0x2A0FF00, the machine ID 0 and 2-4 with the boards)
/// answers no (else the boot waits for the seat's servos, "可動シートに着座中です").
fn cabinet(exe: usize) {
    const FLAG: usize = 0x2804f92;
    const SEAT: usize = 0x2a0ff00;
    // mov [rcx+0x4b3], dil / mov [rsp+8], rcx; sub rsp, 0x38
    let known = unsafe { ((exe + FLAG) as *const [u8; 7]).read_unaligned() } == [0x40, 0x88, 0xb9, 0xb3, 0x04, 0x00, 0x00]
        && unsafe { ((exe + SEAT) as *const [u8; 9]).read_unaligned() } == [0x48, 0x89, 0x4c, 0x24, 0x08, 0x48, 0x83, 0xec, 0x38];
    if !known {
        log!("galaxy: unknown executable at +{FLAG:#x} / +{SEAT:#x}: cabinet check left as it is");
        return;
    }
    // mov byte [rcx+0x4b3], 1 / xor eax, eax; ret
    wal_payload_common::patches::apply_list(&format!("{FLAG:x}:C681B304000001,{SEAT:x}:31C0C3"));
}

/// The cabinet's NESiCA reception hours (the test menu's, the dump's 8:00 to 20:45): the shop
/// is always open (`UTestModeWork::IsNesicaReceptionTime`, +0x27FF360: `start <= now < end -
/// margin`, "閉店中です" and no game outside them) answers yes.
fn all_day(exe: usize) {
    const AT: usize = 0x27ff360;
    // mov [rsp+0x10], edx
    if unsafe { ((exe + AT) as *const [u8; 4]).read_unaligned() } != [0x89, 0x54, 0x24, 0x10] {
        log!("galaxy: unknown executable at +{AT:#x}: NESiCA reception hours left as they are");
        return;
    }
    // mov al, 1; ret
    wal_payload_common::patches::apply_list(&format!("{AT:x}:B001C3"));
}

/// Jumps from `at` to `hook`; the `len` bytes there (whole instructions, none relative) run
/// first in the returned trampoline.
fn detour(at: usize, len: usize, hook: usize) -> usize {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
    let tramp = unsafe { VirtualAlloc(std::ptr::null(), 64, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(at as *const u8, tramp, len);
        // jmp [rip]; dq at + len
        let jmp = tramp.add(len);
        std::ptr::copy_nonoverlapping([0xffu8, 0x25, 0, 0, 0, 0].as_ptr(), jmp, 6);
        std::ptr::write_unaligned(jmp.add(6) as *mut u64, (at + len) as u64);
    }
    // mov rax, hook; jmp rax
    let mut code = vec![0x48, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xe0];
    code[2..10].copy_from_slice(&(hook as u64).to_le_bytes());
    code.resize(len, 0x90);
    let spec = format!("{:x}:{}", at - unsafe { GetModuleHandleW(std::ptr::null()) } as usize, code.iter().map(|b| format!("{b:02X}")).collect::<String>());
    wal_payload_common::patches::apply_list(&spec);
    tramp as usize
}

unsafe extern "system" fn set_window_feedback_setting(_hwnd: *mut u8, _feedback: u32, _flags: u32, _size: u32, _config: *const u8) -> i32 {
    1
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

unsafe extern "system" fn xinput_get_state(_index: u32, _state: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

unsafe extern "system" fn xinput_set_state(_index: u32, _vibration: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

/// The sticks' calibration file of the D: data, read once (the game writes it from its test
/// menu): `LBasePosX`, `LMinPosX`, `LMaxPosX`... A missing key keeps the default.
fn sticks() -> [Calibration; 2] {
    *STICKS.get_or_init(|| {
        let mut cal = [Calibration { base: [0x8000; 2], min: [-0x4000; 2], max: [0x4000; 2] }; 2];
        let path = format!("{}\\Saved\\ACRSaved\\TestMode\\Stick\\StickData.json", crate::drive::data_dir());
        let Ok(text) = std::fs::read_to_string(&path) else {
            log!("galaxy: no stick calibration ({path}): centred sticks");
            return cal;
        };
        let value = |key: &str| -> Option<i32> {
            let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
            let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
            let end = rest.find(|c: char| c != '-' && !c.is_ascii_digit()).unwrap_or(rest.len());
            rest[..end].parse().ok()
        };
        for (side, c) in ["L", "R"].iter().zip(cal.iter_mut()) {
            for (axis, xy) in ["X", "Y"].iter().enumerate() {
                if let (Some(b), Some(lo), Some(hi)) =
                    (value(&format!("{side}BasePos{xy}")), value(&format!("{side}MinPos{xy}")), value(&format!("{side}MaxPos{xy}")))
                {
                    c.base[axis] = b;
                    c.min[axis] = lo;
                    c.max[axis] = hi;
                }
            }
        }
        log!(
            "galaxy: stick calibration L base {:?} min {:?} max {:?}, R base {:?} min {:?} max {:?}",
            cal[0].base, cal[0].min, cal[0].max, cal[1].base, cal[1].min, cal[1].max
        );
        cal
    })
}

/// The game's value of an axis (-32767..32767) under calibration `c`, kept a little inside its
/// extremes.
fn calibrated(c: &Calibration, axis: usize, v: i16) -> u16 {
    let v = i32::from(v);
    let offset = if v < 0 { -c.min[axis] * v / 32767 } else { c.max[axis] * v / 32767 };
    (c.base[axis] + offset * 95 / 100).clamp(0, 0xffff) as u16
}

/// The touch screen's pointer: the mouse (the profile's `input.gun` on the pedal axes: its whole
/// range the sub monitor; no stick nor key moves it); shown on the sub monitor's picture for a
/// while after it moved.
struct Pointer {
    /// Its place on the sub monitor (fractions).
    at: (f32, f32),
    /// The pedals at the last frame.
    pedals: Option<(i16, i16)>,
    moved: Option<std::time::Instant>,
}

static POINTER: std::sync::Mutex<Pointer> = std::sync::Mutex::new(Pointer { at: (0.5, 0.5), pedals: None, moved: None });

/// How long the pointer stays shown after its last move.
const POINTER_SHOWN: std::time::Duration = std::time::Duration::from_secs(3);

/// The pointer's place on the sub monitor this frame (fractions) and whether it is shown.
fn pointer(stick: &wal_protocol::StickState, touching: bool) -> ((f32, f32), bool) {
    let now = std::time::Instant::now();
    let pedals = (stick.axis(Axis::Accel), stick.axis(Axis::Brake));
    let mut p = POINTER.lock().unwrap();
    if p.pedals.is_some_and(|was| was != pedals) {
        let full = |v: i16| (f32::from(v.max(0)) / 32767.0).min(0.999);
        (p.at, p.moved) = ((full(pedals.0), full(pedals.1)), Some(now));
    }
    p.pedals = Some(pedals);
    let shown = touching || p.moved.is_some_and(|t| t.elapsed() < POINTER_SHOWN);
    (p.at, shown)
}

/// The finger on the sub monitor this frame (fractions of it): the touch surface's first finger
/// (the profile's `input.touch`: the whole pad is the sub monitor), else the `touch` input held
/// at the pointer.
fn finger(stick: &wal_protocol::StickState, touch: bool) -> Option<(f32, f32)> {
    let (at, shown) = pointer(stick, touch);
    windows::set_pointer(shown.then_some(at));
    if let Some((x, y)) = wal_payload_common::touches().first() {
        let frac = |v: u16| (f32::from(v) / 65535.0).min(0.999);
        return Some((frac(*x), frac(*y)));
    }
    touch.then_some(at)
}

static COIN_HELD: AtomicBool = AtomicBool::new(false);
static COINS: AtomicU32 = AtomicU32::new(0);
static LAST_SWITCHES: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn GALAXYIO_Init(error: *mut i32) -> u32 {
    log!("galaxy: GALAXYIO_Init");
    if !error.is_null() {
        unsafe { error.write(0) };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn GALAXYIO_Delete() -> u32 {
    0
}

/// Both boards running.
#[unsafe(no_mangle)]
pub extern "C" fn GALAXYIO_GetStatus(_main: i32) -> u32 {
    0x200
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn GALAXYIO_Update(buffer: *mut u8, error: *mut u32) -> u32 {
    if buffer.is_null() {
        return 0;
    }
    if !error.is_null() {
        unsafe { error.write(0) };
    }
    let stick = wal_payload_common::input(0);
    let mut switches = 0u32;
    let mut coin = false;
    let mut thumbs = [[0i32; 2]; 2];
    let mut touch = false;
    let mut wheel = false;
    let mut sub_toggle = false;
    if let Some(map) = MAP.get() {
        map.for_each_pressed(&stick, |n| match n {
            Native::Touch => touch = true,
            Native::Wheel => wheel = true,
            Native::SubToggle => sub_toggle = true,
            Native::Switch(bit) => switches |= 1 << bit,
            Native::Coin => coin = true,
            Native::Thumb(t, x, y) => {
                thumbs[t][0] += x;
                thumbs[t][1] += y;
            }
        });
    }
    windows::keep_hidden();
    static SUB_TOGGLE_HELD: AtomicBool = AtomicBool::new(false);
    if sub_toggle && !SUB_TOGGLE_HELD.swap(true, Ordering::Relaxed) {
        windows::toggle_shown();
    } else if !sub_toggle {
        SUB_TOGGLE_HELD.store(false, Ordering::Relaxed);
    }
    let finger = finger(&stick, touch);
    windows::touch(finger);
    // the focus back to the game viewport a while after the last touch (its click delivered:
    // the touch screen's buttons act on the release, their press cancelled without the focus)
    static LAST_TOUCH: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST_TOUCH.lock().unwrap();
    if finger.is_some() {
        *last = Some(std::time::Instant::now());
    } else if last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(1)) {
        game_viewport_focus();
    }
    drop(last);
    // a coin per press, on coin slot 1's counter
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    if LAST_SWITCHES.swap(switches, Ordering::Relaxed) != switches {
        log!("galaxy: switches {switches:#010x}");
    }
    // the levers, logged when one crosses half way
    static LAST_LEVERS: AtomicU32 = AtomicU32::new(0);
    let half = |a: Axis| match stick.axis(a) {
        v if v > 16384 => 1u32,
        v if v < -16384 => 2,
        _ => 0,
    };
    let levers = half(Axis::LeftX) | half(Axis::LeftY) << 2 | half(Axis::RightX) << 4 | half(Axis::RightY) << 6;
    if LAST_LEVERS.swap(levers, Ordering::Relaxed) != levers {
        log!("galaxy: levers left {},{} right {},{}", stick.axis(Axis::LeftX), stick.axis(Axis::LeftY), stick.axis(Axis::RightX), stick.axis(Axis::RightY));
    }
    let sticks = sticks();
    let full = |d: i32| (d.signum() * 32767) as i16;
    let mut analog = [0u16; 16];
    for t in 0..2 {
        for axis in 0..2 {
            analog[8 + 2 * t + axis] = calibrated(&THUMB, axis, full(thumbs[t][axis]));
        }
    }
    // the sticks (their raw values read inverted by the game), as the axis mappings want them:
    // right the X axes' positive side (MoveRight, TurnRate: scale 1), forward the left Y axis'
    // (MoveForward: scale 1), the right Y axis' negative one (LookUpRate: scale 1, the
    // gamepad's scale -1)
    let axes = [[Axis::LeftX, Axis::LeftY], [Axis::RightX, Axis::RightY]];
    for (s, c) in sticks.iter().enumerate() {
        for axis in 0..2 {
            let v = stick.axis(axes[s][axis]);
            let v = if (s, axis) == (1, 1) { v } else { v.saturating_neg() };
            analog[12 + 2 * s + axis] = 0xffff - calibrated(c, axis, v);
        }
    }
    analog.copy_within(8..16, 0);
    // the wheel's encoder (InEncoder[4]): a notch per press
    static WHEEL_HELD: AtomicBool = AtomicBool::new(false);
    static WHEEL: AtomicU32 = AtomicU32::new(0);
    if wheel && !WHEEL_HELD.swap(true, Ordering::Relaxed) {
        WHEEL.fetch_add(12, Ordering::Relaxed);
    } else if !wheel {
        WHEEL_HELD.store(false, Ordering::Relaxed);
    }
    let coins = (COINS.load(Ordering::Relaxed) & 0x3fff) as u16;
    let card = crate::rfid::card_inserted();
    unsafe {
        (buffer as *mut u32).write_unaligned(switches ^ ACTIVE_LOW);
        for (i, v) in analog.iter().enumerate() {
            (buffer.add(4 + 2 * i) as *mut u16).write_unaligned(*v);
        }
        (buffer.add(0x24 + 2 * 4) as *mut u16).write_unaligned(WHEEL.load(Ordering::Relaxed) as u16);
        (buffer.add(0xc0) as *mut u16).write_unaligned(coins);
        (buffer.add(0xc2) as *mut u16).write_unaligned(0);
        // the game's coin decrements, consumed
        let dec = buffer.add(0xc4) as *mut i16;
        let n = dec.read_unaligned();
        if n > 0 {
            COINS.fetch_sub(u32::from(n as u16).min(COINS.load(Ordering::Relaxed)), Ordering::Relaxed);
            dec.write_unaligned(0);
        }
        static COMMAND: AtomicU32 = AtomicU32::new(0xffff_ffff);
        let command = u32::from(*buffer.add(0xc8));
        if COMMAND.swap(command, Ordering::Relaxed) != command {
            log!("galaxy: reader command {command:#04x}");
        }
        // the reader reading: the card's moment (WAL_NESICA_CARD_AUTO)
        if command & 0x40 != 0 {
            crate::rfid::card_requested();
        }
        let reading = command & 0x40 != 0;
        *buffer.add(0xc9) = match (reading, card) {
            (true, true) => 0x11,
            (true, false) => 0x20,
            _ => 0,
        };
        let id = crate::rfid::card_id();
        let mut serial = [0u8; 16];
        for (d, c) in serial.iter_mut().zip(id.bytes()) {
            *d = c;
        }
        std::ptr::copy_nonoverlapping(serial.as_ptr(), buffer.add(0xd1), 16);
        // MACHINE_ID 0
        *buffer.add(0xf5) = 0;
    }
    static CARD: AtomicBool = AtomicBool::new(false);
    if CARD.swap(card, Ordering::Relaxed) != card {
        log!("galaxy: NESiCA {}", if card { "on the reader" } else { "removed" });
    }
    0
}
