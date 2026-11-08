//! A touch screen's pointer drawn over a Direct3D 11 game's picture: a dot (a disc with a black
//! outline) where the mouse or gun touches, the Direct3D 11 counterpart of the common
//! crosshairs (`WAL_CROSSHAIR_STYLE=disc`); or a gun's crosshair (a ring, four arms and
//! a center dot, outlined in black).
//!
//! `IDXGISwapChain::Present` / `Present1` are replaced in the swap chains' vtable (found on a
//! swap chain of the payload's own, a hidden window's: DXVK's swap chains share one vtable).
//! Before each frame the dot is drawn in the back buffer, in a device context state of its own
//! (`SwapDeviceContextState`): the game's pipeline state is left untouched. Shaders compiled at
//! the first frame (`d3dcompiler_47.dll`).
//!
//! `WAL_SCREENSHOT=<seconds>` (`screenshot.rs`'s test screenshots) works here too: the back
//! buffer, the dot drawn, copied to a staging texture and written as `shot-NNNN.bmp`.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const IID_TEXTURE2D: GUID = GUID::from_u128(0x6f15aaf2_d208_4e89_9ab4_489535d34f9c);
const IID_DEVICE: GUID = GUID::from_u128(0xdb6f6ddb_ac77_4e88_8253_819df9bbf140);
const IID_DEVICE1: GUID = GUID::from_u128(0xa04bfb29_08ef_43d6_a49c_a9bdbdcbe686);
const IID_CONTEXT1: GUID = GUID::from_u128(0xbb2c6faa_b5fb_4082_8e6b_388b8cfa90e1);



/// The dot's look and where it is.
pub struct Dot {
    /// Red, green, blue (0..1).
    pub color: [f32; 3],
    /// Its diameter, fraction of the picture's height.
    pub size: f32,
    /// Where it is (fractions of the picture from its top left corner); `None`: not shown.
    pub position: fn() -> Option<(f32, f32)>,
    /// A crosshair instead of a disc.
    pub crosshair: bool,
}

static DOT: OnceLock<Dot> = OnceLock::new();

/// The other players' guns: how many players, where each one points (as `Dot::position`).
static PLAYERS: OnceLock<(usize, fn(usize) -> Option<(f32, f32)>)> = OnceLock::new();
/// Players 2 to 4's colors (blue, green, yellow).
const COLORS: [[f32; 3]; 3] = [[0.15, 0.45, 1.0], [0.25, 0.9, 0.2], [1.0, 0.85, 0.1]];

/// Where players 2 to 4 pointed last and when they moved: an idle gun (a player not playing,
/// its stick at the picture's center) is not shown, as the common crosshairs (`WAL_CROSSHAIR_HIDE`).
static MOVED: Mutex<[Option<((f32, f32), Option<Instant>)>; 3]> = Mutex::new([None; 3]);

/// Draws the other players' crosshairs too (players 2 to `count`, at most 4), in their colors,
/// the dot's shape and size (`position(player)`, `player` from 0), while their gun moves.
pub fn more_players(count: usize, position: fn(usize) -> Option<(f32, f32)>) {
    let _ = PLAYERS.set((count.min(1 + COLORS.len()), position));
}

/// Draws `dot` over the game's picture from its first frame on.
pub fn init(dot: Dot) {
    if DOT.set(dot).is_ok() {
        crate::border_d3d11::register(presentation);
    }
}

/// Calls the vtable method `index` of the COM object `obj`.
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
#[derive(Default)]
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

/// Waits for the game's Direct3D 11, then replaces `Present` in the swap chains' vtable.
fn presentation(swap: P) { overlay(swap); if crate::border::config().is_none() { capture(swap); } }

/// The objects of the drawing, for one device.
struct Resources {
    device: P,
    context1: P,
    state: P,
    vs: P,
    ps: P,
    blend: P,
    constants: P,
}

unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);

const SHADER: &str = r#"
cbuffer Dot : register(b0) { float4 dot; float4 color; };
float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
}
// a centered box (half sizes h) at offset c: signed distance
float box(float2 v, float2 c, float2 h) {
    float2 q = abs(v - c) - h;
    return length(max(q, 0)) + min(max(q.x, q.y), 0);
}
float4 ps_main(float4 pos : SV_Position) : SV_Target {
    float d = length(pos.xy - dot.xy);
    if (color.w > 1.5) {
        // crosshair: ring, four arms, center dot (half line width w), outline dot.w
        float2 v = abs(pos.xy - dot.xy);
        float r = dot.z, w = max(r * 0.07, 1.0);
        float sd = abs(d - r * 0.7) - w;
        sd = min(sd, box(v, float2(0, r * 0.65), float2(w, r * 0.3)));
        sd = min(sd, box(v, float2(r * 0.65, 0), float2(r * 0.3, w)));
        sd = min(sd, d - w * 1.5);
        float edge = saturate(dot.w - sd + 0.5);
        if (edge <= 0) discard;
        return float4(color.rgb * saturate(0.5 - sd), edge);
    }
    float a = saturate(dot.z - d + 0.5);
    if (a <= 0) discard;
    float inside = saturate(dot.z - dot.w - d + 0.5);
    return float4(color.rgb * inside, a);
}
"#;

/// Compiles `entry` of the shader for `target` (`vs_4_0`, `ps_4_0`).
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
            log!("pointer: shader {entry:?}: {}", String::from_utf8_lossy(&blob(errors)));
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
            constants: std::ptr::null_mut(),
        };
        // a context state of our own (its own pipeline state)
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
        // D3D11_BLEND_DESC: render target 0 blended (src alpha, inverse src alpha), all written
        let mut blend = [0u32; 2 + 8 * 8];
        blend[2..10].copy_from_slice(&[1, 5, 6, 1, 2, 6, 1, 0xF]);
        // CONSTANT_BUFFER, 32 bytes
        let buffer: [u32; 6] = [32, 0, 4, 0, 0, 0];
        if com!(device, 12, fn(*const u8, usize, P, *mut P) -> HRESULT, vs.as_ptr(), vs.len(), std::ptr::null_mut(), &mut r.vs) < 0
            || com!(device, 15, fn(*const u8, usize, P, *mut P) -> HRESULT, ps.as_ptr(), ps.len(), std::ptr::null_mut(), &mut r.ps) < 0
            || com!(device, 20, fn(*const u32, *mut P) -> HRESULT, blend.as_ptr(), &mut r.blend) < 0
            || com!(device, 3, fn(*const u32, P, *mut P) -> HRESULT, buffer.as_ptr(), std::ptr::null_mut(), &mut r.constants) < 0
        {
            return None;
        }
        Some(r)
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        for p in [self.context1, self.state, self.vs, self.ps, self.blend, self.constants] {
            release(p);
        }
    }
}

fn overlay(swap: P) {
    let Some(dot) = DOT.get() else { return };
    let mut dots: Vec<((f32, f32), [f32; 3])> = (dot.position)().map(|p| (p, dot.color)).into_iter().collect();
    if let Some((count, position)) = PLAYERS.get() {
        let now = Instant::now();
        let mut moved = MOVED.lock().unwrap();
        for player in 1..*count {
            let Some(p) = position(player) else { continue };
            let last = &mut moved[player - 1];
            if last.is_none_or(|(at, _)| at != p) {
                // its first position is the idle stick's: not a move
                *last = Some((p, last.is_some().then_some(now)));
            }
            if last.and_then(|(_, m)| m).is_some_and(|m| crate::crosshair::idle().is_none_or(|idle| now.duration_since(m) < idle)) {
                dots.push((p, COLORS[player - 1]));
            }
        }
    }
    if dots.is_empty() {
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
            ONCE.call_once(|| log!("pointer: the pointer cannot be drawn"));
        }
    }
    if let Some(r) = guard.as_ref() {
        let (w, h) = (desc.width as f32, desc.height as f32);
        let radius = (dot.size * h / 2.0).max(3.0);
        for ((x, y), color) in dots {
            draw(r, context, back, (x * w, y * h), radius, color, dot.crosshair);
        }
    }
    release(context);
    release(device);
    release(back);
}

/// `WAL_SCREENSHOT`: the back buffer written as a BMP when a screenshot is due.
pub(crate) fn capture(swap: P) {
    let Some(path) = crate::screenshot::screenshot_due() else { return };
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
    // R8G8B8A8 (28, 29 sRGB), B8G8R8A8 (87, 91 sRGB) or R10G10B10A2 (24: Unreal Engine 4),
    // not multisampled
    let rgba = matches!(desc.format, 28 | 29);
    let rgb10 = desc.format == 24;
    if (rgba || rgb10 || matches!(desc.format, 87 | 91)) && desc.sample_count == 1 {
        let staging_desc = Texture2DDesc {
            mip_levels: 1,
            array_size: 1,
            usage: 3,           // D3D11_USAGE_STAGING
            bind_flags: 0,
            cpu_access: 0x20000, // D3D11_CPU_ACCESS_READ
            misc: 0,
            ..desc
        };
        let mut staging: P = std::ptr::null_mut();
        if com!(device, 5, fn(*const Texture2DDesc, P, *mut P) -> HRESULT, &staging_desc, std::ptr::null_mut(), &mut staging) >= 0 {
            com!(context, 47, fn(P, P) -> (), staging, back);
            // D3D11_MAPPED_SUBRESOURCE: data, row pitch, depth pitch
            let mut mapped = [0usize; 3];
            if com!(context, 14, fn(P, u32, u32, u32, *mut usize) -> HRESULT, staging, 0, 1, 0, mapped.as_mut_ptr()) >= 0 {
                let (w, h, pitch) = (desc.width as usize, desc.height as usize, mapped[1] as u32 as usize);
                let mut pixels = Vec::with_capacity(w * h * 4);
                for row in 0..h {
                    let line = unsafe { std::slice::from_raw_parts((mapped[0] as *const u8).add(row * pitch), w * 4) };
                    for px in line.chunks_exact(4) {
                        let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
                        let c = |shift: u32| ((v >> shift & 0x3FF) >> 2) as u8;
                        pixels.extend_from_slice(&if rgb10 {
                            [c(20), c(10), c(0), 0xFF]
                        } else if rgba { [px[2], px[1], px[0], 0xFF] } else { [px[0], px[1], px[2], 0xFF] });
                    }
                }
                com!(context, 15, fn(P, u32) -> (), staging, 0);
                match crate::screenshot::write_bmp(&path, desc.width, desc.height, &pixels) {
                    Ok(()) => log!("pointer: screenshot {path}"),
                    Err(e) => log!("pointer: screenshot {path}: {e}"),
                }
            }
            release(staging);
        }
    }
    release(context);
    release(device);
    release(back);
}

fn draw(r: &Resources, context: P, back: P, (x, y): (f32, f32), radius: f32, color: [f32; 3], crosshair: bool) {
    let mut rtv: P = std::ptr::null_mut();
    if com!(r.device, 9, fn(P, P, *mut P) -> HRESULT, back, std::ptr::null_mut(), &mut rtv) < 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("pointer: no render target view of the back buffer"));
        return;
    }
    let outline = (radius * 0.3).clamp(1.5, 3.0);
    // color.w: the shape (1 disc, 2 crosshair)
    let constants: [f32; 8] = [x, y, radius, outline, color[0], color[1], color[2], if crosshair { 2.0 } else { 1.0 }];
    let mut previous: P = std::ptr::null_mut();
    com!(r.context1, 131, fn(P, *mut P) -> (), r.state, &mut previous);
    com!(context, 48, fn(P, u32, P, *const f32, u32, u32) -> (), r.constants, 0, std::ptr::null_mut(), constants.as_ptr(), 0, 0);
    com!(context, 33, fn(u32, *const P, P) -> (), 1, &rtv, std::ptr::null_mut());
    // only the dot's square drawn
    let side = radius * 2.0 + 2.0 * outline + 2.0;
    let viewport = [x - side / 2.0, y - side / 2.0, side, side, 0.0, 1.0];
    com!(context, 44, fn(u32, *const f32) -> (), 1, viewport.as_ptr());
    com!(context, 35, fn(P, *const f32, u32) -> (), r.blend, std::ptr::null(), 0xFFFF_FFFF);
    com!(context, 24, fn(u32) -> (), 4); // TRIANGLELIST
    com!(context, 11, fn(P, P, u32) -> (), r.vs, std::ptr::null_mut(), 0);
    com!(context, 9, fn(P, P, u32) -> (), r.ps, std::ptr::null_mut(), 0);
    com!(context, 16, fn(u32, u32, *const P) -> (), 0, 1, &r.constants);
    com!(context, 13, fn(u32, u32) -> (), 3, 0);
    com!(context, 33, fn(u32, *const P, P) -> (), 0, std::ptr::null(), std::ptr::null_mut());
    com!(r.context1, 131, fn(P, *mut P) -> (), previous, std::ptr::null_mut());
    release(previous);
    release(rtv);
}
