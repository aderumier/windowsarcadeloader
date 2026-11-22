//! Taito's Half-Life 2 arcade framework (Half-Life 2: Survivor, `WAL_TYPEX_HL2AC=1`): the
//! game's settings live in `ac.dll`'s `g_Global` (`CHL2Global`), filled by `bin\launcher.dll`
//! from `D:\config.bin` (the test menu's settings) before it opens the NESYS income
//! (`NesysIncomeOpen`).
//!
//! Free play (common option, `WAL_FREEPLAY`): `m_bFreePlay` (+0x504) set then, with the coin
//! weight (`m_CreditWeight`, +0x10, coins a credit) 0: the launcher's own free play (it sets
//! the flag for a weight of 0 in `config.bin`); the flag alone left the game start screen
//! waiting for a credit.
//!
//! Test menu: on the test switch `ac.dll` starts `testmode.exe` of the game's folder and
//! terminates the game. The launcher's game is the program it started, so the launcher starts
//! the test menu once the game is gone (`exe=testmode.exe`), its start answered as done. The
//! test menu's exit restarts the cabinet (`ExitWindowsEx`): the game again (the switcher's).
//!
//! Input only from the launcher: the real keyboard's messages and key states filtered in the
//! engine's message loops (late DLLs). The engine's mouse look polls the cursor and puts it
//! back at the window's center: its `GetCursorPos` answers the point its last `SetCursorPos`
//! asked for (no motion; not `-nomouse`: the arcade client reads the cabinet's sticks and
//! switches in that mouse input, nothing moved or fired without it).
//!
//! Movement and buttons: player 1's left stick posts the engine's movement keys (W / S / A / D,
//! its binds) past half way, b1 / b2 its mouse buttons (fire, alt fire), b3 / b4 / b5 Ctrl /
//! Space / R (duck, jump, reload), marked for the filter: the client's own stick movement and cabinet switches
//! never moved the player or fired.
//!
//! Aim: the cabinet's head stick (up / down, its twist turning the head) on the JVS channels 1 / 2
//! moved nothing (as the movement stick), the engine's mouse look does. Its cursor polls answer
//! the point it centered the cursor at plus the motion since the last poll: the launcher's
//! mouse (a counter on player 1's pedal axes, `accel` x, `brake` y, wrapping; times
//! `WAL_TYPEX_HL2AC_MOUSE` percent, 0 off) and player 1's right stick (its tilt). The real
//! mouse's moves never reach it (window messages dropped, no DirectInput device).
//!
//! Sticks: launcher.dll's factory calibration (aim 0x8000..0x8100, movement 0x200..0) does not
//! fit the board's raw 16-bit channels (rest 0x8000): replaced by their full range, as long as
//! the test menu's ADJUSTMENT has not calibrated them.
//!
//! The attract movies' info windows (`movinfowin_top` 256x64 at the top left,
//! `movinfowin_bottom` 576x64 at the bottom right, black GDI popups over the game) are created
//! visible, and `ac.dll` shows / hides them only when its "movie info" flag changes, off at the
//! start: two black boxes over the game and the coin line. Created hidden.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, late_hooks, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

const CREDIT_WEIGHT: usize = 0x10;
const FREE_PLAY: usize = 0x504;
const STICK_LIMITS: usize = 0x508;
/// launcher.dll's default config.bin calibration (each channel min / max, raw JVS values).
const FACTORY_LIMITS: [(i32, i32); 4] = [(0x8000, 0x8100), (0x8000, 0x8100), (0x200, 0), (0x200, 0)];
const TEST_MENU: &str = "testmode.exe";

static ORIG_INCOME_OPEN: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_CURSOR_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_CURSOR_POS: AtomicUsize = AtomicUsize::new(0);
/// The point the engine last put the cursor at (x << 32 | y), `u64::MAX` before.
static CURSOR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

type CreateWindowExA = unsafe extern "system" fn(u32, *const u8, *const u8, u32, i32, i32, i32, i32, usize, usize, usize, usize) -> usize;
type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, usize, usize, i32, u32, usize, *const u8, usize, usize) -> i32;

pub fn init() {
    if !std::env::var("WAL_TYPEX_HL2AC").is_ok_and(|v| v == "1") {
        return;
    }
    if wal_payload_common::patches::exe_name().eq_ignore_ascii_case(TEST_MENU) {
        let _ = unsafe { iat::hook("user32.dll", "ExitWindowsEx", crate::switcher::exit_windows as *const () as usize) };
        log!("hl2ac: test menu, its exit starts the game again");
        return;
    }
    late_hooks::hook("nesys.dll", "?NesysIncomeOpen@@YA_NXZ", income_open as *const () as usize, &ORIG_INCOME_OPEN);
    log!("hl2ac: settings applied before NesysIncomeOpen");
    // the real keyboard and mouse never reach the engine (its key binds: console, Escape...)
    wal_payload_common::input_filter::init(0, key_held);
    wal_payload_common::input_filter::messages_late();
    // the real mouse's moves dropped too (the engine's mouse look is the payload's)
    wal_payload_common::input_filter::own_mouse(0);
    late_hooks::hook("user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize, &ORIG_GET_CURSOR_POS);
    late_hooks::hook("user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize, &ORIG_SET_CURSOR_POS);
    log!("hl2ac: real keyboard and mouse filtered");
    std::thread::spawn(binds);
    let speed = std::env::var("WAL_TYPEX_HL2AC_MOUSE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
    if speed > 0.0 {
        MOUSE.lock().unwrap().speed = speed;
        log!("hl2ac: mouse aim, speed {speed}");
    }
}

/// The engine's cursor polls: where it last put it (the real one before its first move).
unsafe extern "system" fn get_cursor_pos(point: *mut windows_sys::Win32::Foundation::POINT) -> i32 {
    let c = CURSOR.load(Ordering::Relaxed);
    if c == u64::MAX {
        let orig: unsafe extern "system" fn(*mut windows_sys::Win32::Foundation::POINT) -> i32 =
            unsafe { std::mem::transmute(ORIG_GET_CURSOR_POS.load(Ordering::Relaxed)) };
        let ok = unsafe { orig(point) };
        if ok == 0 && !point.is_null() {
            // no pointer (a headless gamescope): the screen's center, the mouse input working
            log!("hl2ac: no cursor, the screen's center answered");
            unsafe { *point = windows_sys::Win32::Foundation::POINT { x: 680, y: 384 } };
            return 1;
        }
        return ok;
    }
    if !point.is_null() {
        let (dx, dy) = aim_motion();
        unsafe { *point = windows_sys::Win32::Foundation::POINT { x: (c >> 32) as i32 + dx, y: c as u32 as i32 + dy } };
    }
    1
}

/// The engine's cursor moves: remembered, and done (a cursor read another way, or before the
/// first move, stays where the engine expects it: the real cursor left alone, a poll elsewhere
/// saw it off the center for good, the view spun).
unsafe extern "system" fn set_cursor_pos(x: i32, y: i32) -> i32 {
    CURSOR.store((x as u32 as u64) << 32 | y as u32 as u64, Ordering::Relaxed);
    let orig: unsafe extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(ORIG_SET_CURSOR_POS.load(Ordering::Relaxed)) };
    unsafe { orig(x, y) }
}

/// The engine keys held by the payload (bits: [`KEYS`]).
static KEYS_HELD: AtomicUsize = AtomicUsize::new(0);
/// The engine's binds the virtual stick presses: W forward, S back, A / D strafe (the left
/// stick past half way), Ctrl duck (b3), Space jump (b4), R reload (b5); virtual keys.
const KEYS: [u16; 7] = [b'W' as u16, b'S' as u16, b'A' as u16, b'D' as u16, 0x11, 0x20, b'R' as u16];
/// The first bit of the mouse buttons in the held set (after [`KEYS`]).
const MOUSE_BIT: usize = KEYS.len();
/// Its mouse buttons: MOUSE1 fire (b1), MOUSE2 alt fire (b2).
const BUTTONS: [(u32, u32, usize); 2] = [(0x0201, 0x0202, 0x0001), (0x0204, 0x0205, 0x0002)];

/// The keys the engine sees held: those the payload pressed (the mouse buttons too).
fn key_held(vk: i32) -> bool {
    let held = KEYS_HELD.load(Ordering::Relaxed);
    KEYS.iter().enumerate().any(|(i, &k)| held & 1 << i != 0 && k as i32 == vk)
        || (vk == 0x01 && held & 1 << MOUSE_BIT != 0)
        || (vk == 0x02 && held & 1 << (MOUSE_BIT + 1) != 0)
}

/// Player 1's virtual stick as the engine's PC binds: the arcade client's own stick movement
/// and cabinet switches (JVS analog channels 3 / 4, switches) never moved the player or fired,
/// the binds did (keys and mouse buttons posted to its window, marked for the filter).
fn binds() {
    use wal_protocol::{Axis, button};
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowA, GetWindowThreadProcessId};
    let mut window: windows_sys::Win32::Foundation::HWND = std::ptr::null_mut();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if window.is_null() {
            let w = unsafe { FindWindowA(c"Valve001".as_ptr().cast(), std::ptr::null()) };
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(w, &mut pid) };
            if w.is_null() || pid != std::process::id() {
                continue;
            }
            window = w;
            log!("hl2ac: binds to window {window:?}");
        }
        let stick = wal_payload_common::input(0);
        let (x, y) = (stick.axis(Axis::LeftX) as i32, stick.axis(Axis::LeftY) as i32);
        let wanted = (y < -16384) as usize
            | ((y > 16384) as usize) << 1
            | ((x < -16384) as usize) << 2
            | ((x > 16384) as usize) << 3
            | (stick.pressed(button::B3) as usize) << 4
            | (stick.pressed(button::B4) as usize) << 5
            | (stick.pressed(button::B5) as usize) << 6
            | (stick.pressed(button::B1) as usize) << MOUSE_BIT
            | (stick.pressed(button::B2) as usize) << (MOUSE_BIT + 1);
        let held = KEYS_HELD.swap(wanted, Ordering::Relaxed);
        let changed = held ^ wanted;
        for (i, &key) in KEYS.iter().enumerate() {
            if changed & 1 << i != 0 {
                wal_payload_common::input_filter::post_key(window, key, wanted & 1 << i != 0);
            }
        }
        for (i, &(down, up, flag)) in BUTTONS.iter().enumerate() {
            if changed & 1 << (MOUSE_BIT + i) != 0 {
                let pressed = wanted & 1 << (MOUSE_BIT + i) != 0;
                // the window's center (the engine's cursor)
                wal_payload_common::input_filter::post_mouse(window, if pressed { down } else { up }, 680, 384, if pressed { flag } else { 0 });
            }
        }
    }
}

struct Mouse {
    /// Cursor pixels a mouse count, in percent.
    speed: f32,
    /// The mouse's counters at the last cursor poll (x, y).
    last: [Option<i16>; 2],
    /// The motion not handed yet (fractions of a pixel: the engine's look is very sensitive).
    rest: [f32; 2],
}

static MOUSE: std::sync::Mutex<Mouse> = std::sync::Mutex::new(Mouse { speed: 0.0, last: [None; 2], rest: [0.0; 2] });

/// Cursor pixels a poll at the right stick's full tilt.
const STICK_SPEED: f32 = 0.25;

/// The cursor's motion since the engine's last poll: the launcher's mouse (its counters on
/// `accel` / `brake` moved) and player 1's right stick (its tilt).
fn aim_motion() -> (i32, i32) {
    use wal_protocol::Axis;
    let stick = wal_payload_common::input(0);
    let mut mouse = MOUSE.lock().unwrap();
    let mut motion = [0.0f32; 2];
    for (i, (counter, tilt)) in [(Axis::Accel, Axis::RightX), (Axis::Brake, Axis::RightY)].into_iter().enumerate() {
        let counter = stick.axis(counter);
        // the counter wraps (2 units a count): the shortest way
        let moved = mouse.last[i].map_or(0, |last| counter.wrapping_sub(last)) as f32 / 2.0;
        mouse.last[i] = Some(counter);
        let tilt = stick.axis(tilt) as f32 / 32768.0;
        let tilt = if tilt.abs() < 0.15 { 0.0 } else { tilt };
        let total = mouse.rest[i] + moved * mouse.speed / 100.0 + tilt * STICK_SPEED;
        motion[i] = total.trunc();
        mouse.rest[i] = total - motion[i];
    }
    (motion[0] as i32, motion[1] as i32)
}

/// The sticks' calibration (`g_Global` +0x508, 4 min / max pairs) still the launcher's factory
/// values (no test menu ADJUSTMENT yet): the JVS channels' full range instead, each pair's
/// direction kept.
unsafe fn full_range_sticks(g: *mut u8) {
    let adj = unsafe { g.add(STICK_LIMITS) } as *mut i32;
    let pairs: [(i32, i32); 4] = std::array::from_fn(|i| unsafe { (adj.add(2 * i).read_unaligned(), adj.add(2 * i + 1).read_unaligned()) });
    if pairs != FACTORY_LIMITS {
        log!("hl2ac: stick calibration {pairs:x?} kept");
        return;
    }
    for (i, &(a, b)) in pairs.iter().enumerate() {
        let (lo, hi) = if a < b { (0x0000, 0xFFC0) } else { (0xFFC0, 0x0000) };
        unsafe {
            adj.add(2 * i).write_unaligned(lo);
            adj.add(2 * i + 1).write_unaligned(hi);
        }
    }
    log!("hl2ac: factory stick calibration: the full range");
}

/// `ac.dll`'s `g_Global`.
fn global(ac: windows_sys::Win32::Foundation::HMODULE) -> Option<*mut u8> {
    unsafe { GetProcAddress(ac, c"?g_Global@@3VCHL2Global@@A".as_ptr().cast()) }.map(|p| p as *mut u8)
}

unsafe extern "C" fn income_open() -> bool {
    let ac = unsafe { GetModuleHandleA(c"ac.dll".as_ptr().cast()) };
    if ac.is_null() {
        log!("hl2ac: ac.dll not loaded");
    } else {
        if let Some(free) = wal_payload_common::free_play("WAL_TYPEX_FREEPLAY") {
            match global(ac) {
                Some(g) => {
                    unsafe { *g.add(FREE_PLAY) = free as u8 };
                    if free {
                        unsafe { (g.add(CREDIT_WEIGHT) as *mut u32).write_unaligned(0) };
                    }
                    log!("hl2ac: free play {}", if free { "on" } else { "off" });
                }
                None => log!("hl2ac: no g_Global in ac.dll"),
            }
        }
        if let Some(g) = global(ac) {
            unsafe { full_range_sticks(g) };
        }
        // the cursor polls of the DLLs launcher.dll imports (tier0.dll's: the engine's mouse
        // look), loaded with it, out of the late hooks' reach
        for dll in [c"tier0.dll", c"vstdlib.dll"] {
            let module = unsafe { GetModuleHandleA(dll.as_ptr().cast()) };
            if !module.is_null() {
                if let Some(o) = unsafe { iat::hook_module(module as usize, "user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize) } {
                    if o != get_cursor_pos as *const () as usize {
                        let _ = ORIG_GET_CURSOR_POS.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
                    }
                    log!("hl2ac: {:?}'s cursor polls hooked", dll);
                }
            }
        }
        if let Some(o) = unsafe { iat::hook_module(ac as usize, "kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
            ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
        }
        if let Some(o) = unsafe { iat::hook_module(ac as usize, "user32.dll", "CreateWindowExA", create_window as *const () as usize) } {
            ORIG_CREATE_WINDOW.store(o, Ordering::Relaxed);
        }
    }
    let original: unsafe extern "C" fn() -> bool = unsafe { std::mem::transmute(ORIG_INCOME_OPEN.load(Ordering::Relaxed)) };
    unsafe { original() }
}

/// `ac.dll`'s windows: the movie info windows created hidden.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_window(
    ex: u32,
    class: *const u8,
    name: *const u8,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: usize,
    menu: usize,
    instance: usize,
    param: usize,
) -> usize {
    const WS_VISIBLE: u32 = 0x1000_0000;
    // a class name, not an atom
    let movie_info = class as usize > 0xffff && unsafe { std::ffi::CStr::from_ptr(class.cast()) }.to_bytes().starts_with(b"movinfowin_");
    let style = if movie_info { style & !WS_VISIBLE } else { style };
    let orig: CreateWindowExA = unsafe { std::mem::transmute(ORIG_CREATE_WINDOW.load(Ordering::Relaxed)) };
    let hwnd = unsafe { orig(ex, class, name, style, x, y, w, h, parent, menu, instance, param) };
    if movie_info {
        log!("hl2ac: movie info window {hwnd:#x} created hidden");
    }
    hwnd
}

/// `ac.dll`'s program starts: the test menu started by the launcher after the game.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u8,
    cmd: *mut u8,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u8,
    si: usize,
    pi: usize,
) -> i32 {
    let line = if cmd.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(cmd.cast()) }.to_string_lossy().into_owned() };
    if line.trim().trim_matches('"').to_ascii_lowercase().ends_with(&format!("\\{TEST_MENU}")) {
        log!("hl2ac: {line:?}: the test menu after the game (launcher)");
        wal_payload_common::request_relaunch(&format!("exe={TEST_MENU}"));
        // the request sent before the game terminates itself
        std::thread::sleep(std::time::Duration::from_millis(200));
        unsafe { crate::switcher::fake_process(pi) };
        return 1;
    }
    let orig: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}
