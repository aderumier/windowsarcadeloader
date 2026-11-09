//! DOS arcade games in a Glide DOSBox build (`dosbox.exe`, DOSBox 0.74 with the Glide
//! pass-through patch, SDL 1.2, 32-bit): `WAL_ARCADEPC_BOARD=dosbox`, x86 build. Xtom 3D.
//!
//! - **Picture.** The guest's Glide calls go to the host's `glide2x.dll`. DOSBox hands
//!   `grSstWinOpen` its own window, where it keeps drawing its VGA screen (the DOS prompt):
//!   both pictures alternated each frame (Wine composes a Vulkan / Direct3D picture with the
//!   window's GDI surface). Its `grSstWinOpen` (found with `GetProcAddress`) gets a window of the
//!   payload instead, a borderless one with a thread and message loop of its own (made on
//!   DOSBox's thread, gxvk's messages to the window waited for DOSBox's thread, itself waiting
//!   for gxvk), DOSBox's window hidden. The window is the largest centered 4:3 rectangle of the
//!   screen (the whole screen with `WAL_KEEP_ASPECT_RATIO=0`; the profile's black bezel
//!   around): the Glide DLL's sizing of it (its `SetWindowPos` / `MoveWindow`) keeps that
//!   rectangle. DOSBox's own presents (`SDL_Flip`, `SDL_UpdateRects`, `SDL_GL_SwapBuffers`,
//!   `SDL_DisplayYUVOverlay`) do nothing while `glide2x.dll` is loaded.
//! - **Focus.** With `cycles=max`, DOSBox raises its emulated CPU speed from a low start and
//!   stops adjusting it when its window loses the focus (`CPU_Enable_SkipAutoAdjust`): the Glide
//!   window took the focus at the boot, the game ran several times too slow (minutes of
//!   texture loading). SDL's focus losses (`SDL_ACTIVEEVENT`, `gain` 0) are dropped.
//! - **Input.** DOSBox reads the keyboard from `SDL_PollEvent`: the real keyboard's, mouse's
//!   and joysticks' events are dropped, the virtual sticks' keys are queued as `SDL_KEYDOWN` /
//!   `SDL_KEYUP` events with the key symbol and the scan code (the dump's `usescancodes` maps
//!   the scan code).
//!
//! Natives: `key_<name>` (SDL key names: letters, digits, `f1`-`f12`, `up`, `lctrl`,
//! `leftbracket`...). Player 1's map takes `WAL_MAP`, player 2's are its defaults.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, MSG, RegisterClassW, SM_CXSCREEN,
    SM_CYSCREEN, SW_HIDE, ShowWindow, TranslateMessage, WNDCLASSW, WS_EX_APPWINDOW, WS_POPUP, WS_VISIBLE,
};

type P = *mut c_void;

/// SDL 1.2 event types.
const SDL_ACTIVEEVENT: u8 = 1;
const SDL_KEYDOWN: u8 = 2;
const SDL_KEYUP: u8 = 3;
const SDL_MOUSEMOTION: u8 = 4;
const SDL_JOYBUTTONUP: u8 = 11;
const SDL_PRESSED: u8 = 1;
const SDL_RELEASED: u8 = 0;
/// `sizeof(SDL_Event)` on win32.
const EVENT_SIZE: usize = 20;

/// SDL key symbols (SDLKey) by name, and their scan codes (DirectInput's, as SDL's `directx`
/// driver reports them: set 1, 0x80 on the extended keys).
const KEY_NAMES: &[(&str, u32, u8)] = &[
    ("backspace", 8, 0x0E),
    ("tab", 9, 0x0F),
    ("enter", 13, 0x1C),
    ("escape", 27, 0x01),
    ("space", 32, 0x39),
    ("minus", 45, 0x0C),
    ("equals", 61, 0x0D),
    ("leftbracket", 91, 0x1A),
    ("rightbracket", 93, 0x1B),
    ("up", 273, 0xC8),
    ("down", 274, 0xD0),
    ("right", 275, 0xCD),
    ("left", 276, 0xCB),
    ("insert", 277, 0xD2),
    ("home", 278, 0xC7),
    ("end", 279, 0xCF),
    ("pageup", 280, 0xC9),
    ("pagedown", 281, 0xD1),
    ("rshift", 303, 0x36),
    ("lshift", 304, 0x2A),
    ("rctrl", 305, 0x9D),
    ("lctrl", 306, 0x1D),
    ("ralt", 307, 0xB8),
    ("lalt", 308, 0x38),
    ("delete", 127, 0xD3),
];

/// Scan codes of the digits and letters (set 1).
const DIGITS: &[u8] = b"1234567890";
const LETTERS: &[(&[u8], u8)] = &[(b"qwertyuiop", 0x10), (b"asdfghjkl", 0x1E), (b"zxcvbnm", 0x2C)];

/// A native: the key symbol, its scan code in the bits 16-23.
fn key(sym: u32, scan: u8) -> u32 {
    sym | u32::from(scan) << 16
}

/// Xtom 3D's keyboard (MAIN.EXE's table at 0x14b410, each key beside its I/O card input):
/// player 1 the arrows, `[` fire, `]` bomb; player 2 WASD, left shift fire, left control bomb.
/// The fire key also starts (the start input's keyboard alternative). Enter is the coin (player
/// 1's input bit 7, the game's coin handler counts a coin on its first frame).
const DEFAULTS_P1: &[(&str, &str)] = &[
    ("up", "key_up"),
    ("down", "key_down"),
    ("left", "key_left"),
    ("right", "key_right"),
    ("b1", "key_leftbracket"),
    ("b2", "key_rightbracket"),
    ("start", "key_leftbracket"),
    ("coin", "key_enter"),
];
const DEFAULTS_P2: &[(&str, &str)] = &[
    ("up", "key_w"),
    ("down", "key_s"),
    ("left", "key_a"),
    ("right", "key_d"),
    ("b1", "key_lshift"),
    ("b2", "key_lctrl"),
    ("start", "key_lshift"),
    ("coin", "key_enter"),
];

fn natives() -> &'static [(&'static str, u32)] {
    static NATIVES: OnceLock<Vec<(&'static str, u32)>> = OnceLock::new();
    NATIVES.get_or_init(|| {
        let mut keys: Vec<(String, u32)> = KEY_NAMES.iter().map(|&(n, k, sc)| (n.to_string(), key(k, sc))).collect();
        keys.extend(DIGITS.iter().enumerate().map(|(i, &c)| ((c as char).to_string(), key(u32::from(c), 0x02 + i as u8))));
        for &(row, first) in LETTERS {
            keys.extend(row.iter().enumerate().map(|(i, &c)| ((c as char).to_string(), key(u32::from(c), first + i as u8))));
        }
        // F1 282 .. F12 293: scan codes 0x3B..0x44, 0x57, 0x58
        keys.extend((1..=12u32).map(|f| (format!("f{f}"), key(281 + f, if f <= 10 { 0x3A + f as u8 } else { 0x4C + f as u8 }))));
        keys.into_iter().map(|(n, k)| (&*Box::leak(format!("key_{n}").into_boxed_str()), k)).collect()
    })
}

fn maps() -> &'static [ButtonMap<u32>; 2] {
    static MAPS: OnceLock<[ButtonMap<u32>; 2]> = OnceLock::new();
    MAPS.get_or_init(|| [ButtonMap::new(natives(), DEFAULTS_P1), ButtonMap::with_defaults(natives(), DEFAULTS_P2)])
}

struct Keys {
    /// The keys down in DOSBox (the payload's events).
    down: Vec<u32>,
    /// Events not handed to DOSBox yet: key, pressed.
    queue: VecDeque<(u32, bool)>,
}

static KEYS: Mutex<Keys> = Mutex::new(Keys { down: Vec::new(), queue: VecDeque::new() });

/// Queues the presses and releases since the last poll.
fn update(keys: &mut Keys) {
    let mut now = Vec::new();
    for (player, map) in maps().iter().enumerate() {
        map.for_each_pressed(&input(player), |k| {
            if !now.contains(&k) {
                now.push(k);
            }
        });
    }
    for &k in keys.down.iter().filter(|k| !now.contains(k)) {
        keys.queue.push_back((k, false));
    }
    for &k in now.iter().filter(|k| !keys.down.contains(k)) {
        keys.queue.push_back((k, true));
    }
    keys.down = now;
}

static POLL_EVENT: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// SDLCALL is cdecl on win32.
unsafe extern "C" fn poll_event(event: *mut u8) -> i32 {
    let orig: unsafe extern "C" fn(*mut u8) -> i32 = unsafe { std::mem::transmute(POLL_EVENT.load(Ordering::Relaxed)) };
    let mut keys = KEYS.lock().unwrap();
    if keys.queue.is_empty() {
        update(&mut keys);
    }
    if event.is_null() {
        return i32::from(!keys.queue.is_empty()) | unsafe { orig(event) };
    }
    if let Some((sym, pressed)) = keys.queue.pop_front() {
        // SDL_KeyboardEvent: type, which, state, keysym @4 (4-byte aligned) { scancode,
        // sym @8, mod @12, unicode @16 }
        let mut e = [0u8; EVENT_SIZE];
        e[0] = if pressed { SDL_KEYDOWN } else { SDL_KEYUP };
        e[2] = if pressed { SDL_PRESSED } else { SDL_RELEASED };
        e[4] = (sym >> 16) as u8;
        e[8..12].copy_from_slice(&(sym & 0xFFFF).to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(e.as_ptr(), event, EVENT_SIZE) };
        return 1;
    }
    drop(keys);
    loop {
        if unsafe { orig(event) } == 0 {
            return 0;
        }
        // the real keyboard, mouse and joysticks; focus losses (SDL_ActiveEvent: type, gain)
        let kind = unsafe { *event };
        if kind == SDL_ACTIVEEVENT && unsafe { *event.add(1) } == 0 {
            continue;
        }
        if !(SDL_KEYDOWN..=SDL_JOYBUTTONUP).contains(&kind) {
            return 1;
        }
        if DROPPED.fetch_add(1, Ordering::Relaxed) == 0 {
            log!("dosbox: real input events dropped (first: type {kind}{})", if kind >= SDL_MOUSEMOTION { "" } else { ", a key" });
        }
    }
}

/// DOSBox's own picture hidden while the guest's Glide draws into the window.
fn glide_active() -> bool {
    unsafe { !GetModuleHandleW(windows_sys::w!("glide2x.dll")).is_null() }
}

static FLIP: AtomicUsize = AtomicUsize::new(0);
static UPDATE_RECTS: AtomicUsize = AtomicUsize::new(0);
static GL_SWAP_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static DISPLAY_YUV_OVERLAY: AtomicUsize = AtomicUsize::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);

fn skip() -> bool {
    if !glide_active() {
        return false;
    }
    if SKIPPED.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("dosbox: Glide active, DOSBox's own picture no longer presented");
    }
    true
}

unsafe extern "C" fn flip(screen: P) -> i32 {
    if skip() {
        return 0;
    }
    let f: unsafe extern "C" fn(P) -> i32 = unsafe { std::mem::transmute(FLIP.load(Ordering::Relaxed)) };
    unsafe { f(screen) }
}

unsafe extern "C" fn update_rects(screen: P, count: i32, rects: P) {
    if skip() {
        return;
    }
    let f: unsafe extern "C" fn(P, i32, P) = unsafe { std::mem::transmute(UPDATE_RECTS.load(Ordering::Relaxed)) };
    unsafe { f(screen, count, rects) }
}

unsafe extern "C" fn gl_swap_buffers() {
    if skip() {
        return;
    }
    let f: unsafe extern "C" fn() = unsafe { std::mem::transmute(GL_SWAP_BUFFERS.load(Ordering::Relaxed)) };
    unsafe { f() }
}

unsafe extern "C" fn display_yuv_overlay(overlay: P, rect: P) -> i32 {
    if skip() {
        return 0;
    }
    let f: unsafe extern "C" fn(P, P) -> i32 = unsafe { std::mem::transmute(DISPLAY_YUV_OVERLAY.load(Ordering::Relaxed)) };
    unsafe { f(overlay, rect) }
}

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static GR_SST_WIN_OPEN: AtomicUsize = AtomicUsize::new(0);
/// The payload's Glide window.
static WINDOW: AtomicUsize = AtomicUsize::new(0);

type WinOpen = unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, i32) -> i32;

/// The Glide window's place: x, y, width, height.
fn glide_rect() -> (i32, i32, i32, i32) {
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if std::env::var("WAL_KEEP_ASPECT_RATIO").is_ok_and(|v| v.trim() == "0") {
        return (0, 0, sw, sh);
    }
    let (w, h) = if sw * 3 > sh * 4 { (sh * 4 / 3, sh) } else { (sw, sw * 3 / 4) };
    ((sw - w) / 2, (sh - h) / 2, w, h)
}

/// The payload's window, created once, on a thread of its own running its messages.
fn glide_window() -> usize {
    let w = WINDOW.load(Ordering::Relaxed);
    if w != 0 {
        return w;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let class = windows_sys::w!("WalGlide");
        let (x, y, cx, cy) = glide_rect();
        let w = unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: GetModuleHandleW(std::ptr::null()),
                lpszClassName: class,
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            CreateWindowExW(WS_EX_APPWINDOW, class, windows_sys::w!("Glide"), WS_POPUP | WS_VISIBLE, x, y, cx, cy, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
        } as usize;
        log!("dosbox: Glide window {w:#x} at {x},{y} {cx}x{cy}");
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
    let w = rx.recv().unwrap_or(0);
    WINDOW.store(w, Ordering::Relaxed);
    w
}

static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);

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

unsafe extern "system" fn sst_win_open(hwnd: u32, res: i32, refresh: i32, color: i32, origin: i32, colors: i32, aux: i32) -> i32 {
    let f: WinOpen = unsafe { std::mem::transmute(GR_SST_WIN_OPEN.load(Ordering::Relaxed)) };
    let w = glide_window();
    if w == 0 {
        return unsafe { f(hwnd, res, refresh, color, origin, colors, aux) };
    }
    let glide = unsafe { GetModuleHandleW(windows_sys::w!("glide2x.dll")) } as usize;
    if glide != 0 && SET_WINDOW_POS.load(Ordering::Relaxed) == 0 {
        unsafe {
            if let Some(o) = iat::hook_module(glide, "USER32.dll", "SetWindowPos", set_window_pos as *const () as usize) {
                SET_WINDOW_POS.store(o, Ordering::Relaxed);
            }
            if let Some(o) = iat::hook_module(glide, "USER32.dll", "MoveWindow", move_window as *const () as usize) {
                MOVE_WINDOW.store(o, Ordering::Relaxed);
            }
        }
    }
    unsafe { ShowWindow(hwnd as usize as P, SW_HIDE) };
    unsafe { f(w as u32, res, refresh, color, origin, colors, aux) }
}

unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    let f: unsafe extern "system" fn(P, *const u8) -> usize = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let addr = unsafe { f(module, name) };
    // a name, not an ordinal
    if addr != 0 && name as usize > 0xFFFF {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
        if n.windows(12).any(|w| w == b"grSstWinOpen") {
            GR_SST_WIN_OPEN.store(addr, Ordering::Relaxed);
            return sst_win_open as *const () as usize;
        }
    }
    addr
}

pub fn init() {
    match unsafe { iat::hook("KERNEL32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("dosbox: the executable does not import GetProcAddress, Glide in DOSBox's window"),
    }
    let hooks: [(&str, usize, &AtomicUsize); 5] = [
        ("SDL_PollEvent", poll_event as *const () as usize, &POLL_EVENT),
        ("SDL_Flip", flip as *const () as usize, &FLIP),
        ("SDL_UpdateRects", update_rects as *const () as usize, &UPDATE_RECTS),
        ("SDL_GL_SwapBuffers", gl_swap_buffers as *const () as usize, &GL_SWAP_BUFFERS),
        ("SDL_DisplayYUVOverlay", display_yuv_overlay as *const () as usize, &DISPLAY_YUV_OVERLAY),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook("SDL.dll", name, hook) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("dosbox: the executable does not import SDL.dll!{name}"),
        }
    }
    log!("dosbox: SDL hooked (the virtual sticks' keys, DOSBox's picture hidden under Glide)");
}
