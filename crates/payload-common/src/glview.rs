//! An OpenGL game drawing a fixed size, fitted to its window: `WAL_GL_SIZE=<w>x<h>`.
//!
//! Aliens Extermination draws 1920x1080 (its viewports) whatever its window: in a smaller
//! screen-sized window (`WAL_WINDOW_SIZE=screen`, `WAL_NO_MODE_CHANGE=1`) the picture went off
//! it. The `glViewport` calls of the game and of the DLLs of its directory (GLUT's) are scaled
//! from `<w>x<h>` to the centered rectangle of `WAL_D3D9_ASPECT` in the window's client area,
//! black bars around (cleared before each `SwapBuffers`: the game's clears left them white), or
//! to the whole client area
//! with `WAL_KEEP_ASPECT_RATIO=0` ([`crate::screenshot::d3d9_aspect`]). A pointer spanning
//! the screen is mapped onto that picture with [`crate::screenshot::to_picture`].

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::WindowFromDC;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

type P = *mut c_void;

static WIDTH: AtomicI32 = AtomicI32::new(0);
static HEIGHT: AtomicI32 = AtomicI32::new(0);
static ORIG_VIEWPORT: AtomicUsize = AtomicUsize::new(0);
static GET_CURRENT_DC: AtomicUsize = AtomicUsize::new(0);
static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);
/// glEnable, glDisable, glIsEnabled, glScissor, glGetIntegerv, glGetFloatv, glClearColor, glClear.
static GL: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
const GL_NAMES: [&std::ffi::CStr; 8] =
    [c"glEnable", c"glDisable", c"glIsEnabled", c"glScissor", c"glGetIntegerv", c"glGetFloatv", c"glClearColor", c"glClear"];

pub fn init() {
    let Some((w, h)) = std::env::var("WAL_GL_SIZE").ok().and_then(|v| {
        let (w, h) = v.trim().split_once('x')?;
        Some((w.trim().parse::<i32>().ok()?, h.trim().parse::<i32>().ok()?))
    }) else {
        return;
    };
    if w <= 0 || h <= 0 {
        return;
    }
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store(h, Ordering::Relaxed);
    let mut hooked = 0;
    for base in crate::workarea::game_modules(iat::game_module()) {
        if let Some(o) = unsafe { iat::hook_module(base, "opengl32.dll", "glViewport", viewport as *const () as usize) } {
            if o != viewport as *const () as usize {
                ORIG_VIEWPORT.store(o, Ordering::Relaxed);
            }
            hooked += 1;
        }
    }
    let gl = unsafe { GetModuleHandleW(windows_sys::w!("opengl32.dll")) };
    if let Some(f) = unsafe { GetProcAddress(gl, c"wglGetCurrentDC".as_ptr().cast()) } {
        GET_CURRENT_DC.store(f as usize, Ordering::Relaxed);
    }
    let mut all = true;
    for (slot, name) in GL.iter().zip(GL_NAMES) {
        match unsafe { GetProcAddress(gl, name.as_ptr().cast()) } {
            Some(f) => slot.store(f as usize, Ordering::Relaxed),
            None => all = false,
        }
    }
    if all {
        for base in crate::workarea::game_modules(iat::game_module()) {
            if let Some(o) = unsafe { iat::hook_module(base, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
                if o != swap_buffers as *const () as usize {
                    ORIG_SWAP.store(o, Ordering::Relaxed);
                }
            }
        }
    }
    log!("glview: {w}x{h} viewports fitted to the window ({hooked} module(s))");
}

/// The picture's rectangle in the current context's window (x, y from its top left, w, h) and
/// the window's client size.
fn picture() -> Option<(i32, i32, i32, i32, i32, i32)> {
    let get_dc = GET_CURRENT_DC.load(Ordering::Relaxed);
    if get_dc == 0 {
        return None;
    }
    let get_dc: unsafe extern "system" fn() -> P = unsafe { std::mem::transmute(get_dc) };
    let hwnd = unsafe { WindowFromDC(get_dc()) };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 {
        return None;
    }
    let (cw, ch) = (r.right - r.left, r.bottom - r.top);
    if cw <= 0 || ch <= 0 {
        return None;
    }
    let Some((aw, ah)) = crate::screenshot::d3d9_aspect() else { return Some((0, 0, cw, ch, cw, ch)) };
    let (aw, ah) = (aw as i64, ah as i64);
    let (w, h) = if cw as i64 * ah > ch as i64 * aw { ((ch as i64 * aw / ah) as i32, ch) } else { (cw, (cw as i64 * ah / aw) as i32) };
    Some(((cw - w) / 2, (ch - h) / 2, w, h, cw, ch))
}

unsafe extern "system" fn viewport(x: i32, y: i32, w: i32, h: i32) {
    let orig: unsafe extern "system" fn(i32, i32, i32, i32) = unsafe { std::mem::transmute(ORIG_VIEWPORT.load(Ordering::Relaxed)) };
    let Some((px, py, pw, ph, cw, ch)) = picture() else { return unsafe { orig(x, y, w, h) } };
    // the game's size, or the window's for a viewport over the whole window (GLUT's reshape)
    let (gw, gh) = if (x, y, w, h) == (0, 0, cw, ch) {
        (cw as i64, ch as i64)
    } else {
        (WIDTH.load(Ordering::Relaxed) as i64, HEIGHT.load(Ordering::Relaxed) as i64)
    };
    // GL's origin is the lower left corner: the bottom bar's height is the top one's
    let sx = |v: i32| (v as i64 * pw as i64 / gw) as i32;
    let sy = |v: i32| (v as i64 * ph as i64 / gh) as i32;
    let (nx, ny, nw, nh) = (px + sx(x), py + sy(y), sx(w), sy(h));
    // each viewport size logged once (the first few)
    static LOGGED: std::sync::Mutex<Vec<(i32, i32)>> = std::sync::Mutex::new(Vec::new());
    let mut logged = LOGGED.lock().unwrap();
    if logged.len() < 8 && !logged.contains(&(w, h)) {
        logged.push((w, h));
        log!("glview: viewport {w}x{h} at {x},{y} -> {nw}x{nh} at {nx},{ny}");
    }
    drop(logged);
    unsafe { orig(nx, ny, nw, nh) }
}

fn gl(i: usize) -> usize {
    GL[i].load(Ordering::Relaxed)
}

/// The bars around the picture cleared black, the scissor and clear color put back.
unsafe extern "system" fn swap_buffers(dc: P) -> i32 {
    let orig: unsafe extern "system" fn(P) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    if let Some((px, py, pw, ph, cw, ch)) = picture().filter(|p| (p.2, p.3) != (p.4, p.5)) {
        const GL_SCISSOR_TEST: u32 = 0x0C11;
        const GL_SCISSOR_BOX: u32 = 0x0C10;
        const GL_COLOR_CLEAR_VALUE: u32 = 0x0C22;
        const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
        unsafe {
            let enable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(0));
            let disable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(1));
            let is_enabled: unsafe extern "system" fn(u32) -> u8 = std::mem::transmute(gl(2));
            let scissor: unsafe extern "system" fn(i32, i32, i32, i32) = std::mem::transmute(gl(3));
            let get_integer: unsafe extern "system" fn(u32, *mut i32) = std::mem::transmute(gl(4));
            let get_float: unsafe extern "system" fn(u32, *mut f32) = std::mem::transmute(gl(5));
            let clear_color: unsafe extern "system" fn(f32, f32, f32, f32) = std::mem::transmute(gl(6));
            let clear: unsafe extern "system" fn(u32) = std::mem::transmute(gl(7));
            let was_enabled = is_enabled(GL_SCISSOR_TEST) != 0;
            let mut boxed = [0i32; 4];
            get_integer(GL_SCISSOR_BOX, boxed.as_mut_ptr());
            let mut color = [0f32; 4];
            get_float(GL_COLOR_CLEAR_VALUE, color.as_mut_ptr());
            enable(GL_SCISSOR_TEST);
            clear_color(0.0, 0.0, 0.0, 1.0);
            // GL rectangles (from the lower left corner): below, above, left, right
            let bottom = ch - py - ph;
            for (x, y, w, h) in [(0, 0, cw, bottom), (0, bottom + ph, cw, py), (0, bottom, px, ph), (px + pw, bottom, cw - px - pw, ph)] {
                if w > 0 && h > 0 {
                    scissor(x, y, w, h);
                    clear(GL_COLOR_BUFFER_BIT);
                }
            }
            clear_color(color[0], color[1], color[2], color[3]);
            scissor(boxed[0], boxed[1], boxed[2], boxed[3]);
            if !was_enabled {
                disable(GL_SCISSOR_TEST);
            }
        }
    }
    unsafe { orig(dc) }
}
