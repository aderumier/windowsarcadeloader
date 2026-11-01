//! UltraPin's frontend turned upright, as the tables (`WAL_ULTRAPIN_ROTATE`).
//!
//! `plfe.exe` draws the cabinet's screens with OpenGL into one 2640x1024 window: the playfield
//! monitor's 1360x768 at its left, the backglass monitor's 1280x1024 right of it (its DMD
//! strip at its right edge), both monitors turned on their side (the pictures sideways). At
//! each `SwapBuffers`, its back buffer is copied into a texture and drawn back turned 90
//! degrees clockwise into the screen (the window made its height, the tables' window covers the
//! same): the playfield upright and centered, scaled into the height, the backglass upright in
//! the margin at its left.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{HDC, WindowFromDC};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
};

/// The playfield monitor's and the backglass monitor's areas of the frontend's window.
const PLAYFIELD: (f32, f32, f32, f32) = (0.0, 0.0, 1360.0, 768.0);
const BACKGLASS: (f32, f32, f32, f32) = (1360.0, 0.0, 1280.0, 1024.0);

static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn init() {
    if let Some(o) = unsafe { iat::hook("gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
        ORIG_SWAP.store(o, Ordering::Relaxed);
        log!("ultrapin: the frontend turned upright");
    }
}

unsafe extern "system" fn swap_buffers(hdc: HDC) -> i32 {
    unsafe { turn(hdc) };
    let orig: unsafe extern "system" fn(HDC) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(hdc) }
}

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_RGB: u32 = 0x1907;
const GL_BACK: u32 = 0x0405;
const GL_ALL_ATTRIB_BITS: u32 = 0x000F_FFFF;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_MODELVIEW: u32 = 0x1700;
const GL_PROJECTION: u32 = 0x1701;
const GL_TEXTURE: u32 = 0x1702;
const GL_QUADS: u32 = 0x0007;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_LINEAR: i32 = 0x2601;
const GL_CLAMP_TO_EDGE: i32 = 0x812F;
const GL_TEXTURE_ENV: u32 = 0x2300;
const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
const GL_REPLACE: i32 = 0x1E01;
/// Capabilities the frontend may leave on: depth / alpha / stencil / scissor tests, blending,
/// lighting, fog, face culling, texture coordinate generation.
const DISABLED: [u32; 11] = [0x0B71, 0x0BC0, 0x0B90, 0x0C11, 0x0BE2, 0x0B50, 0x0B60, 0x0B44, 0x0C60, 0x0C61, 0x0DE0];

/// OpenGL 1.1, from opengl32.dll.
#[allow(non_snake_case)]
struct Gl {
    GenTextures: unsafe extern "system" fn(i32, *mut u32),
    BindTexture: unsafe extern "system" fn(u32, u32),
    TexParameteri: unsafe extern "system" fn(u32, u32, i32),
    TexEnvi: unsafe extern "system" fn(u32, u32, i32),
    CopyTexImage2D: unsafe extern "system" fn(u32, i32, u32, i32, i32, i32, i32, i32),
    ReadBuffer: unsafe extern "system" fn(u32),
    PushAttrib: unsafe extern "system" fn(u32),
    PopAttrib: unsafe extern "system" fn(),
    MatrixMode: unsafe extern "system" fn(u32),
    PushMatrix: unsafe extern "system" fn(),
    PopMatrix: unsafe extern "system" fn(),
    LoadIdentity: unsafe extern "system" fn(),
    Ortho: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64),
    Viewport: unsafe extern "system" fn(i32, i32, i32, i32),
    Enable: unsafe extern "system" fn(u32),
    Disable: unsafe extern "system" fn(u32),
    ColorMask: unsafe extern "system" fn(u8, u8, u8, u8),
    ClearColor: unsafe extern "system" fn(f32, f32, f32, f32),
    Clear: unsafe extern "system" fn(u32),
    Color4f: unsafe extern "system" fn(f32, f32, f32, f32),
    Begin: unsafe extern "system" fn(u32),
    End: unsafe extern "system" fn(),
    TexCoord2f: unsafe extern "system" fn(f32, f32),
    Vertex2f: unsafe extern "system" fn(f32, f32),
}

impl Gl {
    fn load() -> Option<Gl> {
        let module = unsafe { GetModuleHandleW("opengl32.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr()) };
        if module.is_null() {
            return None;
        }
        macro_rules! f {
            ($name:literal) => {
                unsafe { std::mem::transmute(GetProcAddress(module, concat!($name, "\0").as_ptr())?) }
            };
        }
        Some(Gl {
            GenTextures: f!("glGenTextures"),
            BindTexture: f!("glBindTexture"),
            TexParameteri: f!("glTexParameteri"),
            TexEnvi: f!("glTexEnvi"),
            CopyTexImage2D: f!("glCopyTexImage2D"),
            ReadBuffer: f!("glReadBuffer"),
            PushAttrib: f!("glPushAttrib"),
            PopAttrib: f!("glPopAttrib"),
            MatrixMode: f!("glMatrixMode"),
            PushMatrix: f!("glPushMatrix"),
            PopMatrix: f!("glPopMatrix"),
            LoadIdentity: f!("glLoadIdentity"),
            Ortho: f!("glOrtho"),
            Viewport: f!("glViewport"),
            Enable: f!("glEnable"),
            Disable: f!("glDisable"),
            ColorMask: f!("glColorMask"),
            ClearColor: f!("glClearColor"),
            Clear: f!("glClear"),
            Color4f: f!("glColor4f"),
            Begin: f!("glBegin"),
            End: f!("glEnd"),
            TexCoord2f: f!("glTexCoord2f"),
            Vertex2f: f!("glVertex2f"),
        })
    }
}

/// The texture the frame is copied into (0: not made yet).
static TEXTURE: AtomicU32 = AtomicU32::new(0);

/// The frame in the back buffer redrawn upright.
unsafe fn turn(hdc: HDC) {
    static GL: std::sync::OnceLock<Option<Gl>> = std::sync::OnceLock::new();
    let Some(gl) = GL.get_or_init(Gl::load) else { return };
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let window = unsafe { WindowFromDC(hdc) };
    if window.is_null() || unsafe { GetClientRect(window, &mut client) } == 0 {
        return;
    }
    let (w, h) = (client.right, client.bottom);
    if w < (BACKGLASS.0 + BACKGLASS.2) as i32 || h < BACKGLASS.3 as i32 {
        // not the cabinet's three-screen window
        return;
    }
    // the picture fills the screen: the window keeps the three screens' width, made the
    // screen's height (the frontend stretches its screens' pictures to its height)
    let screen = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let size = (w.max(screen.0), (BACKGLASS.3 as i32).max(screen.1));
    if (w, h) != size {
        unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE) };
        log!("ultrapin: the frontend's window made {}x{}", size.0, size.1);
        return;
    }
    let stretch = h as f32 / BACKGLASS.3;
    unsafe {
        (gl.PushAttrib)(GL_ALL_ATTRIB_BITS);
        for (mode, ortho) in [(GL_TEXTURE, false), (GL_MODELVIEW, false), (GL_PROJECTION, true)] {
            (gl.MatrixMode)(mode);
            (gl.PushMatrix)();
            (gl.LoadIdentity)();
            if ortho {
                (gl.Ortho)(0.0, w as f64, h as f64, 0.0, -1.0, 1.0);
            }
        }
        for cap in DISABLED {
            (gl.Disable)(cap);
        }
        (gl.Viewport)(0, 0, w, h);
        (gl.ColorMask)(1, 1, 1, 1);
        (gl.Enable)(GL_TEXTURE_2D);
        let mut texture = TEXTURE.load(Ordering::Relaxed);
        if texture == 0 {
            (gl.GenTextures)(1, &mut texture);
            TEXTURE.store(texture, Ordering::Relaxed);
        }
        (gl.BindTexture)(GL_TEXTURE_2D, texture);
        (gl.TexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        (gl.TexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        (gl.TexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        (gl.TexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        (gl.TexEnvi)(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_REPLACE);
        (gl.ReadBuffer)(GL_BACK);
        (gl.CopyTexImage2D)(GL_TEXTURE_2D, 0, GL_RGB, 0, 0, w, h, 0);
        (gl.ClearColor)(0.0, 0.0, 0.0, 1.0);
        (gl.Clear)(GL_COLOR_BUFFER_BIT);
        (gl.Color4f)(1.0, 1.0, 1.0, 1.0);
        let (sw, sh) = (screen.0.min(w) as f32, screen.1.min(h) as f32);
        // the playfield upright, scaled into the screen's height, centered
        let scale = (sh / PLAYFIELD.2).min(sw / PLAYFIELD.3);
        let table = (PLAYFIELD.3 * scale, PLAYFIELD.2 * scale);
        let left = (sw - table.0) / 2.0;
        let quad = |from: (f32, f32, f32, f32), to: (f32, f32, f32, f32)| upright(gl, (w as f32, h as f32), from, to);
        let at = |r: (f32, f32, f32, f32)| (r.0, r.1 * stretch, r.2, r.3 * stretch);
        quad(at(PLAYFIELD), (left, (sh - table.1) / 2.0, table.0, table.1));
        // the backglass upright in the margin
        let pad = 8.0;
        let room = (left - 2.0 * pad, sh - 2.0 * pad);
        let k = (room.0 / BACKGLASS.3).min(room.1 / BACKGLASS.2);
        let glass = (BACKGLASS.3 * k, BACKGLASS.2 * k);
        quad(at(BACKGLASS), (pad + (room.0 - glass.0) / 2.0, (sh - glass.1) / 2.0, glass.0, glass.1));
        for mode in [GL_PROJECTION, GL_MODELVIEW, GL_TEXTURE] {
            (gl.MatrixMode)(mode);
            (gl.PopMatrix)();
        }
        (gl.PopAttrib)();
    }
}

/// The frame's `from` area (x, y, width, height, from the top left) drawn turned 90 degrees
/// clockwise into `to`: its left side at the top.
fn upright(gl: &Gl, frame: (f32, f32), from: (f32, f32, f32, f32), to: (f32, f32, f32, f32)) {
    let (x0, y0, x1, y1) = (from.0, from.1, from.0 + from.2, from.1 + from.3);
    // texture coordinates: from the bottom left
    let tex = |x: f32, y: f32| (x / frame.0, 1.0 - y / frame.1);
    let corners = [
        ((to.0, to.1), tex(x0, y1)),
        ((to.0 + to.2, to.1), tex(x0, y0)),
        ((to.0 + to.2, to.1 + to.3), tex(x1, y0)),
        ((to.0, to.1 + to.3), tex(x1, y1)),
    ];
    unsafe {
        (gl.Begin)(GL_QUADS);
        for ((x, y), (s, t)) in corners {
            (gl.TexCoord2f)(s, t);
            (gl.Vertex2f)(x, y);
        }
        (gl.End)();
    }
}
