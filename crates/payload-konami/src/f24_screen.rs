//! Gashaaaan's picture on one screen.
//!
//! The cabinet has two 640x480 projectors side by side. The game (OpenGL, a 1280x480 window)
//! draws its scene once, 1248x468 (`sfgl_set_screen_size`) in the window's upper left, copies
//! the window to a texture and draws it back as two 640x480 halves (`fgl_draw_frame`, system.dll
//! rva 0x3f39): each half shifted by the operator's screen offsets, the left one 8 pixels to the
//! right, the right one starting 16 pixels before the left one's end (the projectors overlap).
//! On a PC that shows a band twice in the middle: util.dll's `sfgl_get_double_screen_mode`
//! answers 0 (the halves copied in place), and before each `SwapBuffers` (fgl.dll's import) the
//! scene is drawn over the whole window, scaled, aspect kept (black bars; `WAL_KEEP_ASPECT_RATIO=0`:
//! stretched to the whole window), and the players'
//! crosshairs over it (`crosshair.rs`). The players' pointers map onto that area ([`to_scene`]).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

/// The scene: its size and its place in the game's 1280x480 (GL coordinates, from the bottom:
/// its upper left).
pub const SCENE_W: i32 = 1248;
pub const SCENE_H: i32 = 468;
const SCENE_Y: i32 = 480 - SCENE_H;

/// Where the scene is shown: x, y (from the top), width, height, in a window of that size.
static PLACEMENT: Mutex<Option<[i32; 6]>> = Mutex::new(None);

/// A pointer (0..1 over the window) in scene pixels, `None` outside the picture.
pub fn to_scene(x: f32, y: f32) -> Option<(f32, f32)> {
    let Some([px, py, pw, ph, ww, wh]) = *PLACEMENT.lock().unwrap() else {
        return Some((x * SCENE_W as f32, y * SCENE_H as f32));
    };
    let sx = (x * ww as f32 - px as f32) / pw as f32;
    let sy = (y * wh as f32 - py as f32) / ph as f32;
    ((0.0..1.0).contains(&sx) && (0.0..1.0).contains(&sy)).then(|| (sx * SCENE_W as f32, sy * SCENE_H as f32))
}

extern "C" fn double_screen_mode() -> i32 {
    0
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

/// Before a swap: the scene scaled over the window's back buffer.
unsafe fn present(hdc: *mut c_void) {
    static FBO: AtomicU32 = AtomicU32::new(0);
    let Some(gl) = gl() else {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("f24: no framebuffer objects (GL 3): picture not scaled"));
        return;
    };
    let window = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(hdc as _) };
    let mut rect = unsafe { std::mem::zeroed::<RECT>() };
    if window.is_null() || unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut rect) } == 0 {
        return;
    }
    let (ww, wh) = (rect.right - rect.left, rect.bottom - rect.top);
    if ww < 1280 || wh < 480 {
        return;
    }
    let (w, h) = (SCENE_W, SCENE_H);
    let (sw, sh) = if STRETCH.load(Ordering::Relaxed) {
        (ww, wh)
    } else if ww as i64 * h as i64 > wh as i64 * w as i64 {
        (wh * w / h, wh)
    } else {
        (ww, ww * h / w)
    };
    let (x, y) = ((ww - sw) / 2, (wh - sh) / 2);
    *PLACEMENT.lock().unwrap() = Some([x, y, sw, sh, ww, wh]);
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
            log!("f24: {w}x{h} scene scaled to {sw}x{sh} in the {ww}x{wh} window");
        }
        (gl.disable)(SCISSOR_TEST);
        (gl.color_mask)(1, 1, 1, 1);
        // the scene into the framebuffer (the game's 1280x480 is the back buffer's lower left)
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, 0);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, fbo);
        (gl.blit_framebuffer)(0, SCENE_Y, w, SCENE_Y + h, 0, 0, w, h, COLOR_BUFFER_BIT, NEAREST);
        // the back buffer black, the scene over it
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, 0);
        (gl.clear_color)(0.0, 0.0, 0.0, 1.0);
        (gl.clear)(COLOR_BUFFER_BIT);
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, fbo);
        (gl.blit_framebuffer)(0, 0, w, h, x, wh - y - sh, x + sw, wh - y, COLOR_BUFFER_BIT, LINEAR);
        if CROSSHAIRS.load(Ordering::Relaxed) {
            crate::crosshair::draw_gl(ww, wh);
        }

        (gl.bind_framebuffer)(READ_FRAMEBUFFER, read as u32);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, draw as u32);
        if scissor != 0 {
            (gl.enable)(SCISSOR_TEST);
        }
        (gl.clear_color)(color[0], color[1], color[2], color[3]);
        (gl.color_mask)(mask[0], mask[1], mask[2], mask[3]);
    }
}

static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);
static CROSSHAIRS: AtomicBool = AtomicBool::new(false);
/// `WAL_KEEP_ASPECT_RATIO=0` (Batocera's KEEP ASPECT RATIO off): the scene over the whole
/// window, aspect not kept.
static STRETCH: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn swap_buffers(hdc: *mut c_void) -> i32 {
    unsafe { present(hdc) };
    let orig: unsafe extern "system" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(hdc) }
}

pub fn init() {
    CROSSHAIRS.store(crate::crosshair::init_gl(), Ordering::Relaxed);
    STRETCH.store(std::env::var("WAL_KEEP_ASPECT_RATIO").is_ok_and(|v| v == "0"), Ordering::Relaxed);
    let util = unsafe { GetModuleHandleA(c"util.dll".as_ptr().cast()) };
    match (!util.is_null()).then(|| unsafe { GetProcAddress(util, c"?sfgl_get_double_screen_mode@@YAHXZ".as_ptr().cast()) }).flatten() {
        Some(f) => unsafe { crate::f24::jump(f as usize, double_screen_mode as *const () as usize) },
        None => log!("f24: util.dll sfgl_get_double_screen_mode not found"),
    }
    let fgl = unsafe { GetModuleHandleA(c"fgl.dll".as_ptr().cast()) } as usize;
    match (fgl != 0).then(|| unsafe { iat::hook_module(fgl, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) }).flatten() {
        Some(orig) => ORIG_SWAP.store(orig, Ordering::Relaxed),
        None => log!("f24: fgl.dll SwapBuffers not hooked: picture not scaled"),
    }
}
