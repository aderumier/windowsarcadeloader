//! An OpenGL game drawn at its own size, scaled to the screen: `WAL_RINGEDGE_GL_SCALE=<w>x<h>`
//! (with `WAL_WINDOW_POPUP=1`: full screen without a display mode change).
//!
//! Initial D 8 only works with a 1360x768 window (another size, or its full screen mode,
//! crashes it at start, a screen-sized window too). Its window keeps its size during the
//! start and is enlarged to the screen 5 seconds after its first frame. The game's window
//! procedures get `WM_SIZE` with `<w>x<h>` (its classes' procedures wrapped) and its
//! `GetClientRect` reports that size: it draws that much, in the lower left corner of its
//! window's back buffer; before each `wglSwapBuffers` that
//! corner is copied (a framebuffer of its own) and drawn back over the whole back buffer,
//! scaled, aspect kept (black bars).

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::RECT;

static WIDTH: AtomicU32 = AtomicU32::new(0);
static HEIGHT: AtomicU32 = AtomicU32::new(0);
static ORIG_CLIENT_RECT: AtomicUsize = AtomicUsize::new(0);

/// `WAL_RINGEDGE_GL_SCALE` set: the game's `GetClientRect` hooked. Returns whether the swaps
/// must go through [`present`].
pub(crate) fn init() -> bool {
    let Some((w, h)) = std::env::var("WAL_RINGEDGE_GL_SCALE").ok().and_then(|v| {
        let (w, h) = v.trim().split_once('x')?;
        Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
    }) else {
        return false;
    };
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store(h, Ordering::Relaxed);
    if let Some(o) = unsafe { iat::hook("user32.dll", "GetClientRect", get_client_rect as *const () as usize) } {
        ORIG_CLIENT_RECT.store(o, Ordering::Relaxed);
    }
    for (name, hook, orig) in [
        ("RegisterClassA", register_class_a as *const () as usize, &ORIG_REGISTER_A),
        ("RegisterClassExW", register_class_ex_w as *const () as usize, &ORIG_REGISTER_EX_W),
    ] {
        if let Some(o) = unsafe { iat::hook("user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
    log!("glscale: drawn at {w}x{h}, scaled to the window");
    true
}

static ORIG_REGISTER_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_REGISTER_EX_W: AtomicUsize = AtomicUsize::new(0);
/// The wrapped window procedures, by [`window_proc`] slot.
static PROCS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
type WindowProc = unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize;
const WRAPPERS: [WindowProc; 4] = [window_proc::<0>, window_proc::<1>, window_proc::<2>, window_proc::<3>];

/// `proc` wrapped by a free [`window_proc`] slot (itself when none is free).
fn wrap(proc: usize) -> usize {
    if proc == 0 {
        return proc;
    }
    for (slot, wrapper) in PROCS.iter().zip(WRAPPERS) {
        if slot.compare_exchange(0, proc, Ordering::Relaxed, Ordering::Relaxed).is_ok() || slot.load(Ordering::Relaxed) == proc {
            return wrapper as usize;
        }
    }
    log!("glscale: no free window procedure slot");
    proc
}

unsafe extern "system" fn window_proc<const N: usize>(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize {
    let proc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize = unsafe { std::mem::transmute(PROCS[N].load(Ordering::Relaxed)) };
    // WM_SIZE (not minimized) of a top-level window: the game's size
    let lparam = if message == 0x0005 && wparam != 1 && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(window as _) }.is_null() {
        ((HEIGHT.load(Ordering::Relaxed) << 16) | (WIDTH.load(Ordering::Relaxed) & 0xFFFF)) as isize
    } else {
        lparam
    };
    unsafe { proc(window, message, wparam, lparam) }
}

unsafe extern "system" fn register_class_a(class: *mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSA) -> u16 {
    let orig: unsafe extern "system" fn(*mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSA) -> u16 = unsafe { std::mem::transmute(ORIG_REGISTER_A.load(Ordering::Relaxed)) };
    if !class.is_null() {
        let mut copy = unsafe { std::ptr::read(class) };
        copy.lpfnWndProc = unsafe { std::mem::transmute(wrap(copy.lpfnWndProc.map_or(0, |f| f as usize))) };
        return unsafe { orig(&mut copy) };
    }
    unsafe { orig(class) }
}

unsafe extern "system" fn register_class_ex_w(class: *mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSEXW) -> u16 {
    let orig: unsafe extern "system" fn(*mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSEXW) -> u16 = unsafe { std::mem::transmute(ORIG_REGISTER_EX_W.load(Ordering::Relaxed)) };
    if !class.is_null() {
        let mut copy = unsafe { std::ptr::read(class) };
        copy.lpfnWndProc = unsafe { std::mem::transmute(wrap(copy.lpfnWndProc.map_or(0, |f| f as usize))) };
        return unsafe { orig(&mut copy) };
    }
    unsafe { orig(class) }
}

unsafe extern "system" fn get_client_rect(window: *mut c_void, rect: *mut RECT) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, *mut RECT) -> i32 = unsafe { std::mem::transmute(ORIG_CLIENT_RECT.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(window, rect) };
    if ok != 0 && !rect.is_null() && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(window as _) }.is_null() {
        unsafe {
            (*rect).left = 0;
            (*rect).top = 0;
            (*rect).right = WIDTH.load(Ordering::Relaxed) as i32;
            (*rect).bottom = HEIGHT.load(Ordering::Relaxed) as i32;
        }
    }
    ok
}

/// The GL 3 functions used, from the current context.
struct Gl {
    gen_framebuffers: unsafe extern "system" fn(i32, *mut u32),
    gen_renderbuffers: unsafe extern "system" fn(i32, *mut u32),
    bind_framebuffer: unsafe extern "system" fn(u32, u32),
    bind_renderbuffer: unsafe extern "system" fn(u32, u32),
    renderbuffer_storage: unsafe extern "system" fn(u32, u32, i32, i32),
    framebuffer_renderbuffer: unsafe extern "system" fn(u32, u32, u32, u32),
    blit_framebuffer: unsafe extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32),
    get_integerv: unsafe extern "system" fn(u32, *mut i32),
    is_enabled: unsafe extern "system" fn(u32) -> u8,
    enable: unsafe extern "system" fn(u32),
    disable: unsafe extern "system" fn(u32),
    clear_color: unsafe extern "system" fn(f32, f32, f32, f32),
    get_floatv: unsafe extern "system" fn(u32, *mut f32),
    clear: unsafe extern "system" fn(u32),
    color_mask: unsafe extern "system" fn(u8, u8, u8, u8),
    get_booleanv: unsafe extern "system" fn(u32, *mut u8),
}

fn gl() -> Option<&'static Gl> {
    static GL: std::sync::OnceLock<Option<Gl>> = std::sync::OnceLock::new();
    GL.get_or_init(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        let module = GetModuleHandleA(c"opengl32.dll".as_ptr().cast());
        let wgl: unsafe extern "system" fn(*const c_char) -> usize = std::mem::transmute(GetProcAddress(module, c"wglGetProcAddress".as_ptr().cast())?);
        let ext = |name: &CStr| -> Option<usize> { Some(wgl(name.as_ptr())).filter(|&f| f > 3) };
        let core = |name: &CStr| -> Option<usize> { GetProcAddress(module, name.as_ptr().cast()).map(|f| f as usize) };
        Some(Gl {
            gen_framebuffers: std::mem::transmute(ext(c"glGenFramebuffers")?),
            gen_renderbuffers: std::mem::transmute(ext(c"glGenRenderbuffers")?),
            bind_framebuffer: std::mem::transmute(ext(c"glBindFramebuffer")?),
            bind_renderbuffer: std::mem::transmute(ext(c"glBindRenderbuffer")?),
            renderbuffer_storage: std::mem::transmute(ext(c"glRenderbufferStorage")?),
            framebuffer_renderbuffer: std::mem::transmute(ext(c"glFramebufferRenderbuffer")?),
            blit_framebuffer: std::mem::transmute(ext(c"glBlitFramebuffer")?),
            get_integerv: std::mem::transmute(core(c"glGetIntegerv")?),
            is_enabled: std::mem::transmute(core(c"glIsEnabled")?),
            enable: std::mem::transmute(core(c"glEnable")?),
            disable: std::mem::transmute(core(c"glDisable")?),
            clear_color: std::mem::transmute(core(c"glClearColor")?),
            get_floatv: std::mem::transmute(core(c"glGetFloatv")?),
            clear: std::mem::transmute(core(c"glClear")?),
            color_mask: std::mem::transmute(core(c"glColorMask")?),
            get_booleanv: std::mem::transmute(core(c"glGetBooleanv")?),
        })
    })
    .as_ref()
}

const READ_FRAMEBUFFER: u32 = 0x8CA8;
const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const RENDERBUFFER: u32 = 0x8D41;
const COLOR_ATTACHMENT0: u32 = 0x8CE0;
const COLOR_BUFFER_BIT: u32 = 0x4000;
const SCISSOR_TEST: u32 = 0x0C11;
const LINEAR: u32 = 0x2601;
const NEAREST: u32 = 0x2600;

/// Before a swap: the game's `<w>x<h>` corner scaled over the window's back buffer.
pub(crate) unsafe fn present(hdc: *mut c_void) {
    static FBO: AtomicU32 = AtomicU32::new(0);
    let (w, h) = (WIDTH.load(Ordering::Relaxed) as i32, HEIGHT.load(Ordering::Relaxed) as i32);
    if w == 0 {
        return;
    }
    let Some(gl) = gl() else {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("glscale: no framebuffer objects (GL 3): not scaled"));
        return;
    };
    let window = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(hdc as _) };
    let mut rect = unsafe { std::mem::zeroed::<RECT>() };
    // the real size (not the game's GetClientRect)
    let orig = |window, rect: &mut RECT| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(window, rect) };
    if window.is_null() || orig(window, &mut rect) == 0 {
        return;
    }
    let (mut ww, mut wh) = (rect.right - rect.left, rect.bottom - rect.top);
    // the window enlarged to the screen once the game has started
    static FIRST: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static ENLARGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let first = *FIRST.get_or_init(std::time::Instant::now);
    if !ENLARGED.load(Ordering::Relaxed) && first.elapsed().as_secs() >= 5 {
        ENLARGED.store(true, Ordering::Relaxed);
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SetWindowPos, SWP_NOZORDER, SWP_NOACTIVATE};
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let top = unsafe { GetAncestor(window, GA_ROOT) };
        unsafe { SetWindowPos(top, std::ptr::null_mut(), 0, 0, sw, sh, SWP_NOZORDER | SWP_NOACTIVATE) };
        log!("glscale: window enlarged to {sw}x{sh}");
        if orig(window, &mut rect) != 0 {
            (ww, wh) = (rect.right - rect.left, rect.bottom - rect.top);
        }
    }
    if ww <= 0 || wh <= 0 || (ww == w && wh == h) {
        return;
    }
    unsafe {
        // the game's state, restored after
        let (mut read, mut draw) = (0, 0);
        (gl.get_integerv)(0x8CAA, &mut read); // GL_READ_FRAMEBUFFER_BINDING
        (gl.get_integerv)(0x8CA6, &mut draw); // GL_DRAW_FRAMEBUFFER_BINDING
        let scissor = (gl.is_enabled)(SCISSOR_TEST);
        let mut color = [0f32; 4];
        (gl.get_floatv)(0x0C22, color.as_mut_ptr()); // GL_COLOR_CLEAR_VALUE
        let mut mask = [0u8; 4];
        (gl.get_booleanv)(0x0C23, mask.as_mut_ptr()); // GL_COLOR_WRITEMASK

        let mut fbo = FBO.load(Ordering::Relaxed);
        if fbo == 0 {
            let mut rb = 0;
            (gl.gen_framebuffers)(1, &mut fbo);
            (gl.gen_renderbuffers)(1, &mut rb);
            (gl.bind_renderbuffer)(RENDERBUFFER, rb);
            (gl.renderbuffer_storage)(RENDERBUFFER, 0x8058, w, h); // GL_RGBA8
            (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, fbo);
            (gl.framebuffer_renderbuffer)(DRAW_FRAMEBUFFER, COLOR_ATTACHMENT0, RENDERBUFFER, rb);
            FBO.store(fbo, Ordering::Relaxed);
            log!("glscale: {w}x{h} scaled to {ww}x{wh}");
        }
        (gl.disable)(SCISSOR_TEST);
        // the corner into the framebuffer
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, 0);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, fbo);
        (gl.blit_framebuffer)(0, 0, w, h, 0, 0, w, h, COLOR_BUFFER_BIT, NEAREST);
        // the back buffer black, the picture over it, aspect kept, centered
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, 0);
        (gl.color_mask)(1, 1, 1, 1);
        (gl.clear_color)(0.0, 0.0, 0.0, 1.0);
        (gl.clear)(COLOR_BUFFER_BIT);
        let (sw, sh) = if ww as i64 * h as i64 > wh as i64 * w as i64 { (wh * w / h, wh) } else { (ww, ww * h / w) };
        let (x, y) = ((ww - sw) / 2, (wh - sh) / 2);
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, fbo);
        (gl.blit_framebuffer)(0, 0, w, h, x, y, x + sw, y + sh, COLOR_BUFFER_BIT, LINEAR);

        (gl.bind_framebuffer)(READ_FRAMEBUFFER, read as u32);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, draw as u32);
        if scissor != 0 {
            (gl.enable)(SCISSOR_TEST);
        }
        (gl.clear_color)(color[0], color[1], color[2], color[3]);
        (gl.color_mask)(mask[0], mask[1], mask[2], mask[3]);
    }
}
