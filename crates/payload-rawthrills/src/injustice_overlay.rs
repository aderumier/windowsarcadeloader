//! Injustice Arcade's card picker, drawn over the game before each `SwapBuffers` (the mobile
//! simulation renders with OpenGL): a panel per player listing the cards around the selected
//! one, while browsing and while the card is on the reader. Fixed-function GL with the game's
//! state saved around it; the text is a GDI bitmap font (`wglUseFontBitmapsW`).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{CreateFontW, DeleteObject, HDC, SelectObject, WindowFromDC};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::injustice_cards::{CARDS, panels};

static SWAP_BUFFERS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn init() {
    match unsafe { iat::hook("gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
        Some(o) => SWAP_BUFFERS.store(o, Ordering::Relaxed),
        None => log!("injustice: SwapBuffers not imported, no card overlay"),
    }
}

unsafe extern "system" fn swap_buffers(hdc: HDC) -> i32 {
    let p = panels();
    if p.iter().any(|(_, _, shown)| *shown)
        && let Some(gl) = Gl::get()
    {
        unsafe { draw(gl, hdc, &p) };
    }
    let f: unsafe extern "system" fn(HDC) -> i32 = unsafe { std::mem::transmute(SWAP_BUFFERS.load(Ordering::Relaxed)) };
    unsafe { f(hdc) }
}

// ── GL entry points ─────────────────────────────────────────────────────────────────────────

const GL_ALL_ATTRIB_BITS: u32 = 0x000f_ffff;
const GL_MODELVIEW: u32 = 0x1700;
const GL_PROJECTION: u32 = 0x1701;
const GL_TEXTURE: u32 = 0x1702;
const GL_QUADS: u32 = 7;
const GL_BLEND: u32 = 0x0be2;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_CURRENT_PROGRAM: u32 = 0x8b8d;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8ca6;
const GL_FRAMEBUFFER: u32 = 0x8d40;
const GL_VERTEX_ARRAY_BINDING: u32 = 0x85b5;
const GL_ACTIVE_TEXTURE: u32 = 0x84e0;
const GL_TEXTURE0: u32 = 0x84c0;
/// Disabled for the panel (blending is set up explicitly).
const GL_DISABLED: [u32; 11] = [
    0x0b71, // depth test
    0x0b90, // stencil test
    0x0c11, // scissor test
    0x0b44, // cull face
    0x0de1, // texture 2d
    0x0b50, // lighting
    0x0bc0, // alpha test
    0x0b60, // fog
    0x8db9, // framebuffer sRGB
    0x0bf2, // color logic op
    0x8513, // texture cube map
];

#[allow(non_snake_case)]
struct Gl {
    glPushAttrib: unsafe extern "system" fn(u32),
    glPopAttrib: unsafe extern "system" fn(),
    glMatrixMode: unsafe extern "system" fn(u32),
    glPushMatrix: unsafe extern "system" fn(),
    glPopMatrix: unsafe extern "system" fn(),
    glLoadIdentity: unsafe extern "system" fn(),
    glOrtho: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64),
    glViewport: unsafe extern "system" fn(i32, i32, i32, i32),
    glDisable: unsafe extern "system" fn(u32),
    glEnable: unsafe extern "system" fn(u32),
    glBlendFunc: unsafe extern "system" fn(u32, u32),
    glColor4f: unsafe extern "system" fn(f32, f32, f32, f32),
    glBegin: unsafe extern "system" fn(u32),
    glEnd: unsafe extern "system" fn(),
    glVertex2f: unsafe extern "system" fn(f32, f32),
    glRasterPos2f: unsafe extern "system" fn(f32, f32),
    glListBase: unsafe extern "system" fn(u32),
    glCallLists: unsafe extern "system" fn(i32, u32, *const c_void),
    glGenLists: unsafe extern "system" fn(i32) -> u32,
    glGetIntegerv: unsafe extern "system" fn(u32, *mut i32),
    wglGetProcAddress: unsafe extern "system" fn(*const u8) -> usize,
    wglGetCurrentContext: unsafe extern "system" fn() -> usize,
    wglUseFontBitmapsW: unsafe extern "system" fn(HDC, u32, u32, u32) -> i32,
}

impl Gl {
    fn get() -> Option<&'static Gl> {
        static GL: OnceLock<Option<Gl>> = OnceLock::new();
        GL.get_or_init(|| unsafe {
            let name: Vec<u16> = "opengl32.dll\0".encode_utf16().collect();
            let mut m = GetModuleHandleW(name.as_ptr());
            if m.is_null() {
                m = LoadLibraryW(name.as_ptr());
            }
            if m.is_null() {
                return None;
            }
            macro_rules! f {
                ($n:literal) => {
                    std::mem::transmute(GetProcAddress(m, concat!($n, "\0").as_ptr())?)
                };
            }
            Some(Gl {
                glPushAttrib: f!("glPushAttrib"),
                glPopAttrib: f!("glPopAttrib"),
                glMatrixMode: f!("glMatrixMode"),
                glPushMatrix: f!("glPushMatrix"),
                glPopMatrix: f!("glPopMatrix"),
                glLoadIdentity: f!("glLoadIdentity"),
                glOrtho: f!("glOrtho"),
                glViewport: f!("glViewport"),
                glDisable: f!("glDisable"),
                glEnable: f!("glEnable"),
                glBlendFunc: f!("glBlendFunc"),
                glColor4f: f!("glColor4f"),
                glBegin: f!("glBegin"),
                glEnd: f!("glEnd"),
                glVertex2f: f!("glVertex2f"),
                glRasterPos2f: f!("glRasterPos2f"),
                glListBase: f!("glListBase"),
                glCallLists: f!("glCallLists"),
                glGenLists: f!("glGenLists"),
                glGetIntegerv: f!("glGetIntegerv"),
                wglGetProcAddress: f!("wglGetProcAddress"),
                wglGetCurrentContext: f!("wglGetCurrentContext"),
                wglUseFontBitmapsW: f!("wglUseFontBitmapsW"),
            })
        })
        .as_ref()
    }

    /// An extension entry point of the current context (0 when missing).
    unsafe fn ext(&self, name: &str) -> usize {
        let n: Vec<u8> = name.bytes().chain([0]).collect();
        unsafe { (self.wglGetProcAddress)(n.as_ptr()) }
    }
}

// ── drawing ──────────────────────────────────────────────────────────────────────────────────

const FONT_HEIGHT: i32 = 22;
const LINE: f32 = 26.0;
const PANEL_WIDTH: f32 = 380.0;
/// Cards listed above and below the selected one.
const AROUND: isize = 3;

/// The font's display lists (glyphs 0..256) of the GL context they were made in.
static FONT: std::sync::Mutex<Option<(usize, u32)>> = std::sync::Mutex::new(None);

unsafe fn font_lists(gl: &Gl, hdc: HDC) -> Option<u32> {
    let context = unsafe { (gl.wglGetCurrentContext)() };
    let mut font = FONT.lock().unwrap();
    if let Some((c, base)) = *font
        && c == context
    {
        return Some(base);
    }
    let face: Vec<u16> = "Arial\0".encode_utf16().collect();
    unsafe {
        let hfont = CreateFontW(-FONT_HEIGHT, 0, 0, 0, 700, 0, 0, 0, 0, 0, 0, 4, 0, face.as_ptr());
        let old = SelectObject(hdc, hfont);
        let base = (gl.glGenLists)(256);
        let ok = (gl.wglUseFontBitmapsW)(hdc, 0, 256, base) != 0;
        SelectObject(hdc, old);
        DeleteObject(hfont);
        if !ok || base == 0 {
            return None;
        }
        *font = Some((context, base));
        Some(base)
    }
}

unsafe fn text(gl: &Gl, base: u32, x: f32, y: f32, s: &str) {
    let bytes: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
    unsafe {
        (gl.glRasterPos2f)(x, y);
        (gl.glListBase)(base);
        (gl.glCallLists)(bytes.len() as i32, GL_UNSIGNED_BYTE, bytes.as_ptr().cast());
    }
}

unsafe fn rect(gl: &Gl, x0: f32, y0: f32, x1: f32, y1: f32) {
    unsafe {
        (gl.glBegin)(GL_QUADS);
        (gl.glVertex2f)(x0, y0);
        (gl.glVertex2f)(x1, y0);
        (gl.glVertex2f)(x1, y1);
        (gl.glVertex2f)(x0, y1);
        (gl.glEnd)();
    }
}

unsafe fn draw(gl: &Gl, hdc: HDC, panels: &[(usize, bool, bool); 2]) {
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(WindowFromDC(hdc), &mut client) };
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let use_program = unsafe { gl.ext("glUseProgram") };
    let bind_framebuffer = unsafe { gl.ext("glBindFramebuffer") };
    let bind_vertex_array = unsafe { gl.ext("glBindVertexArray") };
    let active_texture = unsafe { gl.ext("glActiveTexture") };
    let mut saved = [0i32; 4];
    unsafe {
        (gl.glGetIntegerv)(GL_CURRENT_PROGRAM, &mut saved[0]);
        (gl.glGetIntegerv)(GL_DRAW_FRAMEBUFFER_BINDING, &mut saved[1]);
        (gl.glGetIntegerv)(GL_VERTEX_ARRAY_BINDING, &mut saved[2]);
        (gl.glGetIntegerv)(GL_ACTIVE_TEXTURE, &mut saved[3]);
        if use_program != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(use_program)(0);
        }
        if bind_framebuffer != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32, u32)>(bind_framebuffer)(GL_FRAMEBUFFER, 0);
        }
        if bind_vertex_array != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(bind_vertex_array)(0);
        }
        if active_texture != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(active_texture)(GL_TEXTURE0);
        }
        (gl.glPushAttrib)(GL_ALL_ATTRIB_BITS);
        for m in [GL_TEXTURE, GL_PROJECTION, GL_MODELVIEW] {
            (gl.glMatrixMode)(m);
            (gl.glPushMatrix)();
            (gl.glLoadIdentity)();
        }
        (gl.glMatrixMode)(GL_PROJECTION);
        (gl.glOrtho)(0.0, w as f64, h as f64, 0.0, -1.0, 1.0);
        (gl.glMatrixMode)(GL_MODELVIEW);
        (gl.glViewport)(0, 0, w, h);
        for cap in GL_DISABLED {
            (gl.glDisable)(cap);
        }
        (gl.glEnable)(GL_BLEND);
        (gl.glBlendFunc)(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

        if let Some(base) = font_lists(gl, hdc) {
            for (player, (selected, on_reader, shown)) in panels.iter().enumerate() {
                if *shown {
                    panel(gl, base, player, *selected, *on_reader, w as f32, h as f32);
                }
            }
        }

        for m in [GL_MODELVIEW, GL_PROJECTION, GL_TEXTURE] {
            (gl.glMatrixMode)(m);
            (gl.glPopMatrix)();
        }
        (gl.glPopAttrib)();
        if active_texture != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(active_texture)(saved[3] as u32);
        }
        if bind_vertex_array != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(bind_vertex_array)(saved[2] as u32);
        }
        if bind_framebuffer != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32, u32)>(bind_framebuffer)(GL_FRAMEBUFFER, saved[1] as u32);
        }
        if use_program != 0 {
            std::mem::transmute::<usize, unsafe extern "system" fn(u32)>(use_program)(saved[0] as u32);
        }
    }
}

unsafe fn panel(gl: &Gl, base: u32, player: usize, selected: usize, on_reader: bool, w: f32, h: f32) {
    let rows = 2 * AROUND as usize + 1;
    let height = LINE * (rows as f32 + 3.0) + 16.0;
    let x0 = if player == 0 { 16.0 } else { w - PANEL_WIDTH - 16.0 };
    let y0 = h - height - 16.0;
    unsafe {
        (gl.glColor4f)(0.0, 0.0, 0.0, 0.75);
        rect(gl, x0, y0, x0 + PANEL_WIDTH, y0 + height);
        let mut y = y0 + 8.0 + LINE - 6.0;
        (gl.glColor4f)(1.0, 0.85, 0.2, 1.0);
        text(gl, base, x0 + 12.0, y, &format!("P{} CARD  {}/{}", player + 1, selected + 1, CARDS.len()));
        y += LINE;
        for d in -AROUND..=AROUND {
            let i = (selected as isize + d).rem_euclid(CARDS.len() as isize) as usize;
            if d == 0 {
                if on_reader {
                    (gl.glColor4f)(0.1, 0.6, 0.1, 0.9);
                } else {
                    (gl.glColor4f)(0.25, 0.35, 0.8, 0.9);
                }
                rect(gl, x0 + 6.0, y - LINE + 6.0, x0 + PANEL_WIDTH - 6.0, y + 6.0);
                (gl.glColor4f)(1.0, 1.0, 1.0, 1.0);
            } else {
                (gl.glColor4f)(0.7, 0.7, 0.7, 1.0);
            }
            text(gl, base, x0 + 12.0, y, &format!("{:3}  {}", i + 1, CARDS[i].0));
            y += LINE;
        }
        y += 4.0;
        if on_reader {
            (gl.glColor4f)(0.4, 1.0, 0.4, 1.0);
            text(gl, base, x0 + 12.0, y, "ON THE READER  (card: remove)");
        } else {
            (gl.glColor4f)(0.8, 0.8, 0.8, 1.0);
            text(gl, base, x0 + 12.0, y, "up/down/left/right  card: scan");
        }
    }
}
