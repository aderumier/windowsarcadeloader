//! Quake: Arcade Tournament Edition (Lazer-Tron / LBE Systems, 1998: `glquake.exe` 1.30 r3,
//! GLQuake 1.07 with the arcade's accounting, menus and tickets; OpenGL through the dump's 3dfx
//! MiniGL, 32-bit; the profile runs `glquake_decrypted.exe`, the same program without its
//! Sentinel dongle's protection, with `-basedir .`: the map path overflowed a 64-byte buffer
//! with the run directory's): `WAL_ARCADEPC_BOARD=quakearcade`.
//!
//! **Controls.** The cabinet's controls went through the GCI (Game Control Interface) serial
//! board on COM1 or COM2: a trackball (the look, the GCI's yaw and pitch) and buttons and a
//! joystick as Quake's keys `AUX1`..`AUX32` (`ID1/config.cfg`: `AUX1` attack, `AUX2` next
//! weapon, `AUX3` jump, `AUX4`..`AUX7` strafe right, back, forward, strafe left, `AUX25` a coin,
//! `AUX30` the operator menu; `menukey`, which takes only these arcade keys, makes them the
//! menus' Enter, Escape and arrows). Without the board the game says OUT OF ORDER. The profile
//! starts it with `-nogci` (the board off, its `gciarcade` cvar 0): the keyboard and mouse of
//! GLQuake. The keyboard's keys are its scan codes (`MapKey`, the table `scantokey` at
//! 0x46ec60, 128 bytes, the codes 0x59..0x7f unused): the payload maps codes 0x60..0x7f to
//! `AUX1`..`AUX32` and posts them from the virtual sticks, the cabinet's binds and menu keys
//! unchanged. The right stick moves the mouse (the game's cursor polls; `+mlook`, added to the
//! command line: the mouse look, the cabinet's trackball). The real keyboard and mouse never
//! reach the game.
//!
//! **Movies.** The attract's movies (`avis\*.avi`, Microsoft Video 1, MCI) play in a second
//! full-screen window, "Quake AVI", created visible over the game's: on the cabinet the 3dfx
//! card drew the game on its own output, the desktop (that window) was never seen; with a
//! desktop OpenGL it hides the game. The payload creates it hidden and shows it over the game
//! from each movie's `MCI_PLAY` to its `MCI_STOP` / `MCI_CLOSE`; each movie opened is fitted
//! to that window (`MCI_PUT` of its window and picture: MCIAVI draws 640x480 in the corner of
//! the window the profile scales). In free play (free time) the game plays no movie.
//!
//! **Free play** (the common `WAL_FREEPLAY`, on by default): the operator's free time
//! (`a_freetime`, a toggle, 0x47e718: "Free Play" in place of the credits, attack plays
//! without a credit), added to the command line. (`a_freeplay`, the other operator key, gives
//! one credit.)
//!
//! Natives: `forward`, `back`, `strafe_left`, `strafe_right`, `attack`, `weapon`, `jump`,
//! `coin`, `operator`, `aux1`..`aux32` (each arcade key).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_scan};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowA, HWND_TOP, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, WS_VISIBLE,
};

/// Quake's scan code to key table, `K_AUX1`.
const SCANTOKEY: usize = 0x46ec60;
const K_AUX1: u8 = 0xcf;
/// The scan code of `AUX1` (`AUX32` 0x7f).
const SCAN_AUX1: u8 = 0x60;
/// The virtual key of the messages (the game reads their scan code only).
const VK_AUX: u16 = 0xE8;

/// The arcade keys (1..32) of the cabinet's controls (`ID1/config.cfg`'s binds).
const NATIVES: &[(&str, u8)] = &[
    ("attack", 1),
    ("weapon", 2),
    ("jump", 3),
    ("strafe_right", 4),
    ("back", 5),
    ("forward", 6),
    ("strafe_left", 7),
    ("coin", 25),
    ("operator", 30),
    ("aux1", 1),
    ("aux2", 2),
    ("aux3", 3),
    ("aux4", 4),
    ("aux5", 5),
    ("aux6", 6),
    ("aux7", 7),
    ("aux8", 8),
    ("aux9", 9),
    ("aux10", 10),
    ("aux11", 11),
    ("aux12", 12),
    ("aux13", 13),
    ("aux14", 14),
    ("aux15", 15),
    ("aux16", 16),
    ("aux17", 17),
    ("aux18", 18),
    ("aux19", 19),
    ("aux20", 20),
    ("aux21", 21),
    ("aux22", 22),
    ("aux23", 23),
    ("aux24", 24),
    ("aux25", 25),
    ("aux26", 26),
    ("aux27", 27),
    ("aux28", 28),
    ("aux29", 29),
    ("aux30", 30),
    ("aux31", 31),
    ("aux32", 32),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "forward"),
    ("down", "back"),
    ("left", "strafe_left"),
    ("right", "strafe_right"),
    ("b1", "attack"),
    ("b2", "jump"),
    ("b3", "weapon"),
    ("b4", "weapon"),
    // the cabinet has no start: attack starts
    ("start", "attack"),
    ("coin", "coin"),
    ("service", "coin"),
    ("test", "operator"),
];

fn map() -> &'static ButtonMap<u8> {
    static MAP: OnceLock<ButtonMap<u8>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Scan codes 0x60..0x7f as `AUX1`..`AUX32`.
fn map_aux_keys() {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let table = SCANTOKEY as *mut u8;
    // the table's start: Escape, '1', '2'...
    if unsafe { std::slice::from_raw_parts(table, 4) } != [0, 0x1b, b'1', b'2'] {
        log!("quakearcade: scan code table not found, no arcade keys");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(table.cast(), 128, PAGE_READWRITE, &mut old);
        for n in 0..32u8 {
            *table.add((SCAN_AUX1 + n) as usize) = K_AUX1 + n;
        }
        VirtualProtect(table.cast(), 128, old, &mut old);
    }
}

/// The game's cursor: where it last put it (`SetCursorPos`, the window's center), and the
/// right stick's motion since.
static CURSOR_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);
/// The motion in 1/256 pixel.
static MOTION_X: AtomicIsize = AtomicIsize::new(0);
static MOTION_Y: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    if point.is_null() {
        return 0;
    }
    let dx = MOTION_X.swap(0, Ordering::Relaxed);
    let dy = MOTION_Y.swap(0, Ordering::Relaxed);
    // the fraction kept for the next poll
    MOTION_X.fetch_add(dx % 256, Ordering::Relaxed);
    MOTION_Y.fetch_add(dy % 256, Ordering::Relaxed);
    unsafe {
        *point = POINT { x: CURSOR_X.load(Ordering::Relaxed) + (dx / 256) as i32, y: CURSOR_Y.load(Ordering::Relaxed) + (dy / 256) as i32 };
    }
    1
}

unsafe extern "system" fn set_cursor_pos(x: i32, y: i32) -> i32 {
    CURSOR_X.store(x, Ordering::Relaxed);
    CURSOR_Y.store(y, Ordering::Relaxed);
    1
}

/// The right stick's full tilt, pixels a second (the game's `sensitivity 3`, `m_yaw 0.022`:
/// about 400 degrees a second).
const LOOK_SPEED: f64 = 6000.0;
/// The stick's dead zone (of 32767).
const DEAD_ZONE: f64 = 4000.0;
const PERIOD: Duration = Duration::from_millis(8);
/// The mouse's look, pixels per count (the profile's trackball: the mouse's position on the
/// pedal axes, `mouse_wrap`, one count a pedal unit).
const MOUSE_GAIN: f64 = 1.0;

/// The motion of a pedal axis carrying a wrapping trackball counter (0..32767) since `last`.
fn trackball(now: i16, last: &mut Option<i16>) -> isize {
    let was = last.replace(now);
    let Some(was) = was else { return 0 };
    // modulo 32768: the shortest way round
    let d = (now as i32 - was as i32).rem_euclid(32768);
    (if d >= 16384 { d - 32768 } else { d }) as isize
}

/// A stick axis, -1..1 past its dead zone, its response curved (fine aim near the center).
fn tilt(v: i16) -> f64 {
    let v = v as f64;
    if v.abs() < DEAD_ZONE {
        return 0.0;
    }
    let t = ((v.abs() - DEAD_ZONE) / (32767.0 - DEAD_ZONE)).min(1.0);
    t * t * v.signum()
}

/// The game's window ("GLQuake", class "WinQuake").
fn game_window() -> HWND {
    unsafe { FindWindowA(c"WinQuake".as_ptr().cast(), c"GLQuake".as_ptr().cast()) }
}

fn input_thread() {
    let mut was = 0u32;
    let (mut mouse_x, mut mouse_y) = (None, None);
    loop {
        std::thread::sleep(PERIOD);
        let hwnd = game_window();
        if hwnd.is_null() {
            continue;
        }
        let stick = input(0);
        let mut keys = 0u32;
        map().for_each_pressed(&stick, |aux| keys |= 1 << (aux - 1));
        for n in 0..32u8 {
            let (now, before) = (keys >> n & 1 != 0, was >> n & 1 != 0);
            if now != before {
                post_scan(hwnd, VK_AUX, SCAN_AUX1 + n, now);
            }
        }
        was = keys;
        // the look: the stick up looks up (the cabinet's m_pitch is negative, the trackball's
        // sense: a cursor moved down looks up)
        let step = LOOK_SPEED * PERIOD.as_secs_f64() * 256.0;
        MOTION_X.fetch_add((tilt(stick.axis(Axis::RightX)) * step) as isize, Ordering::Relaxed);
        MOTION_Y.fetch_add((-tilt(stick.axis(Axis::RightY)) * step) as isize, Ordering::Relaxed);
        // the mouse: a trackball on the pedal axes, its motion as the cursor's, vertically
        // inverted (the cabinet's m_pitch is negative): the mouse pushed away looks up
        let gain = MOUSE_GAIN * 256.0;
        MOTION_X.fetch_add((trackball(stick.axis(Axis::Accel), &mut mouse_x) as f64 * gain) as isize, Ordering::Relaxed);
        MOTION_Y.fetch_add((-trackball(stick.axis(Axis::Brake), &mut mouse_y) as f64 * gain) as isize, Ordering::Relaxed);
    }
}

/// The movies' window.
static AVI_WINDOW: AtomicUsize = AtomicUsize::new(0);
static CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static MCI_SEND_COMMAND: AtomicUsize = AtomicUsize::new(0);

type CreateWindowExA = unsafe extern "system" fn(u32, *const u8, *const u8, u32, i32, i32, i32, i32, HWND, *mut c_void, *mut c_void, *const c_void) -> HWND;

unsafe extern "system" fn create_window(
    ex_style: u32, class: *const u8, name: *const u8, mut style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: *mut c_void,
    instance: *mut c_void, param: *const c_void,
) -> HWND {
    let movies = !name.is_null() && (name as usize) >> 16 != 0 && unsafe { std::ffi::CStr::from_ptr(name.cast()) } == c"Quake AVI";
    if movies {
        style &= !WS_VISIBLE;
    }
    let f: CreateWindowExA = unsafe { std::mem::transmute(CREATE_WINDOW.load(Ordering::Relaxed)) };
    let hwnd = unsafe { f(ex_style, class, name, style, x, y, w, h, parent, menu, instance, param) };
    if movies {
        AVI_WINDOW.store(hwnd as usize, Ordering::Relaxed);
        log!("quakearcade: movie window {hwnd:?} hidden");
    }
    hwnd
}

const MCI_OPEN: u32 = 0x0803;
const MCI_CLOSE: u32 = 0x0804;
const MCI_PLAY: u32 = 0x0806;
const MCI_STOP: u32 = 0x0808;
const MCI_PUT: u32 = 0x0842;
const MCI_OPEN_ELEMENT: usize = 0x200;
const MCI_DGV_RECT: usize = 0x10000;
const MCI_DGV_PUT_DESTINATION: usize = 0x40000;
const MCI_DGV_PUT_WINDOW: usize = 0x200000;

/// `MCI_DGV_PUT_PARMS`: the rectangle's right and bottom are its width and height.
#[repr(C)]
struct PutParms {
    callback: usize,
    rect: windows_sys::Win32::Foundation::RECT,
}

/// A movie opened (`device`): its picture and its window the movies' window's size (MCIAVI
/// draws it at its own size, 640x480, in the window's corner).
unsafe fn fit_movie(f: MciSendCommandA, device: u32, movies: HWND) {
    let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(movies, &mut r) } == 0 || r.right <= 0 || r.bottom <= 0 {
        return;
    }
    for what in [MCI_DGV_PUT_WINDOW, MCI_DGV_PUT_DESTINATION] {
        let put = PutParms { callback: 0, rect: r };
        unsafe { f(device, MCI_PUT, what | MCI_DGV_RECT, &put as *const PutParms as usize) };
    }
}

type MciSendCommandA = unsafe extern "system" fn(u32, u32, usize, usize) -> u32;

unsafe extern "system" fn mci_send_command(device: u32, message: u32, flags: usize, params: usize) -> u32 {
    let f: MciSendCommandA = unsafe { std::mem::transmute(MCI_SEND_COMMAND.load(Ordering::Relaxed)) };
    let movies = AVI_WINDOW.load(Ordering::Relaxed) as HWND;
    if message == MCI_PLAY && !movies.is_null() {
        unsafe { SetWindowPos(movies, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
    }
    let r = unsafe { f(device, message, flags, params) };
    if message == MCI_OPEN && r == 0 && flags & MCI_OPEN_ELEMENT != 0 && params != 0 && !movies.is_null() {
        // MCI_OPEN_PARMS: callback, then the device id
        let opened = unsafe { *((params + 4) as *const u32) };
        unsafe { fit_movie(f, opened, movies) };
    }
    if matches!(message, MCI_STOP | MCI_CLOSE) && !movies.is_null() {
        unsafe { ShowWindow(movies, SW_HIDE) };
    }
    r
}

/// The command line, with ours after the game's.
static COMMAND_LINE: OnceLock<std::ffi::CString> = OnceLock::new();
static GET_COMMAND_LINE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_command_line() -> *const u8 {
    COMMAND_LINE
        .get_or_init(|| {
            let f: unsafe extern "system" fn() -> *const u8 = unsafe { std::mem::transmute(GET_COMMAND_LINE.load(Ordering::Relaxed)) };
            let game = unsafe { std::ffi::CStr::from_ptr(f().cast()) }.to_string_lossy().into_owned();
            let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(true);
            let line = format!("{game} +mlook{}", if free { " +a_freetime" } else { "" });
            log!("quakearcade: command line {line}");
            std::ffi::CString::new(line).unwrap_or_default()
        })
        .as_ptr()
        .cast()
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    map_aux_keys();
    // the real mouse's moves dropped; the cursor polls are ours
    own_mouse(0);
    unsafe {
        iat::hook("user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize);
        iat::hook("user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize);
        if let Some(o) = iat::hook("user32.dll", "CreateWindowExA", create_window as *const () as usize) {
            CREATE_WINDOW.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("winmm.dll", "mciSendCommandA", mci_send_command as *const () as usize) {
            MCI_SEND_COMMAND.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("kernel32.dll", "GetCommandLineA", get_command_line as *const () as usize) {
            GET_COMMAND_LINE.store(o, Ordering::Relaxed);
        }
    }
    std::thread::spawn(input_thread);
    log!("quakearcade: arcade keys and look from the virtual sticks, movie window managed");
}


