//! `WAL_KONAMI_CROSSHAIRS=1`: each player's aim drawn over the picture (Gashaaaan Refill! and
//! Gashaaaan: the cabinet's players see where their real balls go, here nothing shows where a
//! throw lands).
//!
//! - `WAL_KONAMI_CROSSHAIR_STYLE`: the built-in shape, `ring-cross` (default), `cross`, `ring`,
//!   `dot`, `disc` (a filled circle of the crosshair's size), white with a black outline, in the player's color (P1 blue, P2 red, P3 green, P4
//!   yellow).
//! - `WAL_KONAMI_CROSSHAIR_P1` ... `_P4`: a PNG of the player's own instead (a Linux path, or a
//!   Windows one). A dark icon (a silhouette, as icon sites ship them) is drawn in the player's
//!   color, a colored one with the player's color applied (`WAL_KONAMI_CROSSHAIR_TINT=0`: as is).
//! - `WAL_KONAMI_CROSSHAIR_SIZE`: its size in percent of the screen height (6).
//! - `WAL_KONAMI_CROSSHAIR_COLOR`: `RRGGBB`, every player's color instead of theirs.
//! - `WAL_KONAMI_CROSSHAIR_AXES`: what it follows, `stick` (the left stick, the guns' aim:
//!   default) or `pedals` (`accel` / `brake`, 0..32767 across the screen: the touch pointer of
//!   Steel Chronicle Victroopers, whose sticks steer).
//! - `WAL_KONAMI_CROSSHAIR_HIDE`: seconds without moving before it hides (5; 0: never).
//!
//! Before each `Present` (payload-common's Direct3D 9 overlay) the crosshairs are drawn into the
//! game's back buffer as alpha-blended quads, the game's render states, render target and depth
//! buffer saved around them, while that player's gun moved in the last seconds. The aim spans the
//! whole back buffer, as the throws do.
//!
//! Gashaaaan (OpenGL) draws them itself before its swaps ([`draw_gl`]: textured quads of the
//! fixed-function pipeline, the game's attributes and matrices pushed around them).

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::{log, screenshot};
use wal_protocol::{Axis, MAX_PLAYERS};

type P = *mut c_void;

/// A crosshair is hidden when its gun did not move for this long (`WAL_KONAMI_CROSSHAIR_HIDE`;
/// `None`: never).
fn idle() -> Option<Duration> {
    static IDLE: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *IDLE.get_or_init(|| {
        let secs = std::env::var("WAL_KONAMI_CROSSHAIR_HIDE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(5.0);
        (secs > 0.0).then(|| Duration::from_secs_f32(secs))
    })
}

/// The aim follows `accel` / `brake` (`WAL_KONAMI_CROSSHAIR_AXES=pedals`).
fn pedals() -> bool {
    static PEDALS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PEDALS.get_or_init(|| std::env::var("WAL_KONAMI_CROSSHAIR_AXES").is_ok_and(|v| v.trim() == "pedals"))
}

/// `WAL_KONAMI_CROSSHAIR_COLOR` (`RRGGBB`) as B, G, R.
fn color_override() -> Option<[u8; 3]> {
    let v = std::env::var("WAL_KONAMI_CROSSHAIR_COLOR").ok()?;
    let v = v.trim().trim_start_matches('#');
    let n = u32::from_str_radix(v, 16).ok().filter(|_| v.len() == 6)?;
    Some([n as u8, (n >> 8) as u8, (n >> 16) as u8])
}
/// Player colors (B, G, R).
const COLORS: [[u8; 3]; MAX_PLAYERS] = [[0xff, 0x80, 0x20], [0x30, 0x30, 0xff], [0x40, 0xe0, 0x30], [0x20, 0xe0, 0xff]];
/// Built-in shapes are drawn at this size.
const SHAPE: usize = 128;

const DEV_GET_BACK_BUFFER: usize = 18;
const DEV_CREATE_TEXTURE: usize = 23;
const DEV_SET_RENDER_TARGET: usize = 37;
const DEV_GET_RENDER_TARGET: usize = 38;
const DEV_SET_DEPTH_STENCIL: usize = 39;
const DEV_GET_DEPTH_STENCIL: usize = 40;
const DEV_BEGIN_SCENE: usize = 41;
const DEV_END_SCENE: usize = 42;
const DEV_SET_VIEWPORT: usize = 47;
const DEV_SET_RENDER_STATE: usize = 57;
const DEV_CREATE_STATE_BLOCK: usize = 59;
const DEV_SET_TEXTURE: usize = 65;
const DEV_SET_TEXTURE_STAGE_STATE: usize = 67;
const DEV_SET_SAMPLER_STATE: usize = 69;
const DEV_DRAW_PRIMITIVE_UP: usize = 83;
const DEV_SET_FVF: usize = 89;
const DEV_SET_VERTEX_SHADER: usize = 92;
const DEV_SET_PIXEL_SHADER: usize = 107;
const SURF_GET_DESC: usize = 12;
const TEX_LOCK_RECT: usize = 19;
const TEX_UNLOCK_RECT: usize = 20;
const SB_CAPTURE: usize = 4;
const SB_APPLY: usize = 5;
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DPOOL_MANAGED: u32 = 1;
const D3DSBT_ALL: u32 = 1;
const D3DTEXF_LINEAR: u32 = 2;
const D3DFVF_XYZRHW_TEX1: u32 = 0x104;
const D3DPT_TRIANGLESTRIP: u32 = 5;

/// A picture, BGRA.
struct Image {
    w: u32,
    h: u32,
    bgra: Vec<u8>,
}

struct State {
    images: Vec<Image>,
    /// Height of a crosshair, fraction of the screen's.
    size: f32,
    /// Each player's last aim and when it moved.
    aims: [((i16, i16), Option<Instant>); MAX_PLAYERS],
    res: Option<Res>,
    /// The players' OpenGL textures.
    gl_textures: Vec<u32>,
}

/// Device resources: the players' textures and the state block saving the game's states.
struct Res {
    device: usize,
    textures: Vec<P>,
    states: P,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn init() {
    if load() {
        screenshot::set_overlay(draw, reset);
    }
}

/// Gashaaaan (OpenGL): the crosshairs drawn by [`draw_gl`]. Whether they are on.
pub fn init_gl() -> bool {
    load()
}

/// The options read, the players' pictures made. Whether the crosshairs are on.
fn load() -> bool {
    if !std::env::var("WAL_KONAMI_CROSSHAIRS").is_ok_and(|v| v == "1") {
        return false;
    }
    let style = std::env::var("WAL_KONAMI_CROSSHAIR_STYLE").unwrap_or_default();
    let tint = !std::env::var("WAL_KONAMI_CROSSHAIR_TINT").is_ok_and(|v| v == "0");
    let size = std::env::var("WAL_KONAMI_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(6.0);
    let custom_color = color_override();
    let images = (0..MAX_PLAYERS)
        .map(|player| {
            let color = custom_color.unwrap_or(COLORS[player]);
            let custom = std::env::var(format!("WAL_KONAMI_CROSSHAIR_P{}", player + 1)).ok().filter(|p| !p.trim().is_empty());
            let image = custom.and_then(|path| match read_png(&windows_path(path.trim())) {
                Ok(image) => {
                    log!("crosshair: P{} {path}", player + 1);
                    Some(if tint { colorize(image, color) } else { image })
                }
                Err(e) => {
                    log!("crosshair: P{} {path}: {e}, built-in crosshair", player + 1);
                    None
                }
            });
            image.unwrap_or_else(|| multiply(shape(&style), color))
        })
        .collect();
    *STATE.lock().unwrap() = Some(State {
        images,
        size: (size / 100.0).clamp(0.01, 0.5),
        aims: [((0, 0), None); MAX_PLAYERS],
        res: None,
        gl_textures: Vec::new(),
    });
    log!("crosshair: players' aim drawn over the picture (style {}, {size}% high)", if style.is_empty() { "ring-cross" } else { &style });
    true
}

/// `/home/...` is `Z:\home\...` to the game.
fn windows_path(path: &str) -> String {
    if path.starts_with('/') { format!("Z:{}", path.replace('/', "\\")) } else { path.to_string() }
}

/// A built-in shape: white, black outline, transparent around.
fn shape(style: &str) -> Image {
    let c = SHAPE as f32 / 2.0;
    let (ring, cross, dot) = match style {
        "cross" => (false, true, true),
        "ring" => (true, false, true),
        "dot" | "disc" => (false, false, true),
        _ => (true, true, true),
    };
    // the dot's radius: `disc` fills the picture (a pointer dot of the picture's size)
    let dot_radius = if style == "disc" { c - 6.0 } else { 7.0 };
    // distance from the shape's white strokes, in pixels (negative inside)
    let stroke = |x: f32, y: f32| -> f32 {
        let (dx, dy) = (x - c, y - c);
        let r = (dx * dx + dy * dy).sqrt();
        let mut d = f32::MAX;
        if ring {
            d = d.min((r - c * 0.62).abs() - 6.0);
        }
        if cross {
            // four arms from the gap to the edge
            let (gap, end) = (c * 0.28, c * 0.92);
            let (mid, half) = ((gap + end) / 2.0, (end - gap) / 2.0);
            for (ax, ay, hx, hy) in [(mid, 0.0, half, 6.0), (-mid, 0.0, half, 6.0), (0.0, mid, 6.0, half), (0.0, -mid, 6.0, half)] {
                d = d.min(boxed(dx - ax, dy - ay, hx, hy));
            }
        }
        if dot {
            d = d.min(r - dot_radius);
        }
        d
    };
    let mut bgra = vec![0u8; SHAPE * SHAPE * 4];
    for y in 0..SHAPE {
        for x in 0..SHAPE {
            let d = stroke(x as f32 + 0.5, y as f32 + 0.5);
            // white stroke, 4.5 px black outline, antialiased edges
            let white = (0.5 - d).clamp(0.0, 1.0);
            let alpha = (5.0 - d).clamp(0.0, 1.0);
            let v = (white * 255.0) as u8;
            bgra[(y * SHAPE + x) * 4..][..4].copy_from_slice(&[v, v, v, (alpha * 255.0) as u8]);
        }
    }
    Image { w: SHAPE as u32, h: SHAPE as u32, bgra }
}

/// Signed distance from a box of half sizes `hx`, `hy` centered on the origin.
fn boxed(x: f32, y: f32, hx: f32, hy: f32) -> f32 {
    let (qx, qy) = (x.abs() - hx, y.abs() - hy);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0)
}

/// The player's color applied (white becomes the color, black stays black).
fn multiply(mut image: Image, color: [u8; 3]) -> Image {
    for p in image.bgra.chunks_exact_mut(4) {
        for i in 0..3 {
            p[i] = (p[i] as u16 * color[i] as u16 / 255) as u8;
        }
    }
    image
}

/// A dark icon (a silhouette) drawn in the player's color, a lighter one multiplied by it.
fn colorize(mut image: Image, color: [u8; 3]) -> Image {
    let (sum, count) = image.bgra.chunks_exact(4).filter(|p| p[3] > 128).fold((0u64, 0u64), |(s, n), p| {
        (s + (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3, n + 1)
    });
    if count > 0 && sum / count < 80 {
        for p in image.bgra.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&color);
        }
        image
    } else {
        multiply(image, color)
    }
}

fn read_png(path: &str) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let data = &buf[..info.buffer_size()];
    let bgra: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[2], p[1], p[0], 0xFF]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        other => return Err(format!("color type {other:?}")),
    };
    Ok(Image { w: info.width, h: info.height, bgra })
}

unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, 2)) };
        unsafe { f(obj) };
    }
}

/// Device resources go before a reset (the state block); the textures are managed but follow.
unsafe fn reset(_dev: P) {
    if let Some(state) = STATE.lock().unwrap().as_mut()
        && let Some(res) = state.res.take()
    {
        for t in res.textures {
            unsafe { release(t) };
        }
        unsafe { release(res.states) };
    }
}

/// A managed A8R8G8B8 texture of `image`.
unsafe fn texture(dev: P, image: &Image) -> P {
    #[repr(C)]
    struct Locked {
        pitch: i32,
        bits: *mut u8,
    }
    unsafe {
        let create: unsafe extern "system" fn(P, u32, u32, u32, u32, u32, u32, *mut P, P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_TEXTURE));
        let mut tex: P = std::ptr::null_mut();
        if create(dev, image.w, image.h, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &mut tex, std::ptr::null_mut()) < 0 || tex.is_null() {
            return std::ptr::null_mut();
        }
        let mut locked = Locked { pitch: 0, bits: std::ptr::null_mut() };
        let lock: unsafe extern "system" fn(P, u32, *mut Locked, P, u32) -> i32 = std::mem::transmute(method(tex, TEX_LOCK_RECT));
        if lock(tex, 0, &mut locked, std::ptr::null_mut(), 0) < 0 {
            release(tex);
            return std::ptr::null_mut();
        }
        let row = image.w as usize * 4;
        for y in 0..image.h as usize {
            std::ptr::copy_nonoverlapping(image.bgra[y * row..].as_ptr(), locked.bits.offset(y as isize * locked.pitch as isize), row);
        }
        let unlock: unsafe extern "system" fn(P, u32) -> i32 = std::mem::transmute(method(tex, TEX_UNLOCK_RECT));
        unlock(tex, 0);
        tex
    }
}

/// Fixed-function states of the quads: pre-transformed textured vertices, alpha blending, no
/// depth, linear sampling.
unsafe fn prepare(dev: P, w: u32, h: u32) {
    unsafe {
        let rs: unsafe extern "system" fn(P, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_RENDER_STATE));
        let tss: unsafe extern "system" fn(P, u32, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_TEXTURE_STAGE_STATE));
        let ss: unsafe extern "system" fn(P, u32, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_SAMPLER_STATE));
        let set_vs: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_VERTEX_SHADER));
        let set_ps: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_PIXEL_SHADER));
        let set_fvf: unsafe extern "system" fn(P, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_FVF));
        let set_viewport: unsafe extern "system" fn(P, *const u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_VIEWPORT));
        let viewport: [u32; 6] = [0, 0, w, h, 0, 1.0f32.to_bits()];
        set_viewport(dev, viewport.as_ptr());
        set_vs(dev, std::ptr::null_mut());
        set_ps(dev, std::ptr::null_mut());
        set_fvf(dev, D3DFVF_XYZRHW_TEX1);
        // ZENABLE, FILLMODE solid, ZWRITEENABLE, ALPHATESTENABLE, SRCBLEND srcalpha, DESTBLEND
        // invsrcalpha, CULLMODE none, ALPHABLENDENABLE, FOGENABLE, STENCILENABLE, CLIPPING,
        // LIGHTING, COLORWRITEENABLE, BLENDOP add, SCISSORTESTENABLE, SRGBWRITEENABLE,
        // SEPARATEALPHABLENDENABLE
        for (state, value) in [(7, 0), (8, 3), (14, 0), (15, 0), (19, 5), (20, 6), (22, 1), (27, 1), (28, 0), (52, 0), (136, 1), (137, 0), (168, 0xF), (171, 1), (174, 0), (194, 0), (206, 0)] {
            rs(dev, state, value);
        }
        // stage 0: texture color and alpha; stage 1 off
        for (state, value) in [(1, 2), (2, 2), (4, 2), (5, 2)] {
            tss(dev, 0, state, value);
        }
        tss(dev, 1, 1, 1);
        tss(dev, 1, 4, 1);
        // clamp, linear, no mipmaps
        for (state, value) in [(1, 3), (2, 3), (5, D3DTEXF_LINEAR), (6, D3DTEXF_LINEAR), (7, 0)] {
            ss(dev, 0, state, value);
        }
    }
}

/// Draws `tex` over `x, y, w, h`.
unsafe fn quad(dev: P, tex: P, (x, y, w, h): (f32, f32, f32, f32)) {
    let (x0, y0, x1, y1) = (x - 0.5, y - 0.5, x + w - 0.5, y + h - 0.5);
    let v: [f32; 24] = [
        x0, y0, 0.0, 1.0, 0.0, 0.0, //
        x1, y0, 0.0, 1.0, 1.0, 0.0, //
        x0, y1, 0.0, 1.0, 0.0, 1.0, //
        x1, y1, 0.0, 1.0, 1.0, 1.0,
    ];
    unsafe {
        let set_texture: unsafe extern "system" fn(P, u32, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_TEXTURE));
        let draw_up: unsafe extern "system" fn(P, u32, u32, *const f32, u32) -> i32 = std::mem::transmute(method(dev, DEV_DRAW_PRIMITIVE_UP));
        set_texture(dev, 0, tex);
        draw_up(dev, D3DPT_TRIANGLESTRIP, 2, v.as_ptr(), 24);
    }
}

/// Who aims (a gun that moved lately) and where.
fn aims(state: &mut State) -> [Option<(i16, i16)>; MAX_PLAYERS] {
    let now = Instant::now();
    let mut shown = [None; MAX_PLAYERS];
    for (player, (last, moved)) in state.aims.iter_mut().enumerate() {
        let stick = wal_payload_common::input(player);
        let aim = if pedals() {
            // 0..32767 to the sticks' -32768..32767
            let full = |v: i16| (v.max(0) as i32 * 2 - 32768).clamp(-32768, 32767) as i16;
            (full(stick.axis(Axis::Accel)), full(stick.axis(Axis::Brake)))
        } else {
            (stick.axis(Axis::LeftX), stick.axis(Axis::LeftY))
        };
        if aim != *last {
            *last = aim;
            *moved = Some(now);
        }
        if moved.is_some_and(|m| idle().is_none_or(|idle| now.duration_since(m) < idle)) {
            shown[player] = Some(aim);
        }
    }
    shown
}

unsafe fn draw(dev: P, _window: P) -> Option<P> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.as_mut()?;
    let shown = aims(state);
    if shown.iter().all(Option::is_none) {
        return None;
    }
    unsafe {
        if state.res.as_ref().is_some_and(|r| r.device != dev as usize) {
            drop(guard);
            reset(dev);
            guard = STATE.lock().unwrap();
        }
        let state = guard.as_mut()?;
        if state.res.is_none() {
            let create_sb: unsafe extern "system" fn(P, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_STATE_BLOCK));
            let mut states: P = std::ptr::null_mut();
            if create_sb(dev, D3DSBT_ALL, &mut states) < 0 || states.is_null() {
                log!("crosshair: no state block, crosshairs disabled");
                screenshot::clear_overlay();
                return None;
            }
            let textures = state.images.iter().map(|i| texture(dev, i)).collect();
            state.res = Some(Res { device: dev as usize, textures, states });
        }
        let res = state.res.as_ref().unwrap();

        let get_back_buffer: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_BACK_BUFFER));
        let mut back: P = std::ptr::null_mut();
        if get_back_buffer(dev, 0, 0, 0, &mut back) < 0 || back.is_null() {
            return None;
        }
        // D3DSURFACE_DESC: Format, Type, Usage, Pool, MultiSampleType, MultiSampleQuality, Width, Height
        let mut desc = [0u32; 8];
        let get_desc: unsafe extern "system" fn(P, *mut u32) -> i32 = std::mem::transmute(method(back, SURF_GET_DESC));
        get_desc(back, desc.as_mut_ptr());
        let (w, h) = (desc[6], desc[7]);

        let capture: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(res.states, SB_CAPTURE));
        let apply: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(res.states, SB_APPLY));
        capture(res.states);
        let get_rt: unsafe extern "system" fn(P, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_RENDER_TARGET));
        let set_rt: unsafe extern "system" fn(P, u32, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_RENDER_TARGET));
        let get_ds: unsafe extern "system" fn(P, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_DEPTH_STENCIL));
        let set_ds: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_DEPTH_STENCIL));
        let (mut old_rt, mut old_ds): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
        get_rt(dev, 0, &mut old_rt);
        get_ds(dev, &mut old_ds);
        set_rt(dev, 0, back);
        set_ds(dev, std::ptr::null_mut());
        prepare(dev, w, h);
        let begin: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_BEGIN_SCENE));
        let end: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_END_SCENE));
        begin(dev);
        let ch = state.size * h as f32;
        for (player, aim) in shown.iter().enumerate() {
            let (Some(aim), Some(&tex)) = (aim, res.textures.get(player)) else { continue };
            if tex.is_null() {
                continue;
            }
            let image = &state.images[player];
            let cw = ch * image.w as f32 / image.h as f32;
            let x = (aim.0 as f32 + 32768.0) / 65535.0 * w as f32;
            let y = (aim.1 as f32 + 32768.0) / 65535.0 * h as f32;
            quad(dev, tex, (x - cw / 2.0, y - ch / 2.0, cw, ch));
        }
        end(dev);
        set_rt(dev, 0, old_rt);
        set_ds(dev, old_ds);
        release(old_rt);
        release(old_ds);
        apply(res.states);
        release(back);
    }
    None
}

/// The legacy OpenGL functions of the crosshairs (the game's compatibility context).
struct Gl {
    gen_textures: unsafe extern "system" fn(i32, *mut u32),
    bind_texture: unsafe extern "system" fn(u32, u32),
    tex_image_2d: unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void),
    tex_parameteri: unsafe extern "system" fn(u32, u32, i32),
    tex_envi: unsafe extern "system" fn(u32, u32, i32),
    enable: unsafe extern "system" fn(u32),
    disable: unsafe extern "system" fn(u32),
    blend_func: unsafe extern "system" fn(u32, u32),
    begin: unsafe extern "system" fn(u32),
    end: unsafe extern "system" fn(),
    tex_coord_2f: unsafe extern "system" fn(f32, f32),
    vertex_2f: unsafe extern "system" fn(f32, f32),
    color_4f: unsafe extern "system" fn(f32, f32, f32, f32),
    matrix_mode: unsafe extern "system" fn(u32),
    push_matrix: unsafe extern "system" fn(),
    pop_matrix: unsafe extern "system" fn(),
    load_identity: unsafe extern "system" fn(),
    ortho: unsafe extern "system" fn(f64, f64, f64, f64, f64, f64),
    push_attrib: unsafe extern "system" fn(u32),
    pop_attrib: unsafe extern "system" fn(),
    viewport: unsafe extern "system" fn(i32, i32, i32, i32),
    get_integerv: unsafe extern "system" fn(u32, *mut i32),
    active_texture: Option<unsafe extern "system" fn(u32)>,
    use_program: Option<unsafe extern "system" fn(u32)>,
}

fn gl() -> Option<&'static Gl> {
    static GL: std::sync::OnceLock<Option<Gl>> = std::sync::OnceLock::new();
    GL.get_or_init(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        let module = GetModuleHandleA(c"opengl32.dll".as_ptr().cast());
        let wgl: unsafe extern "system" fn(*const u8) -> usize = std::mem::transmute(GetProcAddress(module, c"wglGetProcAddress".as_ptr().cast())?);
        let ext = |name: &std::ffi::CStr| -> Option<usize> { Some(wgl(name.as_ptr().cast())).filter(|&f| f > 3) };
        let core = |name: &std::ffi::CStr| -> Option<usize> { GetProcAddress(module, name.as_ptr().cast()).map(|f| f as usize) };
        Some(Gl {
            gen_textures: std::mem::transmute(core(c"glGenTextures")?),
            bind_texture: std::mem::transmute(core(c"glBindTexture")?),
            tex_image_2d: std::mem::transmute(core(c"glTexImage2D")?),
            tex_parameteri: std::mem::transmute(core(c"glTexParameteri")?),
            tex_envi: std::mem::transmute(core(c"glTexEnvi")?),
            enable: std::mem::transmute(core(c"glEnable")?),
            disable: std::mem::transmute(core(c"glDisable")?),
            blend_func: std::mem::transmute(core(c"glBlendFunc")?),
            begin: std::mem::transmute(core(c"glBegin")?),
            end: std::mem::transmute(core(c"glEnd")?),
            tex_coord_2f: std::mem::transmute(core(c"glTexCoord2f")?),
            vertex_2f: std::mem::transmute(core(c"glVertex2f")?),
            color_4f: std::mem::transmute(core(c"glColor4f")?),
            matrix_mode: std::mem::transmute(core(c"glMatrixMode")?),
            push_matrix: std::mem::transmute(core(c"glPushMatrix")?),
            pop_matrix: std::mem::transmute(core(c"glPopMatrix")?),
            load_identity: std::mem::transmute(core(c"glLoadIdentity")?),
            ortho: std::mem::transmute(core(c"glOrtho")?),
            push_attrib: std::mem::transmute(core(c"glPushAttrib")?),
            pop_attrib: std::mem::transmute(core(c"glPopAttrib")?),
            viewport: std::mem::transmute(core(c"glViewport")?),
            get_integerv: std::mem::transmute(core(c"glGetIntegerv")?),
            active_texture: ext(c"glActiveTexture").map(|f| std::mem::transmute(f)),
            use_program: ext(c"glUseProgram").map(|f| std::mem::transmute(f)),
        })
    })
    .as_ref()
}

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_BLEND: u32 = 0x0BE2;
const GL_QUADS: u32 = 0x0007;
const GL_ALL_ATTRIB_BITS: u32 = 0x000F_FFFF;
const GL_TEXTURE0: u32 = 0x84C0;

/// Gashaaaan: the crosshairs over the window's back buffer (`ww` x `wh`, the default
/// framebuffer bound), before its swap. The game's state saved around them.
pub unsafe fn draw_gl(ww: i32, wh: i32) {
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else { return };
    let shown = aims(state);
    if shown.iter().all(Option::is_none) {
        return;
    }
    let Some(gl) = gl() else {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("crosshair: OpenGL functions missing, no crosshairs"));
        return;
    };
    unsafe {
        let (mut active, mut program) = (GL_TEXTURE0 as i32, 0);
        (gl.get_integerv)(0x84E0, &mut active); // GL_ACTIVE_TEXTURE
        (gl.get_integerv)(0x8B8D, &mut program); // GL_CURRENT_PROGRAM
        (gl.push_attrib)(GL_ALL_ATTRIB_BITS);
        if let Some(active_texture) = gl.active_texture {
            // the game's other texture units off (pushed with the enable bits)
            for unit in 1..4 {
                active_texture(GL_TEXTURE0 + unit);
                (gl.disable)(GL_TEXTURE_2D);
            }
            active_texture(GL_TEXTURE0);
        }
        if let Some(use_program) = gl.use_program {
            use_program(0);
        }
        // fixed function: no shader programs (Cg's ARB / NV ones), depth, lighting, fog...
        for cap in [0x8620, 0x8804, 0x8870, 0x0B71, 0x0B50, 0x0B60, 0x0B44, 0x0BC0, 0x0C11, 0x0B90, 0x8513] {
            (gl.disable)(cap);
        }
        (gl.enable)(GL_TEXTURE_2D);
        (gl.enable)(GL_BLEND);
        (gl.blend_func)(0x0302, 0x0303); // SRC_ALPHA, ONE_MINUS_SRC_ALPHA
        (gl.tex_envi)(0x2300, 0x2200, 0x2100); // TEXTURE_ENV, TEXTURE_ENV_MODE, MODULATE
        (gl.color_4f)(1.0, 1.0, 1.0, 1.0);
        (gl.viewport)(0, 0, ww, wh);
        for mode in [0x1701, 0x1700, 0x1702] {
            // PROJECTION, MODELVIEW, TEXTURE
            (gl.matrix_mode)(mode);
            (gl.push_matrix)();
            (gl.load_identity)();
        }
        (gl.matrix_mode)(0x1701);
        (gl.ortho)(0.0, ww as f64, wh as f64, 0.0, -1.0, 1.0);
        if state.gl_textures.is_empty() {
            for image in &state.images {
                let mut tex = 0;
                (gl.gen_textures)(1, &mut tex);
                (gl.bind_texture)(GL_TEXTURE_2D, tex);
                (gl.tex_parameteri)(GL_TEXTURE_2D, 0x2801, 0x2601); // MIN_FILTER LINEAR
                (gl.tex_parameteri)(GL_TEXTURE_2D, 0x2800, 0x2601); // MAG_FILTER LINEAR
                (gl.tex_parameteri)(GL_TEXTURE_2D, 0x2802, 0x812F); // WRAP_S CLAMP_TO_EDGE
                (gl.tex_parameteri)(GL_TEXTURE_2D, 0x2803, 0x812F); // WRAP_T
                (gl.tex_parameteri)(GL_TEXTURE_2D, 0x813D, 0); // MAX_LEVEL 0
                // RGBA8 from BGRA bytes
                (gl.tex_image_2d)(GL_TEXTURE_2D, 0, 0x8058, image.w as i32, image.h as i32, 0, 0x80E1, 0x1401, image.bgra.as_ptr().cast());
                state.gl_textures.push(tex);
            }
        }
        let ch = state.size * wh as f32;
        for (player, aim) in shown.iter().enumerate() {
            let (Some(aim), Some(&tex)) = (aim, state.gl_textures.get(player)) else { continue };
            let image = &state.images[player];
            let cw = ch * image.w as f32 / image.h as f32;
            let x = (aim.0 as f32 + 32768.0) / 65535.0 * ww as f32 - cw / 2.0;
            let y = (aim.1 as f32 + 32768.0) / 65535.0 * wh as f32 - ch / 2.0;
            (gl.bind_texture)(GL_TEXTURE_2D, tex);
            (gl.begin)(GL_QUADS);
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                (gl.tex_coord_2f)(u, v);
                (gl.vertex_2f)(x + u * cw, y + v * ch);
            }
            (gl.end)();
        }
        for mode in [0x1702, 0x1700, 0x1701] {
            (gl.matrix_mode)(mode);
            (gl.pop_matrix)();
        }
        (gl.pop_attrib)();
        if let Some(use_program) = gl.use_program {
            use_program(program as u32);
        }
        if let Some(active_texture) = gl.active_texture {
            active_texture(active as u32);
        }
    }
}
