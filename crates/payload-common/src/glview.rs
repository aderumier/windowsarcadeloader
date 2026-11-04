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
//!
//! `WAL_GL_CORNER=<w>x<h>`: for a game drawing `<w>x<h>` in the lower left corner of a larger
//! window (its viewports, scissors and pixel reads at its own size: PGA TOUR Team Challenge's
//! Anark frontend, 800x600 in a screen-sized window). Before each `SwapBuffers` that corner is
//! copied (a framebuffer of the payload) and drawn back over the window's back buffer, scaled:
//! aspect kept (centered, black bars), or stretched with `WAL_KEEP_ASPECT_RATIO=0`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use crate::log;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::WindowFromDC;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

type P = *mut c_void;

/// `WAL_GL_CORNER`'s size (0: not set).
static CORNER: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];
static WIDTH: AtomicI32 = AtomicI32::new(0);
static HEIGHT: AtomicI32 = AtomicI32::new(0);
static ORIG_VIEWPORT: AtomicUsize = AtomicUsize::new(0);
static GET_CURRENT_DC: AtomicUsize = AtomicUsize::new(0);

/// `wglGetCurrentDC` (looked up on first use).
fn get_current_dc() -> usize {
    let f = GET_CURRENT_DC.load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let module = unsafe { GetModuleHandleW(windows_sys::w!("opengl32.dll")) };
    let f = unsafe { GetProcAddress(module, c"wglGetCurrentDC".as_ptr().cast()) }.map_or(0, |p| p as usize);
    GET_CURRENT_DC.store(f, Ordering::Relaxed);
    f
}
static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);
static ORIG_WGL_SWAP: AtomicUsize = AtomicUsize::new(0);

/// The game's `GetProcAddress`: `wglSwapBuffers` goes through [`wgl_swap_buffers`].
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    let orig: unsafe extern "system" fn(P, *const u8) -> usize = unsafe { std::mem::transmute(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    // by name (not an ordinal)
    if f != 0 && name as usize > 0xffff && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"wglSwapBuffers" {
        ORIG_WGL_SWAP.store(f, Ordering::Relaxed);
        return wgl_swap_buffers as *const () as usize;
    }
    f
}

unsafe extern "system" fn wgl_swap_buffers(dc: P) -> i32 {
    let orig: unsafe extern "system" fn(P) -> i32 = unsafe { std::mem::transmute(ORIG_WGL_SWAP.load(Ordering::Relaxed)) };
    unsafe { before_swap(dc) };
    unsafe { orig(dc) }
}
/// glEnable, glDisable, glIsEnabled, glScissor, glGetIntegerv, glGetFloatv, glClearColor, glClear.
static GL: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
const GL_NAMES: [&std::ffi::CStr; 8] =
    [c"glEnable", c"glDisable", c"glIsEnabled", c"glScissor", c"glGetIntegerv", c"glGetFloatv", c"glClearColor", c"glClear"];

fn size(var: &str) -> Option<(i32, i32)> {
    let v = std::env::var(var).ok()?;
    let (w, h) = v.trim().split_once('x')?;
    Some((w.trim().parse::<i32>().ok()?, h.trim().parse::<i32>().ok()?)).filter(|&(w, h)| w > 0 && h > 0)
}

pub fn init() {
    let corner = size("WAL_GL_CORNER");
    let viewports = size("WAL_GL_SIZE");
    if corner.is_none() && viewports.is_none() {
        return;
    }
    if let Some((w, h)) = corner {
        CORNER[0].store(w, Ordering::Relaxed);
        CORNER[1].store(h, Ordering::Relaxed);
    } else if let Some((w, h)) = viewports {
        WIDTH.store(w, Ordering::Relaxed);
        HEIGHT.store(h, Ordering::Relaxed);
        // the game's and its directory's DLLs' (GLUT's), those loaded later too
        crate::late_hooks::hook("opengl32.dll", "glViewport", viewport as *const () as usize, &ORIG_VIEWPORT);
    }
    // SiN's renderer (Quake II's ref_gl.dll) is loaded at the game's start: hooked then; it
    // takes its wglSwapBuffers from GetProcAddress (its GL functions table)
    crate::late_hooks::hook("gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize, &ORIG_SWAP);
    crate::late_hooks::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize, &ORIG_GET_PROC);
    match corner {
        Some((w, h)) => log!("glview: the {w}x{h} corner scaled to the window"),
        None => log!("glview: {}x{} viewports fitted to the window", WIDTH.load(Ordering::Relaxed), HEIGHT.load(Ordering::Relaxed)),
    }
}

/// The picture's rectangle in the current context's window (x, y from its top left, w, h) and
/// the window's client size.
fn picture() -> Option<(i32, i32, i32, i32, i32, i32)> {
    let get_dc = get_current_dc();
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

/// GL function `i` of [`GL_NAMES`] (opengl32.dll's, looked up on first use: the game may load
/// it after the payload's start).
fn gl(i: usize) -> usize {
    let f = GL[i].load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let module = unsafe { GetModuleHandleW(windows_sys::w!("opengl32.dll")) };
    let f = unsafe { GetProcAddress(module, GL_NAMES[i].as_ptr().cast()) }.map_or(0, |p| p as usize);
    GL[i].store(f, Ordering::Relaxed);
    f
}

/// All [`GL_NAMES`] functions found.
fn gl_ready() -> bool {
    (0..GL.len()).all(|i| gl(i) != 0)
}

/// The bars around the picture cleared black, the scissor and clear color put back.
unsafe extern "system" fn swap_buffers(dc: P) -> i32 {
    let orig: unsafe extern "system" fn(P) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { before_swap(dc) };
    unsafe { orig(dc) }
}

/// Before a swap: the corner scaled, or the bars cleared.
unsafe fn before_swap(dc: P) {
    if !gl_ready() {
        return;
    }
    if CORNER[0].load(Ordering::Relaxed) > 0 {
        unsafe { corner(dc) };
        return;
    }
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
}

/// The framebuffer object functions (GL 3), from the current context.
struct Fbo {
    gen_framebuffers: unsafe extern "system" fn(i32, *mut u32),
    gen_renderbuffers: unsafe extern "system" fn(i32, *mut u32),
    bind_framebuffer: unsafe extern "system" fn(u32, u32),
    bind_renderbuffer: unsafe extern "system" fn(u32, u32),
    renderbuffer_storage: unsafe extern "system" fn(u32, u32, i32, i32),
    framebuffer_renderbuffer: unsafe extern "system" fn(u32, u32, u32, u32),
    blit_framebuffer: unsafe extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32),
    color_mask: unsafe extern "system" fn(u8, u8, u8, u8),
    get_booleanv: unsafe extern "system" fn(u32, *mut u8),
}

fn fbo() -> Option<&'static Fbo> {
    static FBO: std::sync::OnceLock<Option<Fbo>> = std::sync::OnceLock::new();
    FBO.get_or_init(|| unsafe {
        let module = GetModuleHandleW(windows_sys::w!("opengl32.dll"));
        let wgl: unsafe extern "system" fn(*const u8) -> usize = std::mem::transmute(GetProcAddress(module, c"wglGetProcAddress".as_ptr().cast())?);
        let ext = |name: &std::ffi::CStr| -> Option<usize> { Some(wgl(name.as_ptr().cast())).filter(|&f| f > 3) };
        let core = |name: &std::ffi::CStr| -> Option<usize> { GetProcAddress(module, name.as_ptr().cast()).map(|f| f as usize) };
        Some(Fbo {
            gen_framebuffers: std::mem::transmute(ext(c"glGenFramebuffers")?),
            gen_renderbuffers: std::mem::transmute(ext(c"glGenRenderbuffers")?),
            bind_framebuffer: std::mem::transmute(ext(c"glBindFramebuffer")?),
            bind_renderbuffer: std::mem::transmute(ext(c"glBindRenderbuffer")?),
            renderbuffer_storage: std::mem::transmute(ext(c"glRenderbufferStorage")?),
            framebuffer_renderbuffer: std::mem::transmute(ext(c"glFramebufferRenderbuffer")?),
            blit_framebuffer: std::mem::transmute(ext(c"glBlitFramebuffer")?),
            color_mask: std::mem::transmute(core(c"glColorMask")?),
            get_booleanv: std::mem::transmute(core(c"glGetBooleanv")?),
        })
    })
    .as_ref()
}

/// `WAL_GL_CORNER`: the game's corner scaled over the window's back buffer (before a swap).
unsafe fn corner(dc: P) {
    const READ_FRAMEBUFFER: u32 = 0x8CA8;
    const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
    const RENDERBUFFER: u32 = 0x8D41;
    const COLOR_ATTACHMENT0: u32 = 0x8CE0;
    const COLOR_BUFFER_BIT: u32 = 0x4000;
    const SCISSOR_TEST: u32 = 0x0C11;
    const LINEAR: u32 = 0x2601;
    const NEAREST: u32 = 0x2600;
    static TARGET: AtomicUsize = AtomicUsize::new(0);
    let (w, h) = (CORNER[0].load(Ordering::Relaxed), CORNER[1].load(Ordering::Relaxed));
    let Some(f) = fbo() else {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("glview: no framebuffer objects (GL 3): not scaled"));
        return;
    };
    let hwnd = unsafe { WindowFromDC(dc) };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 {
        return;
    }
    let (ww, wh) = (r.right - r.left, r.bottom - r.top);
    if ww <= 0 || wh <= 0 || (ww, wh) == (w, h) {
        return;
    }
    let stretch = std::env::var("WAL_KEEP_ASPECT_RATIO").is_ok_and(|v| v.trim() == "0");
    unsafe {
        let enable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(0));
        let disable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(1));
        let is_enabled: unsafe extern "system" fn(u32) -> u8 = std::mem::transmute(gl(2));
        let get_integer: unsafe extern "system" fn(u32, *mut i32) = std::mem::transmute(gl(4));
        let get_float: unsafe extern "system" fn(u32, *mut f32) = std::mem::transmute(gl(5));
        let clear_color: unsafe extern "system" fn(f32, f32, f32, f32) = std::mem::transmute(gl(6));
        let clear: unsafe extern "system" fn(u32) = std::mem::transmute(gl(7));
        // the game's state, put back after
        let (mut read, mut draw) = (0, 0);
        get_integer(0x8CAA, &mut read); // GL_READ_FRAMEBUFFER_BINDING
        get_integer(0x8CA6, &mut draw); // GL_DRAW_FRAMEBUFFER_BINDING
        let scissor = is_enabled(SCISSOR_TEST);
        let mut color = [0f32; 4];
        get_float(0x0C22, color.as_mut_ptr()); // GL_COLOR_CLEAR_VALUE
        let mut mask = [0u8; 4];
        (f.get_booleanv)(0x0C23, mask.as_mut_ptr()); // GL_COLOR_WRITEMASK

        let mut target = TARGET.load(Ordering::Relaxed) as u32;
        if target == 0 {
            let mut rb = 0;
            (f.gen_framebuffers)(1, &mut target);
            (f.gen_renderbuffers)(1, &mut rb);
            (f.bind_renderbuffer)(RENDERBUFFER, rb);
            (f.renderbuffer_storage)(RENDERBUFFER, 0x8058, w, h); // GL_RGBA8
            (f.bind_framebuffer)(DRAW_FRAMEBUFFER, target);
            (f.framebuffer_renderbuffer)(DRAW_FRAMEBUFFER, COLOR_ATTACHMENT0, RENDERBUFFER, rb);
            TARGET.store(target as usize, Ordering::Relaxed);
            log!("glview: {w}x{h} corner scaled to {ww}x{wh}{}", if stretch { " (stretched)" } else { "" });
        }
        disable(SCISSOR_TEST);
        (f.bind_framebuffer)(READ_FRAMEBUFFER, 0);
        (f.bind_framebuffer)(DRAW_FRAMEBUFFER, target);
        (f.blit_framebuffer)(0, 0, w, h, 0, 0, w, h, COLOR_BUFFER_BIT, NEAREST);
        // the back buffer black, the picture over it
        (f.bind_framebuffer)(DRAW_FRAMEBUFFER, 0);
        (f.color_mask)(1, 1, 1, 1);
        clear_color(0.0, 0.0, 0.0, 1.0);
        clear(COLOR_BUFFER_BIT);
        let (sw, sh) = if stretch {
            (ww, wh)
        } else if ww as i64 * h as i64 > wh as i64 * w as i64 {
            (wh * w / h, wh)
        } else {
            (ww, ww * h / w)
        };
        let (x, y) = ((ww - sw) / 2, (wh - sh) / 2);
        (f.bind_framebuffer)(READ_FRAMEBUFFER, target);
        (f.blit_framebuffer)(0, 0, w, h, x, y, x + sw, y + sh, COLOR_BUFFER_BIT, LINEAR);
        (f.bind_framebuffer)(READ_FRAMEBUFFER, read as u32);
        (f.bind_framebuffer)(DRAW_FRAMEBUFFER, draw as u32);
        if scissor != 0 {
            enable(SCISSOR_TEST);
        }
        clear_color(color[0], color[1], color[2], color[3]);
        (f.color_mask)(mask[0], mask[1], mask[2], mask[3]);
    }
}
