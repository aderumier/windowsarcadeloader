//! Mad Bullets (Ironbird, 2016, the Steam release: `bin_x86\game.exe`, OpenGL, 32-bit; its
//! `steam_api.dll` a Steam emulator): `WAL_ARCADEPC_BOARD=madbullets`.
//!
//! A one-player shooting game played with the mouse: the game's own window messages (its
//! `PeekMessageA` loop: `WM_MOUSEMOVE`, the left button) and `GetKeyState`, no raw input, no pads.
//! The payload plays that mouse with the virtual sticks: the gun is the mouse (its place on the
//! game window's picture), the trigger its left button, the keys posted (`payload-common`'s
//! `input_filter`: the real keyboard and mouse never reach the game, moves included).
//!
//! The game's pointer is the system cursor (its `cursor.cur`, the window class's): the payload
//! draws the gun's crosshair over its picture instead (the common crosshair option,
//! `WAL_CROSSHAIR`): the executable's `SwapBuffers` hooked, a red cross outlined in black drawn
//! with OpenGL's fixed pipeline (the game's context is a legacy one: `wglCreateContext`), the
//! game's state saved and put back; the game's cursors (`LoadCursorFromFileA`, `LoadCursorA`)
//! then an invisible one.
//!
//! The game reloads only by a touch of its bullet belt (the HUD's reload buttons:
//! `onHudReload`, no key): the gun's reload, or a shot off the screen, clicks the belt (the
//! pointer there, the left button pressed and released, the pointer back on the gun).
//!
//! Natives: `trigger` (player 1), `reload`, `offscreen` (the gun off the screen: the trigger
//! reloads), `back` (Escape: the pause menu, back in the menus).

use std::sync::OnceLock;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::WindowFromDC;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateCursor, GetClientRect, HCURSOR, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Offscreen,
    /// A key: its virtual key.
    Key(u16),
}

const NATIVES: &[(&str, Native)] =
    &[("trigger", Native::Trigger), ("reload", Native::Reload), ("offscreen", Native::Offscreen), ("back", Native::Key(0x1B))];

/// The gun's right button reloads, start is the pause menu.
const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("b2", "reload"), ("b6", "offscreen"), ("start", "back")];

/// The bullet belt (the HUD's reload button), on the picture: its middle, bottom left.
const BELT: (f32, f32) = (0.15, 0.88);

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
    let mut reload_was = false;
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut keys = [0u64; 4];
        let (mut trigger, mut reload, mut offscreen) = (false, false, false);
        map().for_each_pressed(&input(0), |n| match n {
            Native::Trigger => trigger = true,
            Native::Reload => reload = true,
            Native::Offscreen => offscreen = true,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        // off the screen the trigger reloads
        if offscreen && trigger {
            (trigger, reload) = (false, true);
        }
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
        if reload && !reload_was && !trigger {
            click_belt(hwnd, &r, last);
        }
        reload_was = reload;
    }
}

/// A reload: the bullet belt touched (a frame between each step), the pointer back at `back`.
fn click_belt(hwnd: windows_sys::Win32::Foundation::HWND, r: &RECT, back: (i32, i32)) {
    let belt = ((BELT.0 * r.right as f32) as i32, (BELT.1 * r.bottom as f32) as i32);
    let step = Duration::from_millis(35);
    log!("madbullets: reload (the belt at {}, {})", belt.0, belt.1);
    post_mouse(hwnd, WM_MOUSEMOVE, belt.0, belt.1, 0);
    std::thread::sleep(step);
    post_mouse(hwnd, WM_LBUTTONDOWN, belt.0, belt.1, MK_LBUTTON);
    std::thread::sleep(step);
    post_mouse(hwnd, WM_LBUTTONUP, belt.0, belt.1, 0);
    std::thread::sleep(step);
    if back.0 != i32::MIN {
        post_mouse(hwnd, WM_MOUSEMOVE, back.0, back.1, 0);
    }
}

/// An invisible cursor (the payload's crosshair drawn instead).
fn no_cursor() -> HCURSOR {
    static CURSOR: AtomicUsize = AtomicUsize::new(0);
    if CURSOR.load(Ordering::Relaxed) == 0 {
        // 32x32: the AND mask keeps the screen, the XOR mask changes nothing
        let (and, xor) = ([0xFFu8; 128], [0u8; 128]);
        let cursor = unsafe { CreateCursor(GetModuleHandleW(std::ptr::null()), 0, 0, 32, 32, and.as_ptr().cast(), xor.as_ptr().cast()) };
        CURSOR.store(cursor as usize, Ordering::Relaxed);
    }
    CURSOR.load(Ordering::Relaxed) as HCURSOR
}

unsafe extern "system" fn load_cursor_from_file(_name: *const u8) -> HCURSOR {
    no_cursor()
}

unsafe extern "system" fn load_cursor(_instance: *mut c_void, _name: *const u8) -> HCURSOR {
    no_cursor()
}

// The crosshair: OpenGL's functions (opengl32's exports, then the context's)

static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

/// OpenGL 1.1's functions used, by name.
const GL_NAMES: [&[u8]; 16] = [
    b"glGetIntegerv\0",
    b"glPushAttrib\0",
    b"glPopAttrib\0",
    b"glDisable\0",
    b"glViewport\0",
    b"glMatrixMode\0",
    b"glPushMatrix\0",
    b"glPopMatrix\0",
    b"glLoadIdentity\0",
    b"glOrtho\0",
    b"glLineWidth\0",
    b"glColor3f\0",
    b"glBegin\0",
    b"glVertex2f\0",
    b"glEnd\0",
    b"glColorMask\0",
];
static GL: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
/// glUseProgram, glBindFramebuffer (the context's: `wglGetProcAddress`).
static GL_EXT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

fn gl_functions() -> bool {
    if GL[0].load(Ordering::Relaxed) != 0 {
        return true;
    }
    let module = unsafe { GetModuleHandleW(windows_sys::w!("opengl32.dll")) };
    if module.is_null() {
        return false;
    }
    for (slot, name) in GL.iter().zip(GL_NAMES) {
        match unsafe { GetProcAddress(module, name.as_ptr()) } {
            Some(f) => slot.store(f as usize, Ordering::Relaxed),
            None => return false,
        }
    }
    if let Some(get) = unsafe { GetProcAddress(module, b"wglGetProcAddress\0".as_ptr()) } {
        let get: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(get) };
        for (slot, name) in GL_EXT.iter().zip([b"glUseProgram\0".as_slice(), b"glBindFramebuffer\0".as_slice()]) {
            slot.store(unsafe { get(name.as_ptr()) }, Ordering::Relaxed);
        }
    }
    true
}

macro_rules! gl {
    ($i:expr, fn($($t:ty),*) $(-> $r:ty)? $(, $a:expr)*) => {{
        let f: unsafe extern "system" fn($($t),*) $(-> $r)? = unsafe { std::mem::transmute(GL[$i].load(Ordering::Relaxed)) };
        unsafe { f($($a),*) }
    }};
}

/// The gun's crosshair drawn in the window's back buffer (fixed pipeline, the game's state put
/// back).
fn draw_crosshair(hwnd: windows_sys::Win32::Foundation::HWND) {
    let Some((x, y)) = gun::gun_of_player_in_picture(0) else { return };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetClientRect(hwnd, &mut r) } == 0 || r.right <= 0 || r.bottom <= 0 || !gl_functions() {
        return;
    }
    let (w, h) = (r.right as f32, r.bottom as f32);
    let (cx, cy) = (x * w, y * h);
    let (mut program, mut framebuffer) = (0i32, 0i32);
    gl!(0, fn(u32, *mut i32), 0x8B8D, &mut program); // GL_CURRENT_PROGRAM
    gl!(0, fn(u32, *mut i32), 0x8CA6, &mut framebuffer); // GL_DRAW_FRAMEBUFFER_BINDING
    let (use_program, bind_framebuffer) = (GL_EXT[0].load(Ordering::Relaxed), GL_EXT[1].load(Ordering::Relaxed));
    if use_program != 0 {
        let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(use_program) };
        unsafe { f(0) };
    }
    if bind_framebuffer != 0 {
        let f: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(bind_framebuffer) };
        unsafe { f(0x8CA9, 0) }; // GL_DRAW_FRAMEBUFFER
    }
    gl!(1, fn(u32), 0x000F_FFFF); // GL_ALL_ATTRIB_BITS
    // texture 2D, depth test, cull face, blend, stencil test, scissor test, lighting, alpha test
    for cap in [0x0DE1u32, 0x0B71, 0x0B44, 0x0BE2, 0x0B90, 0x0C11, 0x0B50, 0x0BC0] {
        gl!(3, fn(u32), cap);
    }
    gl!(15, fn(u8, u8, u8, u8), 1, 1, 1, 1);
    gl!(4, fn(i32, i32, i32, i32), 0, 0, r.right, r.bottom);
    gl!(5, fn(u32), 0x1701); // GL_PROJECTION
    gl!(6, fn());
    gl!(8, fn());
    gl!(9, fn(f64, f64, f64, f64, f64, f64), 0.0, w as f64, h as f64, 0.0, -1.0, 1.0);
    gl!(5, fn(u32), 0x1700); // GL_MODELVIEW
    gl!(6, fn());
    gl!(8, fn());
    let (arm, gap) = (h * 0.03, h * 0.008);
    for (width, color) in [(h * 0.006 + 2.0, [0.0f32, 0.0, 0.0]), (h * 0.003, [1.0, 0.15, 0.15])] {
        gl!(10, fn(f32), width.max(1.0));
        gl!(11, fn(f32, f32, f32), color[0], color[1], color[2]);
        gl!(12, fn(u32), 0x0001); // GL_LINES
        for (dx, dy) in [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            gl!(13, fn(f32, f32), cx + dx * gap, cy + dy * gap);
            gl!(13, fn(f32, f32), cx + dx * (gap + arm), cy + dy * (gap + arm));
        }
        gl!(14, fn());
    }
    gl!(7, fn());
    gl!(5, fn(u32), 0x1701);
    gl!(7, fn());
    gl!(5, fn(u32), 0x1700);
    gl!(2, fn());
    if bind_framebuffer != 0 {
        let f: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(bind_framebuffer) };
        unsafe { f(0x8CA9, framebuffer as u32) };
    }
    if use_program != 0 {
        let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(use_program) };
        unsafe { f(program as u32) };
    }
}

unsafe extern "system" fn swap_buffers(dc: *mut c_void) -> i32 {
    let hwnd = unsafe { WindowFromDC(dc) };
    if !hwnd.is_null() {
        draw_crosshair(hwnd);
    }
    let orig: unsafe extern "system" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(dc) }
}

pub fn init() {
    // the game's message loop: the executable's
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if wal_payload_common::crosshairs(true) {
        if let Some(orig) = unsafe { wal_payload_common::iat::hook_module(exe, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
            ORIG_SWAP.store(orig, Ordering::Relaxed);
        }
        // the game's cursor hidden: the crosshair is the payload's
        for (name, f) in [("LoadCursorFromFileA", load_cursor_from_file as *const () as usize), ("LoadCursorA", load_cursor as *const () as usize)] {
            unsafe { wal_payload_common::iat::hook_module(exe, "user32.dll", name, f) };
        }
    }
    wal_payload_common::input_filter::init(exe, key_held);
    own_mouse(exe);
    std::thread::spawn(input_thread);
    log!("madbullets: the mouse played by the gun");
}
