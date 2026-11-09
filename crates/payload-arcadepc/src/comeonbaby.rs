//! Come on Baby! (ExtraLine, 2000; `COBDEMO.exe`, Glide 2, 32-bit) and Come On Baby! 2 Winter
//! Championship (2003; `Cob2.exe`, DirectDraw / Direct3D 7, 32-bit):
//! `WAL_ARCADEPC_BOARD=comeonbaby`, x86 build. The addresses below are the first game's; the
//! sequel's same board and settings are at its own (`BUILDS`, picked by the code there).
//!
//! - **Board.** The cabinet's I/O is on the parallel port: the reader (`0x4126d0`, cdecl, a byte
//!   argument) writes the group to the data port (0x378), strobes the control port (0x37a) and
//!   returns the status port's (0x379) upper nibble, active low. Groups: 0 player 1's stick
//!   (bit 0 up, 1 down, 2 left, 3 right), 1 its buttons (A, B, start), 2 / 3 player 2's, 4 the
//!   operator's (0 test, 1 service, 2 coin), 8 the boot's I/O check (5: the board is there,
//!   else `IO_ERROR!`). The dump's `in` / `out` instructions are already NOPs; the reader jumps
//!   here (the lamps' writes do nothing). The dump's dgVoodoo `Glide2x.dll` (the profile uses
//!   gxvk's) imports `cob_patch.dll`, whose reader (the real keyboard) is already there when the
//!   payload loads at the entry point: the payload's jump replaces it too.
//! - **Operator menu.** Its select (`0x46fb10`, the service button) and the game's frame update
//!   (service: a credit) share one edge detector (`0x4112f0`), whose state flips at each call
//!   while the button is held: the frame update, called first, took a short press. The menu's
//!   call is the payload's own edge detector.
//! - **Settings.** `eep.rom` (0x5c bytes, `COB 2.0`): byte 0x0c the coins per credit to start
//!   (`0x530840`, 0 free play: the operator menu's 1..9, FREE PLAY), 0x0d to continue
//!   (`0x5307dc`). The common `WAL_FREEPLAY` sets both after the boot reads the file (the boot's
//!   `jmp 0x412a30` at `0x412c4a` goes through the payload): 0 when on, 1 when off (one coin per
//!   credit); unset, the file's.
//! - **Picture.** gxvk (the profile's `Glide2x.dll`) draws in the window `grSstWinOpen` gets:
//!   the game passes none (gxvk took its 0x0 window, a few pixels on the desktop) and runs no
//!   message loop (its window's messages, gxvk's included, were never handled: the picture went
//!   black). The game's `grSstWinOpen` gets a window of the payload, a borderless one with a
//!   thread and message loop of its own, the largest centered 4:3 rectangle of the screen (the
//!   profile's black bezel around); gxvk's sizing of it (its `SetWindowPos` / `MoveWindow`) keeps
//!   that rectangle. The game's window is hidden.
//! - **Protection.** The boot compares `c:\windows\win386p.swp` with the volume serial number of
//!   C: (`0x412790`): it returns 1 (the profile's patch).
//!
//! - **Come On Baby! 2.** The reader `0x41f6c0` (the same groups; its `in` / `out` are real,
//!   the profile makes the lamps' writers return), the settings reader `0x41fb20` (the jump at
//!   `0x41fd00`; `eep.rom` `COB 2.1`, the coins at the same offsets: `0x953664`, `0x951e50`).
//!   Its operator menu has an edge detector of its own (`0x753470`): no menu hook.
//!
//! Natives: `up down left right a b start` (each player), `coin service test`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// A build's addresses, relative to the executable.
struct Build {
    name: &'static str,
    /// The parallel port reader (cdecl, a group byte), starting with `mov dx, [port]`.
    reader: usize,
    /// The operator menu's call of the service edge detector, and its bytes.
    menu_select: Option<(usize, [u8; 5])>,
    /// The boot's jump to the settings reader, its bytes and the reader.
    settings_jump: (usize, [u8; 5], usize),
    /// The coins per credit to start and to continue (0: free play).
    start_coins: usize,
    continue_coins: usize,
}

const BUILDS: &[Build] = &[
    // Come on Baby! (COBDEMO.exe): the boot's tail `jmp 0x412a30`, the menu's `call 0x4112f0`
    Build {
        name: "Come on Baby!",
        reader: 0x126D0,
        menu_select: Some((0x6FB10, [0xE8, 0xDB, 0x17, 0xFA, 0xFF])),
        settings_jump: (0x12C4A, [0xE9, 0xE1, 0xFD, 0xFF, 0xFF], 0x12A30),
        start_coins: 0x130840,
        continue_coins: 0x1307DC,
    },
    // Come On Baby! 2 Winter Championship (Cob2.exe): the settings reader's `jmp 0x41fb20`
    Build {
        name: "Come On Baby! 2",
        reader: 0x1F6C0,
        menu_select: None,
        settings_jump: (0x1FD00, [0xE9, 0x1B, 0xFE, 0xFF, 0xFF], 0x1FB20),
        start_coins: 0x553664,
        continue_coins: 0x551E50,
    },
];

/// The reader's first bytes (`mov dx, [port]`).
const READER_CODE: [u8; 3] = [0x66, 0x8B, 0x15];
/// Group 8's answer: the board is there.
const IO_OK: u32 = 5;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the player's stick group.
    Stick(u8),
    /// A bit of the player's buttons group.
    Button(u8),
    Coin,
    /// A bit of the operator's group.
    Operator(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Stick(0)),
    ("down", Native::Stick(1)),
    ("left", Native::Stick(2)),
    ("right", Native::Stick(3)),
    ("a", Native::Button(0)),
    ("b", Native::Button(1)),
    ("start", Native::Button(2)),
    ("coin", Native::Coin),
    ("service", Native::Operator(1)),
    ("test", Native::Operator(0)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "a"),
    ("b2", "b"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A coin press stays on the wire this long, whatever the game's polling.
const COIN_HOLD: Duration = Duration::from_millis(150);

static LAST_COIN: Mutex<Option<Instant>> = Mutex::new(None);
static READS: AtomicU64 = AtomicU64::new(0);

/// The pressed bits of a group (bit set: pressed).
fn pressed(group: u8) -> u8 {
    let mut bits = 0u8;
    match group {
        0..=3 => {
            let stick = input(usize::from(group / 2));
            map().for_each_pressed(&stick, |n| match n {
                Native::Stick(bit) if group % 2 == 0 => bits |= 1 << bit,
                Native::Button(bit) if group % 2 == 1 => bits |= 1 << bit,
                _ => {}
            });
        }
        4 => {
            let mut coin = LAST_COIN.lock().unwrap();
            for player in 0..2 {
                map().for_each_pressed(&input(player), |n| match n {
                    Native::Coin => *coin = Some(Instant::now()),
                    Native::Operator(bit) if player == 0 => bits |= 1 << bit,
                    _ => {}
                });
            }
            if coin.is_some_and(|t| t.elapsed() < COIN_HOLD) {
                bits |= 1 << 2;
            }
        }
        _ => {}
    }
    bits
}

/// The group's nibble on the status port (active low); group 8 the I/O check.
fn status(group: u8, pressed: u8) -> u32 {
    if group == 8 { IO_OK } else { u32::from(!pressed & 0x0F) }
}

extern "C" fn read(group: u32) -> u32 {
    let group = group as u8;
    if READS.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("comeonbaby: first board read (group {group})");
    }
    status(group, pressed(group))
}

static SERVICE_HELD: AtomicBool = AtomicBool::new(true);

/// The operator menu's select: 0xff when the service button goes down.
extern "C" fn menu_select() -> u32 {
    let held = pressed(4) & 1 << 1 != 0;
    let was = SERVICE_HELD.swap(held, Ordering::Relaxed);
    if held && !was { 0xFF } else { 0 }
}

static EXE: AtomicUsize = AtomicUsize::new(0);
static BUILD: OnceLock<&'static Build> = OnceLock::new();
static FREE_PLAY: Mutex<Option<bool>> = Mutex::new(None);

/// The coins per credit for the free play option (`None`: the file's).
fn coins(free_play: Option<bool>) -> Option<i32> {
    free_play.map(|on| if on { 0 } else { 1 })
}

/// The boot's settings reader, then the free play option over the file's coins.
extern "C" fn read_settings() {
    let exe = EXE.load(Ordering::Relaxed);
    let Some(build) = BUILD.get() else { return };
    let reader: extern "C" fn() = unsafe { std::mem::transmute(exe + build.settings_jump.2) };
    reader();
    if let Some(coins) = coins(*FREE_PLAY.lock().unwrap()) {
        unsafe {
            *((exe + build.start_coins) as *mut i32) = coins;
            *((exe + build.continue_coins) as *mut i32) = coins;
        }
        log!("comeonbaby: {coins} coin(s) per credit");
    }
}

type P = *mut std::ffi::c_void;
type WinOpen = unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, i32) -> i32;

static GR_SST_WIN_OPEN: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// The payload's Glide window.
static WINDOW: AtomicUsize = AtomicUsize::new(0);

/// The Glide window's place: the largest centered 4:3 rectangle of the screen.
fn glide_rect() -> (i32, i32, i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    picture_rect(sw, sh)
}

/// The centered 4:3 rectangle of a `sw` x `sh` screen.
fn picture_rect(sw: i32, sh: i32) -> (i32, i32, i32, i32) {
    let (w, h) = if sw * 3 > sh * 4 { (sh * 4 / 3, sh) } else { (sw, sw * 3 / 4) };
    ((sw - w) / 2, (sh - h) / 2, w, h)
}

/// The payload's window, created on a thread of its own running its messages.
fn glide_window() -> usize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage, WNDCLASSW, WS_EX_APPWINDOW,
        WS_POPUP, WS_VISIBLE,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let class = windows_sys::w!("WalGlide");
        let (x, y, cx, cy) = glide_rect();
        let w = unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: GetModuleHandleW(std::ptr::null()), lpszClassName: class, ..std::mem::zeroed() };
            RegisterClassW(&wc);
            CreateWindowExW(WS_EX_APPWINDOW, class, windows_sys::w!("Come on Baby!"), WS_POPUP | WS_VISIBLE, x, y, cx, cy, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
        } as usize;
        log!("comeonbaby: Glide window {w:#x} at {x},{y} {cx}x{cy}");
        let _ = tx.send(w);
        if w == 0 {
            return;
        }
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    rx.recv().unwrap_or(0)
}

/// The Glide DLL's sizing of the Glide window: its rectangle kept.
unsafe extern "system" fn set_window_pos(hwnd: P, after: P, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    let f: unsafe extern "system" fn(P, P, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    let (x, y, cx, cy) = if hwnd as usize == WINDOW.load(Ordering::Relaxed) { glide_rect() } else { (x, y, cx, cy) };
    unsafe { f(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn move_window(hwnd: P, x: i32, y: i32, cx: i32, cy: i32, repaint: i32) -> i32 {
    let f: unsafe extern "system" fn(P, i32, i32, i32, i32, i32) -> i32 = unsafe { std::mem::transmute(MOVE_WINDOW.load(Ordering::Relaxed)) };
    let (x, y, cx, cy) = if hwnd as usize == WINDOW.load(Ordering::Relaxed) { glide_rect() } else { (x, y, cx, cy) };
    unsafe { f(hwnd, x, y, cx, cy, repaint) }
}

/// The game's `grSstWinOpen` (no window): the payload's window, the game's hidden.
unsafe extern "system" fn sst_win_open(hwnd: u32, res: i32, refresh: i32, color: i32, origin: i32, colors: i32, aux: i32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowA, SW_HIDE, ShowWindow};
    let f: WinOpen = unsafe { std::mem::transmute(GR_SST_WIN_OPEN.load(Ordering::Relaxed)) };
    let w = glide_window();
    if w == 0 {
        return unsafe { f(hwnd, res, refresh, color, origin, colors, aux) };
    }
    WINDOW.store(w, Ordering::Relaxed);
    let glide = unsafe { GetModuleHandleW(windows_sys::w!("glide2x.dll")) } as usize;
    if glide != 0 {
        unsafe {
            if let Some(o) = iat::hook_module(glide, "USER32.dll", "SetWindowPos", set_window_pos as *const () as usize) {
                SET_WINDOW_POS.store(o, Ordering::Relaxed);
            }
            if let Some(o) = iat::hook_module(glide, "USER32.dll", "MoveWindow", move_window as *const () as usize) {
                MOVE_WINDOW.store(o, Ordering::Relaxed);
            }
        }
    }
    let game = unsafe { FindWindowA(c"WinGlideClass".as_ptr().cast(), std::ptr::null()) };
    if !game.is_null() {
        unsafe { ShowWindow(game, SW_HIDE) };
    }
    unsafe { f(w as u32, res, refresh, color, origin, colors, aux) }
}

/// Writes a `jmp` (0xE9) or `call` (0xE8) to `to` at `at`.
fn branch(at: usize, opcode: u8, to: usize) {
    let mut code = [opcode, 0, 0, 0, 0];
    code[1..].copy_from_slice(&to.wrapping_sub(at + 5).to_le_bytes());
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const _, code.len(), old, &mut old);
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let code = |rva: usize, len: usize| unsafe { std::slice::from_raw_parts((exe + rva) as *const u8, len) };
    // the original reader, or the dump's `cob_patch.dll` jump (the real keyboard)
    let Some(build) = BUILDS.iter().find(|b| {
        let c = code(b.reader, 3);
        (c == READER_CODE || c[0] == 0xE9) && code(b.settings_jump.0, 5) == b.settings_jump.1
    }) else {
        log!("comeonbaby: unknown build, no board");
        return;
    };
    let _ = BUILD.set(build);
    let at = exe + build.reader;
    branch(at, 0xE9, read as *const () as usize);
    if let Some((select, bytes)) = build.menu_select
        && code(select, 5) == bytes
    {
        branch(exe + select, 0xE8, menu_select as *const () as usize);
    }
    EXE.store(exe, Ordering::Relaxed);
    *FREE_PLAY.lock().unwrap() = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY");
    branch(exe + build.settings_jump.0, 0xE9, read_settings as *const () as usize);
    match unsafe { iat::hook("glide2x.dll", "_grSstWinOpen@28", sst_win_open as *const () as usize) } {
        Some(o) => GR_SST_WIN_OPEN.store(o, Ordering::Relaxed),
        None => log!("comeonbaby: no grSstWinOpen import, the Glide DLL picks the window"),
    }
    log!("comeonbaby: {}, parallel port board at {at:#x}", build.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibbles() {
        assert_eq!(status(0, 0), 0x0F);
        assert_eq!(status(1, 0b101), 0x0A);
        assert_eq!(status(8, 0), IO_OK);
    }

    #[test]
    fn picture() {
        assert_eq!(picture_rect(1920, 1080), (240, 0, 1440, 1080));
        assert_eq!(picture_rect(1600, 1200), (0, 0, 1600, 1200));
        assert_eq!(picture_rect(1280, 1024), (0, 32, 1280, 960));
    }

    #[test]
    fn builds() {
        // each build's settings jump is a `jmp` to its settings reader
        for b in BUILDS {
            let (at, code, to) = b.settings_jump;
            let rel = i32::from_le_bytes(code[1..].try_into().unwrap());
            assert_eq!((code[0], (at as i64 + 5 + i64::from(rel)) as usize), (0xE9, to), "{}", b.name);
        }
    }

    #[test]
    fn free_play() {
        assert_eq!((coins(Some(true)), coins(Some(false)), coins(None)), (Some(0), Some(1), None));
    }
}
