//! A second window's picture composed into the main window's (Direct3D 11): a cabinet's second
//! monitor shown on one screen. Hoshi to Tsubasa no Paradox (payload-nesica `galaxy.rs`) draws
//! its touch sub monitor in a window of its own; the payload keeps that window off the screen
//! and shows its picture in a corner of the main one.
//!
//! The presentation dispatcher (`border_d3d11.rs`): at each `Present` of the swap chain of the
//! source window, its back buffer is copied to a texture of the payload's (same device: one
//! renderer for both windows); at each `Present` of the main window's, that texture is drawn
//! over its back buffer as a quad, opaque, in a device context state of its own (the game's
//! pipeline state left untouched), with the touch screen's pointer when the payload gives one.
//! The shaders are compiled at the first frame (`d3dcompiler_47.dll`).
//!
//! With a place for the main window's own picture (`Layout::target_rect`: Mobile Suit Gundam
//! U.C. Card Builder's two screens side by side in one window), that picture is copied too, the
//! back buffer cleared black, and both drawn at their places.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const IID_TEXTURE2D: GUID = GUID::from_u128(0x6f15aaf2_d208_4e89_9ab4_489535d34f9c);
const IID_DEVICE: GUID = GUID::from_u128(0xdb6f6ddb_ac77_4e88_8253_819df9bbf140);
const IID_DEVICE1: GUID = GUID::from_u128(0xa04bfb29_08ef_43d6_a49c_a9bdbdcbe686);
const IID_CONTEXT1: GUID = GUID::from_u128(0xbb2c6faa_b5fb_4082_8e6b_388b8cfa90e1);

/// Where the picture goes: the source window, the main window, and the place in the main
/// window's picture (left, top, width, height: fractions of its back buffer), and a pointer
/// drawn over it (a touch screen's: its place in the picture, fractions). `None`: nothing drawn.
pub struct Layout {
    pub source: usize,
    pub target: usize,
    pub rect: [f32; 4],
    pub pointer: Option<(f32, f32)>,
    /// The main window's own picture's place (fractions, as `rect`), its back buffer cleared
    /// around; `None`: left as it is, the source's picture over it.
    pub target_rect: Option<[f32; 4]>,
}

static LAYOUT: OnceLock<fn() -> Option<Layout>> = OnceLock::new();

/// Composes the source window `layout()` names into its main window each frame.
pub fn init(layout: fn() -> Option<Layout>) {
    if LAYOUT.set(layout).is_ok() {
        crate::border_d3d11::register(presentation);
    }
}

macro_rules! com {
    ($obj:expr, $index:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj;
        let f: unsafe extern "system" fn(P $(, $t)*) -> $r = unsafe { std::mem::transmute(*(*(obj as *const *const usize)).add($index)) };
        unsafe { f(obj $(, $a)*) }
    }};
}

fn release(obj: P) {
    if !obj.is_null() {
        com!(obj, 2, fn() -> u32);
    }
}

#[repr(C)]
struct SwapChainDesc {
    width: u32,
    height: u32,
    refresh: [u32; 2],
    format: u32,
    scanline: u32,
    scaling: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    buffer_count: u32,
    window: P,
    windowed: i32,
    swap_effect: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, PartialEq)]
struct Texture2DDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc: u32,
}

/// The drawing's objects, for one device, and the copy of the source's last frame.
struct Resources {
    device: P,
    context1: P,
    state: P,
    vs: P,
    ps: P,
    blend: P,
    sampler: P,
    /// The copies (the source's picture, the target's own): description, texture, shader
    /// resource view.
    copy: [Option<(Texture2DDesc, P, P)>; 2],
    /// The pointer's colors: black, white (1x1 textures and their views).
    colors: [(P, P); 2],
}

unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);

const SHADER: &str = r#"
Texture2D image : register(t0);
SamplerState linear_clamp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
// a triangle covering the viewport (the quad's place), its texture coordinates 0..1 there
V vs_main(uint id : SV_VertexID) {
    V v;
    float2 p = float2((id << 1) & 2, id & 2);
    v.pos = float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
    v.uv = p;
    return v;
}
float4 ps_main(V v) : SV_Target {
    return float4(image.Sample(linear_clamp, v.uv).rgb, 1);
}
"#;

fn compile(entry: &std::ffi::CStr, target: &std::ffi::CStr) -> Option<Vec<u8>> {
    type Compile = unsafe extern "system" fn(*const u8, usize, *const i8, P, P, *const i8, *const i8, u32, u32, *mut P, *mut P) -> HRESULT;
    let module = unsafe { LoadLibraryW(windows_sys::w!("d3dcompiler_47.dll")) };
    let f = unsafe { GetProcAddress(module, c"D3DCompile".as_ptr() as *const u8) }?;
    let compile: Compile = unsafe { std::mem::transmute(f) };
    let (mut code, mut errors): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
    let hr = unsafe { compile(SHADER.as_ptr(), SHADER.len(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), entry.as_ptr(), target.as_ptr(), 0, 0, &mut code, &mut errors) };
    let blob = |b: P| -> Vec<u8> {
        let ptr = com!(b, 3, fn() -> *const u8);
        let len = com!(b, 4, fn() -> usize);
        unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
    };
    if hr < 0 || code.is_null() {
        if !errors.is_null() {
            log!("subwindow: shader {entry:?}: {}", String::from_utf8_lossy(&blob(errors)));
            release(errors);
        }
        return None;
    }
    let bytes = blob(code);
    release(code);
    release(errors);
    Some(bytes)
}

impl Resources {
    fn new(device: P, context: P) -> Option<Resources> {
        let mut r = Resources {
            device,
            context1: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            vs: std::ptr::null_mut(),
            ps: std::ptr::null_mut(),
            blend: std::ptr::null_mut(),
            sampler: std::ptr::null_mut(),
            copy: [None, None],
            colors: [(std::ptr::null_mut(), std::ptr::null_mut()); 2],
        };
        let mut device1: P = std::ptr::null_mut();
        if com!(device, 0, fn(*const GUID, *mut P) -> HRESULT, &IID_DEVICE1, &mut device1) < 0 {
            return None;
        }
        let level = com!(device, 37, fn() -> u32);
        let hr = com!(device1, 47, fn(u32, *const u32, u32, u32, *const GUID, *mut u32, *mut P) -> HRESULT, 0, &level, 1, 7, &IID_DEVICE, std::ptr::null_mut(), &mut r.state);
        release(device1);
        if hr < 0 || com!(context, 0, fn(*const GUID, *mut P) -> HRESULT, &IID_CONTEXT1, &mut r.context1) < 0 {
            return None;
        }
        let vs = compile(c"vs_main", c"vs_4_0")?;
        let ps = compile(c"ps_main", c"ps_4_0")?;
        // D3D11_BLEND_DESC: render target 0 not blended (one, zero), all written
        let mut blend = [0u32; 2 + 8 * 8];
        blend[2..10].copy_from_slice(&[0, 2, 1, 1, 2, 1, 1, 0xF]);
        // D3D11_SAMPLER_DESC: linear, clamped, comparison never, LOD 0..max
        let max = f32::MAX.to_bits();
        let sampler: [u32; 13] = [0x15, 3, 3, 3, 0, 1, 1, 0, 0, 0, 0, 0, max];
        if com!(device, 12, fn(*const u8, usize, P, *mut P) -> HRESULT, vs.as_ptr(), vs.len(), std::ptr::null_mut(), &mut r.vs) < 0
            || com!(device, 15, fn(*const u8, usize, P, *mut P) -> HRESULT, ps.as_ptr(), ps.len(), std::ptr::null_mut(), &mut r.ps) < 0
            || com!(device, 20, fn(*const u32, *mut P) -> HRESULT, blend.as_ptr(), &mut r.blend) < 0
            || com!(device, 23, fn(*const u32, *mut P) -> HRESULT, sampler.as_ptr(), &mut r.sampler) < 0
        {
            return None;
        }
        for (i, bgra) in [[0u8, 0, 0, 0xff], [0xff; 4]].iter().enumerate() {
            // B8G8R8A8_UNORM 1x1, shader resource
            let desc = Texture2DDesc { width: 1, height: 1, mip_levels: 1, array_size: 1, format: 87, sample_count: 1, bind_flags: 8, ..Default::default() };
            #[repr(C)]
            struct Data {
                ptr: *const u8,
                pitch: u32,
                slice: u32,
            }
            let data = Data { ptr: bgra.as_ptr(), pitch: 4, slice: 0 };
            let (mut tex, mut srv): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
            if com!(device, 5, fn(*const Texture2DDesc, *const Data, *mut P) -> HRESULT, &desc, &data, &mut tex) < 0
                || com!(device, 7, fn(P, P, *mut P) -> HRESULT, tex, std::ptr::null_mut(), &mut srv) < 0
            {
                release(tex);
                return None;
            }
            r.colors[i] = (tex, srv);
        }
        Some(r)
    }

    fn drop_copy(&mut self) {
        for copy in &mut self.copy {
            if let Some((_, tex, srv)) = copy.take() {
                release(srv);
                release(tex);
            }
        }
    }

    /// The source's back buffer copied (a texture of its size and format, made again when they
    /// change).
    fn capture(&mut self, index: usize, context: P, back: P, desc: &Texture2DDesc) {
        let want = Texture2DDesc { mip_levels: 1, array_size: 1, sample_count: 1, sample_quality: 0, usage: 0, bind_flags: 8, cpu_access: 0, misc: 0, ..*desc };
        if self.copy[index].as_ref().is_none_or(|(d, _, _)| *d != want) {
            if let Some((_, tex, srv)) = self.copy[index].take() {
                release(srv);
                release(tex);
            }
            let (mut tex, mut srv): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
            if com!(self.device, 5, fn(*const Texture2DDesc, P, *mut P) -> HRESULT, &want, std::ptr::null_mut(), &mut tex) < 0
                || com!(self.device, 7, fn(P, P, *mut P) -> HRESULT, tex, std::ptr::null_mut(), &mut srv) < 0
            {
                release(tex);
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| log!("subwindow: no copy of the {}x{} picture (format {})", desc.width, desc.height, desc.format));
                return;
            }
            log!("subwindow: {} picture {}x{} (format {})", if index == 0 { "source" } else { "main" }, desc.width, desc.height, desc.format);
            self.copy[index] = Some((want, tex, srv));
        }
        if let Some((_, tex, _)) = self.copy[index] {
            if desc.sample_count > 1 {
                // ResolveSubresource
                com!(context, 57, fn(P, u32, P, u32, u32) -> (), tex, 0, back, 0, desc.format);
            } else {
                // CopyResource
                com!(context, 47, fn(P, P) -> (), tex, back);
            }
        }
    }

    fn draw(&self, index: usize, clear: bool, context: P, back: P, (w, h): (f32, f32), rect: [f32; 4], pointer: Option<(f32, f32)>) {
        let Some((_, _, srv)) = self.copy[index] else { return };
        let mut rtv: P = std::ptr::null_mut();
        if com!(self.device, 9, fn(P, P, *mut P) -> HRESULT, back, std::ptr::null_mut(), &mut rtv) < 0 {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| log!("subwindow: no render target view of the back buffer"));
            return;
        }
        if clear {
            // ClearRenderTargetView: black
            com!(context, 50, fn(P, *const f32) -> (), rtv, [0.0f32, 0.0, 0.0, 1.0].as_ptr());
        }
        let viewport = [rect[0] * w, rect[1] * h, rect[2] * w, rect[3] * h, 0.0, 1.0];
        let mut previous: P = std::ptr::null_mut();
        com!(self.context1, 131, fn(P, *mut P) -> (), self.state, &mut previous);
        com!(context, 33, fn(u32, *const P, P) -> (), 1, &rtv, std::ptr::null_mut());
        com!(context, 35, fn(P, *const f32, u32) -> (), self.blend, std::ptr::null(), 0xFFFF_FFFF);
        com!(context, 24, fn(u32) -> (), 4); // TRIANGLELIST
        com!(context, 11, fn(P, P, u32) -> (), self.vs, std::ptr::null_mut(), 0);
        com!(context, 9, fn(P, P, u32) -> (), self.ps, std::ptr::null_mut(), 0);
        com!(context, 10, fn(u32, u32, *const P) -> (), 0, 1, &self.sampler);
        com!(context, 44, fn(u32, *const f32) -> (), 1, viewport.as_ptr());
        com!(context, 8, fn(u32, u32, *const P) -> (), 0, 1, &srv);
        com!(context, 13, fn(u32, u32) -> (), 3, 0);
        // the pointer: a white square in a black one, a hundredth of the screen's height
        if let Some((u, v)) = pointer {
            let (cx, cy) = (viewport[0] + u * viewport[2], viewport[1] + v * viewport[3]);
            for (half, (_, color)) in [(h * 0.006, self.colors[0]), (h * 0.004, self.colors[1])] {
                let square = [cx - half, cy - half, half * 2.0, half * 2.0, 0.0, 1.0];
                com!(context, 44, fn(u32, *const f32) -> (), 1, square.as_ptr());
                com!(context, 8, fn(u32, u32, *const P) -> (), 0, 1, &color);
                com!(context, 13, fn(u32, u32) -> (), 3, 0);
            }
        }
        let none: P = std::ptr::null_mut();
        com!(context, 8, fn(u32, u32, *const P) -> (), 0, 1, &none);
        com!(context, 33, fn(u32, *const P, P) -> (), 0, std::ptr::null(), std::ptr::null_mut());
        com!(self.context1, 131, fn(P, *mut P) -> (), previous, std::ptr::null_mut());
        release(previous);
        release(rtv);
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        self.drop_copy();
        for p in [self.context1, self.state, self.vs, self.ps, self.blend, self.sampler] {
            release(p);
        }
        for (tex, srv) in self.colors {
            release(srv);
            release(tex);
        }
    }
}

fn presentation(swap: P) {
    let Some(layout) = LAYOUT.get().and_then(|f| f()) else { return };
    let mut sd: SwapChainDesc = unsafe { std::mem::zeroed() };
    // IDXGISwapChain::GetDesc
    if com!(swap, 12, fn(*mut SwapChainDesc) -> HRESULT, &mut sd) < 0 {
        return;
    }
    let window = sd.window as usize;
    let source = window == layout.source;
    if !source && window != layout.target {
        return;
    }
    let mut back: P = std::ptr::null_mut();
    if com!(swap, 9, fn(u32, *const GUID, *mut P) -> HRESULT, 0, &IID_TEXTURE2D, &mut back) < 0 || back.is_null() {
        return;
    }
    let mut desc = Texture2DDesc::default();
    com!(back, 10, fn(*mut Texture2DDesc) -> (), &mut desc);
    let mut device: P = std::ptr::null_mut();
    com!(back, 3, fn(*mut P) -> (), &mut device);
    let mut context: P = std::ptr::null_mut();
    com!(device, 40, fn(*mut P) -> (), &mut context);
    let mut guard = RESOURCES.lock().unwrap();
    if guard.as_ref().is_none_or(|r| r.device != device) {
        *guard = None;
        *guard = Resources::new(device, context);
        if guard.is_none() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| log!("subwindow: the second window's picture cannot be drawn"));
        }
    }
    if let Some(r) = guard.as_mut() {
        let size = (desc.width as f32, desc.height as f32);
        if source {
            r.capture(0, context, back, &desc);
        } else if let Some(own) = layout.target_rect {
            r.capture(1, context, back, &desc);
            r.draw(1, true, context, back, size, own, None);
            r.draw(0, false, context, back, size, layout.rect, layout.pointer);
        } else {
            r.draw(0, false, context, back, size, layout.rect, layout.pointer);
        }
    }
    release(context);
    release(device);
    release(back);
}
