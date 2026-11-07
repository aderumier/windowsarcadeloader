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
    let clean = unsafe { border_frame(dc, false) };
    let result = unsafe { orig(dc) };
    if clean { unsafe { border_frame(dc, true) }; }
    result
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
    if corner.is_none() && viewports.is_none() && crate::border::config().is_none() {
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
        None if viewports.is_some() => log!("glview: {}x{} viewports fitted to the window", WIDTH.load(Ordering::Relaxed), HEIGHT.load(Ordering::Relaxed)),
        None => log!("glview: native Sinden frame enabled"),
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
    let clean = unsafe { border_frame(dc, false) };
    let result = unsafe { orig(dc) };
    if clean { unsafe { border_frame(dc, true) }; }
    result
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
    if WIDTH.load(Ordering::Relaxed) == 0 { return; }
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


/// A clean frame per GL context, as in linuxloader's borderFrame. No readback to the CPU.
unsafe fn border_frame(dc: P, restore: bool) -> bool {
    if crate::border::config().is_none() || !gl_ready() {
        return false;
    }
    let Some(f) = fbo() else { return false };
    let hwnd = unsafe { WindowFromDC(dc) };
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        return false;
    }
    let (w, h) = (rect.right, rect.bottom);
    if w < 2 || h < 2 {
        return false;
    }
    let Some((picture, bx, by, rgb)) = crate::border::geometry(w as u32, h as u32) else {
        return false;
    };
    thread_local! { static CLEAN: std::cell::RefCell<std::collections::HashMap<usize,(u32,u32,i32,i32)>> = std::cell::RefCell::new(std::collections::HashMap::new()); }
    unsafe {
        let get: unsafe extern "system" fn(u32, *mut i32) = std::mem::transmute(gl(4));
        let module = GetModuleHandleW(windows_sys::w!("opengl32.dll"));
        let Some(context) = GetProcAddress(module, c"wglGetCurrentContext".as_ptr().cast()) else {
            return false;
        };
        let context: unsafe extern "system" fn() -> usize = std::mem::transmute(context);
        let Some(read_buffer) = GetProcAddress(module, c"glReadBuffer".as_ptr().cast()) else {
            return false;
        };
        let Some(draw_buffer) = GetProcAddress(module, c"glDrawBuffer".as_ptr().cast()) else {
            return false;
        };
        let read_buffer: unsafe extern "system" fn(u32) = std::mem::transmute(read_buffer);
        let draw_buffer: unsafe extern "system" fn(u32) = std::mem::transmute(draw_buffer);
        let enable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(0));
        let disable: unsafe extern "system" fn(u32) = std::mem::transmute(gl(1));
        let is_enabled: unsafe extern "system" fn(u32) -> u8 = std::mem::transmute(gl(2));
        let scissor_fn: unsafe extern "system" fn(i32, i32, i32, i32) = std::mem::transmute(gl(3));
        let get_float: unsafe extern "system" fn(u32, *mut f32) = std::mem::transmute(gl(5));
        let clear_color: unsafe extern "system" fn(f32, f32, f32, f32) = std::mem::transmute(gl(6));
        let clear: unsafe extern "system" fn(u32) = std::mem::transmute(gl(7));
        let (mut read, mut draw, mut rb, mut readbuf, mut drawbuf) = (0, 0, 0, 0, 0);
        get(0x8CAA, &mut read);
        get(0x8CA6, &mut draw);
        get(0x8CA7, &mut rb);
        let mut color = [0.0; 4];
        get_float(0x0C22, color.as_mut_ptr());
        let mut mask = [0u8; 4];
        (f.get_booleanv)(0x0C23, mask.as_mut_ptr());
        let mut boxed = [0i32; 4];
        get(0x0C10, boxed.as_mut_ptr());
        let scissor = is_enabled(0x0C11);
        (f.bind_framebuffer)(0x8CA8, 0);
        (f.bind_framebuffer)(0x8CA9, 0);
        get(0x0C02, &mut readbuf);
        get(0x0C01, &mut drawbuf);
        read_buffer(0x0405);
        draw_buffer(0x0405); // GL_BACK
        disable(0x0C11);
        let ok = CLEAN.with(|state| {
            let mut state = state.borrow_mut();
            let entry = state.entry(context()).or_insert((0, 0, 0, 0));
            if restore && (entry.0 == 0 || entry.2 != w || entry.3 != h) {
                return false;
            }
            if entry.0 == 0 {
                (f.gen_framebuffers)(1, &mut entry.0);
                (f.gen_renderbuffers)(1, &mut entry.1);
            }
            if (entry.2, entry.3) != (w, h) {
                (f.bind_renderbuffer)(0x8D41, entry.1);
                (f.renderbuffer_storage)(0x8D41, 0x8058, w, h);
                (f.bind_framebuffer)(0x8CA9, entry.0);
                (f.framebuffer_renderbuffer)(0x8CA9, 0x8CE0, 0x8D41, entry.1);
                entry.2 = w;
                entry.3 = h;
            }
            if restore {
                (f.bind_framebuffer)(0x8CA8, entry.0);
                (f.bind_framebuffer)(0x8CA9, 0);
                (f.blit_framebuffer)(0, 0, w, h, 0, 0, w, h, 0x4000, 0x2600);
            } else {
                (f.bind_framebuffer)(0x8CA8, 0);
                (f.bind_framebuffer)(0x8CA9, entry.0);
                (f.blit_framebuffer)(0, 0, w, h, 0, 0, w, h, 0x4000, 0x2600);
                (f.bind_framebuffer)(0x8CA9, 0);
                (f.color_mask)(1, 1, 1, 1);
                clear_color(0.0, 0.0, 0.0, 1.0);
                clear(0x4000);
                enable(0x0C11);
                clear_color(
                    ((rgb >> 16) & 255) as f32 / 255.0,
                    ((rgb >> 8) & 255) as f32 / 255.0,
                    (rgb & 255) as f32 / 255.0,
                    1.0,
                );
                let (ox, oy) = crate::border::outline(w as u32, h as u32);
                for [l, t, r, b] in crate::border::strips(w, h, bx, by, ox, oy) {
                    scissor_fn(l, h - b, r - l, b - t);
                    clear(0x4000);
                }
                disable(0x0C11);
                if let Some((bezel, bw, bh)) = border_bezel_gl(f, context()) {
                    if let Some((outer, _, _, _)) =
                        crate::border::outer_geometry(w as u32, h as u32)
                    {
                        (f.bind_framebuffer)(0x8CA8, bezel);
                        (f.bind_framebuffer)(0x8CA9, 0);
                        (f.blit_framebuffer)(
                            0,
                            bh,
                            bw,
                            0,
                            outer[0],
                            h - outer[3],
                            outer[2],
                            h - outer[1],
                            0x4000,
                            0x2601,
                        );
                    }
                }
                (f.bind_framebuffer)(0x8CA9, 0);
                (f.bind_framebuffer)(0x8CA8, entry.0);
                (f.blit_framebuffer)(
                    0,
                    0,
                    w,
                    h,
                    picture[0],
                    h - picture[3],
                    picture[2],
                    h - picture[1],
                    0x4000,
                    0x2601,
                );
            }
            true
        });
        (f.bind_framebuffer)(0x8CA8, 0);
        (f.bind_framebuffer)(0x8CA9, 0);
        read_buffer(readbuf as u32);
        draw_buffer(drawbuf as u32);
        (f.bind_framebuffer)(0x8CA8, read as u32);
        (f.bind_framebuffer)(0x8CA9, draw as u32);
        (f.bind_renderbuffer)(0x8D41, rb as u32);
        clear_color(color[0], color[1], color[2], color[3]);
        (f.color_mask)(mask[0], mask[1], mask[2], mask[3]);
        scissor_fn(boxed[0], boxed[1], boxed[2], boxed[3]);
        if scissor != 0 {
            enable(0x0C11);
        } else {
            disable(0x0C11);
        }
        ok
    }
}

/// Uploads the static bezel once per GL context, keeping it separate from the clean frame.
unsafe fn border_bezel_gl(f: &Fbo, context: usize) -> Option<(u32, i32, i32)> {
    let image = crate::border::backdrop()?;
    thread_local! { static BEZELS:std::cell::RefCell<std::collections::HashMap<usize,u32>>=std::cell::RefCell::new(std::collections::HashMap::new()); }
    if let Some(fbo) = BEZELS.with(|b| b.borrow().get(&context).copied()) {
        return Some((fbo, image.w as i32, image.h as i32));
    }
    unsafe {
        let module = GetModuleHandleW(windows_sys::w!("opengl32.dll"));
        let core = |name: &std::ffi::CStr| {
            GetProcAddress(module, name.as_ptr().cast()).map(|p| p as usize)
        };
        let wgl: unsafe extern "system" fn(*const u8) -> usize =
            std::mem::transmute(core(c"wglGetProcAddress")?);
        let ext = |name: &std::ffi::CStr| Some(wgl(name.as_ptr().cast())).filter(|p| *p > 3);
        let generate_texture: unsafe extern "system" fn(i32, *mut u32) =
            std::mem::transmute(core(c"glGenTextures")?);
        let bind: unsafe extern "system" fn(u32, u32) =
            std::mem::transmute(core(c"glBindTexture")?);
        let upload: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const u8) =
            std::mem::transmute(core(c"glTexImage2D")?);
        let store: unsafe extern "system" fn(u32, i32) =
            std::mem::transmute(core(c"glPixelStorei")?);
        let bind_buffer: unsafe extern "system" fn(u32, u32) =
            std::mem::transmute(ext(c"glBindBuffer")?);
        let attach: unsafe extern "system" fn(u32, u32, u32, u32, i32) =
            std::mem::transmute(ext(c"glFramebufferTexture2D")?);
        let get: unsafe extern "system" fn(u32, *mut i32) = std::mem::transmute(gl(4));
        let (mut old_texture, mut pbo) = (0, 0);
        get(0x8069, &mut old_texture);
        get(0x88EF, &mut pbo);
        let mut unpack = [0i32; 4];
        for (i, tag) in [0x0CF2, 0x0CF3, 0x0CF4, 0x0CF5].into_iter().enumerate() {
            get(tag, &mut unpack[i]);
            store(tag, if i == 3 { 4 } else { 0 });
        }
        bind_buffer(0x88EC, 0);
        let (mut texture, mut target) = (0, 0);
        generate_texture(1, &mut texture);
        bind(0x0DE1, texture);
        upload(
            0x0DE1,
            0,
            0x8058,
            image.w as i32,
            image.h as i32,
            0,
            0x80E1,
            0x1401,
            image.bgra.as_ptr(),
        );
        bind(0x0DE1, old_texture as u32);
        bind_buffer(0x88EC, pbo as u32);
        for (i, tag) in [0x0CF2, 0x0CF3, 0x0CF4, 0x0CF5].into_iter().enumerate() {
            store(tag, unpack[i]);
        }
        (f.gen_framebuffers)(1, &mut target);
        (f.bind_framebuffer)(0x8CA9, target);
        attach(0x8CA9, 0x8CE0, 0x0DE1, texture, 0);
        BEZELS.with(|b| b.borrow_mut().insert(context, target));
        Some((target, image.w as i32, image.h as i32))
    }
}
