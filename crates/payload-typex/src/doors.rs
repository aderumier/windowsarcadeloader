//! Elevator Action Death Parade's attraction doors (`WAL_TYPEX_DOORS=<folder>`).
//!
//! The cabinet is an elevator: two physical doors in front of the screen close and open at
//! moments of the game (between stages, during some scenes) and shake. The game drives them
//! through its door board, calling two functions:
//! * doors(left, right, speed) at +0xC4B10: positions 0.0 open .. 1.0 closed, travel time
//!   4000 - 3000 x speed ms;
//! * vibration(level, power, time) at +0xC4C40: shake for `time` ms.
//!
//! Without the board both return at once: their entries are hooked (the values recorded, the
//! game's code then run) and the doors are drawn over the game's picture before each
//! `Present` (`screenshot::set_overlay`), with the dump's pictures (`<folder>` relative to the
//! game directory: `<W>x<H>/Landscape/` sets of the 16:9 screen sizes, the one closest to the
//! window height used): `DoorLeft.png` / `DoorRight.png` (each half of the picture wide; a
//! closed door covers its half, an open one is beside the picture, behind the wall), and with
//! `WAL_TYPEX_BEZEL` (default 1) the elevator wall `Bezel.png` and the door frame
//! `DoorsBezel.png` (full 16:9 screen pictures with the game's picture in their middle).
//!
//! The game keeps its 720x1280 back buffer (a larger one changed its layout: 3D centered on
//! the buffer, 2D in the corner). Each frame the screen is composed in an additional swap
//! chain of the screen's size, on a window of ours over the game's (topmost, never activated,
//! transparent to the mouse): the picture scaled (linear: the window scaling of the 720x1280
//! picture showed a grid) into its place, then the frame, the doors and the wall (alpha
//! blended); that swap chain is presented instead of the game's. The guns aim relative to the
//! picture (`guns::set_picture`).
//!
//! `WAL_CROSSHAIR=1` (the common crosshair option): each player's aim drawn over the screen
//! with the game directory's `P1.png` / `P2.png` (sized for a 1080-line screen, scaled with
//! it), while that player's gun moved in the last few seconds.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{log, screenshot};

use crate::guns;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

type P = *mut c_void;

/// Hooked function: RVA, its first instructions (copied to the trampoline, no relative ones).
const DOORS: (usize, &[u8]) = (0xC4B10, &[0x80, 0x3D, 0x3D, 0xF6, 0x5E, 0x00, 0x00]);
const VIBRATION: (usize, &[u8]) = (0xC4C40, &[0x80, 0x3D, 0x3E, 0xF6, 0x5E, 0x00, 0x00]);

static DOORS_TRAMP: AtomicUsize = AtomicUsize::new(0);
static VIBRATION_TRAMP: AtomicUsize = AtomicUsize::new(0);
/// Door targets (f32 bits, 0 open .. 1 closed) and travel time in ms.
static TARGET_LEFT: AtomicU32 = AtomicU32::new(0);
static TARGET_RIGHT: AtomicU32 = AtomicU32::new(0);
static TRAVEL_MS: AtomicU32 = AtomicU32::new(1000);
/// Vibration: (start, duration, amplitude).
static SHAKE: Mutex<Option<(Instant, Duration, f32)>> = Mutex::new(None);

pub(crate) fn init() {
    let Ok(folder) = std::env::var("WAL_TYPEX_DOORS") else { return };
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let mem = unsafe { VirtualAlloc(std::ptr::null(), 64, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
    if mem == 0 {
        return;
    }
    let ok = unsafe {
        hook(base, DOORS, doors as *const () as usize, mem, &DOORS_TRAMP)
            && hook(base, VIBRATION, vibration as *const () as usize, mem + 32, &VIBRATION_TRAMP)
    };
    if !ok {
        log!("doors: unknown game build (door functions not found), doors disabled");
        return;
    }
    let bezel = std::env::var("WAL_TYPEX_BEZEL").map_or(true, |v| v != "0");
    let crosshairs = if wal_payload_common::crosshairs(false) {
        ["P1.png", "P2.png"]
            .iter()
            .filter_map(|name| match read_png(std::path::Path::new(name)) {
                Ok(image) => Some(image),
                Err(e) => {
                    log!("doors: {name}: {e}");
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    log!("doors: {} crosshair(s)", crosshairs.len());
    *STATE.lock().unwrap() = Some(State { folder, bezel, pack: None, res: None, crosshairs, aims: [((0, 0), None); 2] });
    screenshot::set_overlay(draw, reset);
    log!("doors: door functions hooked");
}

/// Jumps from the function's entry to `replacement`; its first instructions + a jump back
/// form the trampoline at `tramp` (called to run the game's function).
unsafe fn hook(base: usize, (rva, prologue): (usize, &[u8]), replacement: usize, tramp: usize, slot: &AtomicUsize) -> bool {
    let at = base + rva;
    let code = unsafe { std::slice::from_raw_parts(at as *const u8, prologue.len()) };
    if code != prologue {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(prologue.as_ptr(), tramp as *mut u8, prologue.len());
        let jmp = tramp + prologue.len();
        *(jmp as *mut u8) = 0xE9;
        std::ptr::write_unaligned((jmp + 1) as *mut i32, (at + prologue.len()).wrapping_sub(jmp + 5) as i32);
        slot.store(tramp, Ordering::Relaxed);
        let mut old = 0;
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xE9;
        std::ptr::write_unaligned((at + 1) as *mut i32, replacement.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
    true
}

unsafe extern "C" fn doors(left: f32, right: f32, speed: f32) -> u32 {
    let travel = if speed <= 0.0 { 4000.0 } else if speed >= 1.0 { 1000.0 } else { 4000.0 - 3000.0 * speed };
    TARGET_LEFT.store(left.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    TARGET_RIGHT.store(right.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    TRAVEL_MS.store(travel as u32, Ordering::Relaxed);
    log!("doors: left {left:.2} right {right:.2} speed {speed:.2} ({travel:.0} ms)");
    let orig: unsafe extern "C" fn(f32, f32, f32) -> u32 = unsafe { std::mem::transmute(DOORS_TRAMP.load(Ordering::Relaxed)) };
    unsafe { orig(left, right, speed) }
}

unsafe extern "C" fn vibration(level: f32, power: i32, time: i32) -> u32 {
    // a 1/400 of the door width per power unit, both ways
    let amplitude = (power.max(0) as f32 / 400.0).min(0.25);
    *SHAKE.lock().unwrap() = Some((Instant::now(), Duration::from_millis(time.max(0) as u64), amplitude));
    log!("doors: vibration level {level:.2} power {power} time {time} ms");
    let orig: unsafe extern "C" fn(f32, i32, i32) -> u32 = unsafe { std::mem::transmute(VIBRATION_TRAMP.load(Ordering::Relaxed)) };
    unsafe { orig(level, power, time) }
}

/// Shake frequency (Hz).
const SHAKE_HZ: f32 = 4.0;
/// A crosshair is hidden when its gun did not move for this long.
const CROSSHAIR_IDLE: Duration = Duration::from_secs(5);

/// A picture, BGRA.
struct Image {
    w: u32,
    h: u32,
    bgra: Vec<u8>,
}

/// The pictures of one screen size.
struct Pack {
    /// Screen size of the set (the bezel's).
    size: (u32, u32),
    bezel: Option<Image>,
    frame: Option<Image>,
    left: Image,
    right: Image,
}

/// Device resources: textures (managed, kept across resets), the overlay window's swap chain
/// (default pool, released before resets) and the state block saving the game's states.
struct Resources {
    device: usize,
    size: (u32, u32),
    window: P,
    bezel: P,
    frame: P,
    left: P,
    right: P,
    chain: P,
    states: P,
    crosshairs: Vec<P>,
}

struct State {
    folder: String,
    bezel: bool,
    pack: Option<Pack>,
    res: Option<Resources>,
    /// Crosshair pictures of players 1, 2 (`WAL_CROSSHAIR`).
    crosshairs: Vec<Image>,
    /// Each player's last aim and when it last moved.
    aims: [((i16, i16), Option<Instant>); 2],
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Displayed door positions (0 open .. 1 closed) and when they were last moved.
struct Motion {
    left: f32,
    right: f32,
    last: Instant,
}

static MOTION: Mutex<Option<Motion>> = Mutex::new(None);

const RELEASE: usize = 2;
const DEV_CREATE_ADDITIONAL_SWAP_CHAIN: usize = 13;
const DEV_GET_BACK_BUFFER: usize = 18;
const DEV_CREATE_TEXTURE: usize = 23;
const DEV_STRETCH_RECT: usize = 34;
const DEV_COLOR_FILL: usize = 35;
const DEV_SET_RENDER_TARGET: usize = 37;
const DEV_GET_RENDER_TARGET: usize = 38;
const DEV_SET_DEPTH_STENCIL: usize = 39;
const DEV_GET_DEPTH_STENCIL: usize = 40;
const CHAIN_GET_BACK_BUFFER: usize = 5;
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
const SB_CAPTURE: usize = 4;
const SB_APPLY: usize = 5;
const SURF_GET_DESC: usize = 12;
const TEX_LOCK_RECT: usize = 19;
const TEX_UNLOCK_RECT: usize = 20;
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DPOOL_MANAGED: u32 = 1;
const D3DSBT_ALL: u32 = 1;
const D3DTEXF_LINEAR: u32 = 2;
const D3DFVF_XYZRHW_TEX1: u32 = 0x104;
const D3DPT_TRIANGLESTRIP: u32 = 5;

unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, RELEASE)) };
        unsafe { f(obj) };
    }
}

/// Moves `current` toward `target` by `step`.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target { (current + step).min(target) } else { (current - step).max(target) }
}

/// Door positions to draw now: the doors move at their travel speed, plus the vibration.
fn positions() -> (f32, f32) {
    let now = Instant::now();
    let (tl, tr) = (f32::from_bits(TARGET_LEFT.load(Ordering::Relaxed)), f32::from_bits(TARGET_RIGHT.load(Ordering::Relaxed)));
    let mut motion = MOTION.lock().unwrap();
    let m = motion.get_or_insert(Motion { left: tl, right: tr, last: now });
    let step = now.duration_since(m.last).as_secs_f32() * 1000.0 / TRAVEL_MS.load(Ordering::Relaxed).max(1) as f32;
    m.left = approach(m.left, tl, step);
    m.right = approach(m.right, tr, step);
    m.last = now;
    let (mut l, mut r) = (m.left, m.right);
    let mut shake = SHAKE.lock().unwrap();
    if let Some((start, duration, amplitude)) = *shake {
        let t = now.duration_since(start);
        if t >= duration {
            *shake = None;
        } else {
            // a door rattle: a sine of SHAKE_HZ
            let offset = amplitude * (t.as_secs_f32() * SHAKE_HZ * std::f32::consts::TAU).sin();
            l = (l + offset).clamp(0.0, 1.0);
            r = (r + offset).clamp(0.0, 1.0);
        }
    }
    (l, r)
}

/// Releases the default-pool resources before a device reset.
unsafe fn reset(_dev: P) {
    if let Some(res) = STATE.lock().unwrap().as_mut().and_then(|s| s.res.as_mut()) {
        unsafe {
            release(res.chain);
            release(res.states);
        }
        res.chain = std::ptr::null_mut();
        res.states = std::ptr::null_mut();
    }
}

/// Largest rectangle of aspect `aw`:`ah` centered in `w`x`h`: x, y, width, height.
fn fit(w: f32, h: f32, aw: f32, ah: f32) -> (f32, f32, f32, f32) {
    let (fw, fh) = if w * ah > h * aw { (h * aw / ah, h) } else { (w, w * ah / aw) };
    ((w - fw) / 2.0, (h - fh) / 2.0, fw, fh)
}

unsafe fn draw(dev: P, window: P) -> Option<P> {
    let mut state = STATE.lock().unwrap();
    let state = state.as_mut()?;
    let (left, right) = positions();
    let (rect, w, h) = screen_rect(window)?;
    if state.pack.is_none() {
        match load_pack(&state.folder, state.bezel, h) {
            Ok(pack) => state.pack = Some(pack),
            Err(e) => {
                log!("doors: {e}, doors disabled");
                screenshot::clear_overlay();
                return None;
            }
        }
    }
    let pack = state.pack.as_ref().unwrap();
    unsafe {
        let get_back_buffer: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_BACK_BUFFER));
        let mut back: P = std::ptr::null_mut();
        if get_back_buffer(dev, 0, 0, 0, &mut back) < 0 || back.is_null() {
            return None;
        }
        let mut desc = [0u32; 8];
        let get_desc: unsafe extern "system" fn(P, *mut u32) -> i32 = std::mem::transmute(method(back, SURF_GET_DESC));
        get_desc(back, desc.as_mut_ptr());
        let (format, bw, bh) = (desc[0], desc[6], desc[7]);
        let (fw, fh) = (w as f32, h as f32);
        // the bezel's screen, the picture in its middle (the doors' width, its height)
        let (sw, sh) = (pack.size.0 as f32, pack.size.1 as f32);
        let (scr_x, scr_y, scr_w, scr_h) = fit(fw, fh, sw, sh);
        let k = scr_h / sh;
        let pic_w = if pack.bezel.is_some() { 2.0 * pack.left.w as f32 * k } else { scr_h * bw as f32 / bh as f32 };
        let (pic_x, pic_y, pic_h) = ((fw - pic_w) / 2.0, scr_y, scr_h);
        let picture = [pic_x.round() as i32, pic_y.round() as i32, (pic_x + pic_w).round() as i32, (pic_y + pic_h).round() as i32];
        guns::set_picture(pic_x / fw, (pic_x + pic_w) / fw, pic_y / fh, (pic_y + pic_h) / fh);

        let stale = state.res.as_ref().is_some_and(|r| r.device != dev as usize || r.size != (w, h));
        if stale {
            let r = state.res.take().unwrap();
            for t in [r.bezel, r.frame, r.left, r.right, r.chain, r.states].into_iter().chain(r.crosshairs) {
                release(t);
            }
        }
        if state.res.is_none() {
            state.res = Some(Resources {
                device: dev as usize,
                size: (w, h),
                window: overlay_window(),
                bezel: pack.bezel.as_ref().map_or(std::ptr::null_mut(), |i| texture(dev, i)),
                frame: pack.frame.as_ref().map_or(std::ptr::null_mut(), |i| texture(dev, i)),
                left: texture(dev, &pack.left),
                right: texture(dev, &pack.right),
                chain: std::ptr::null_mut(),
                states: std::ptr::null_mut(),
                crosshairs: state.crosshairs.iter().map(|i| texture(dev, i)).collect(),
            });
        }
        let res = state.res.as_mut().unwrap();
        if res.window.is_null() {
            res.window = overlay_window();
        }
        place_window(res.window, rect);
        if res.chain.is_null() && !res.window.is_null() {
            // D3DPRESENT_PARAMETERS: the screen size, discard, the overlay window, windowed,
            // no depth, presentation interval default (vsync)
            let mut params: [u32; 14] = [w, h, format, 1, 0, 0, 1, res.window as u32, 1, 0, 0, 0, 0, 0];
            let create: unsafe extern "system" fn(P, *mut u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_ADDITIONAL_SWAP_CHAIN));
            let hr = create(dev, params.as_mut_ptr(), &mut res.chain);
            log!("doors: swap chain {w}x{h} on the overlay window: {hr:#x}");
            // the game's window is no longer presented: hidden (Xwayland keeps it above
            // ours despite topmost)
            if hr >= 0 {
                hide_window(window);
            }
        }
        if res.states.is_null() {
            let create: unsafe extern "system" fn(P, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_STATE_BLOCK));
            create(dev, D3DSBT_ALL, &mut res.states);
        }
        if res.chain.is_null() || res.states.is_null() || res.left.is_null() || res.right.is_null() {
            release(back);
            log!("doors: Direct3D resources not created, doors disabled");
            hide_window(res.window);
            screenshot::clear_overlay();
            return None;
        }
        let get_chain_back: unsafe extern "system" fn(P, u32, u32, *mut P) -> i32 = std::mem::transmute(method(res.chain, CHAIN_GET_BACK_BUFFER));
        let mut target: P = std::ptr::null_mut();
        if get_chain_back(res.chain, 0, 0, &mut target) < 0 || target.is_null() {
            release(back);
            return None;
        }

        // the game's picture scaled into its place
        let stretch: unsafe extern "system" fn(P, P, *const i32, P, *const i32, u32) -> i32 = std::mem::transmute(method(dev, DEV_STRETCH_RECT));
        let fill: unsafe extern "system" fn(P, P, *const i32, u32) -> i32 = std::mem::transmute(method(dev, DEV_COLOR_FILL));
        fill(dev, target, std::ptr::null(), 0xFF00_0000);
        stretch(dev, back, std::ptr::null(), target, picture.as_ptr(), D3DTEXF_LINEAR);
        release(back);

        // frame, doors, wall: alpha blended quads, the game's states saved around them
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
        set_rt(dev, 0, target);
        // the game's depth buffer is smaller than the screen: none
        set_ds(dev, std::ptr::null_mut());
        prepare(dev, w, h);
        let begin: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_BEGIN_SCENE));
        let end: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_END_SCENE));
        begin(dev);
        let whole = (scr_x, scr_y, scr_w, scr_h);
        if !res.frame.is_null() {
            quad(dev, res.frame, whole, (0.0, 1.0));
        }
        // a door: half the picture wide; open, it is beside the picture (clipped)
        let dw = pic_w / 2.0;
        door(dev, res.left, pic_x - dw + left * dw, dw, pic_x, pic_x + pic_w, pic_y, pic_h);
        door(dev, res.right, pic_x + pic_w - right * dw, dw, pic_x, pic_x + pic_w, pic_y, pic_h);
        if !res.bezel.is_null() {
            quad(dev, res.bezel, whole, (0.0, 1.0));
        }
        // crosshairs of the players aiming (their gun moved lately), over everything
        let now = Instant::now();
        for (player, (&tex, image)) in res.crosshairs.iter().zip(&state.crosshairs).enumerate() {
            let stick = wal_payload_common::input(player);
            let aim = (stick.axis(wal_protocol::Axis::LeftX), stick.axis(wal_protocol::Axis::LeftY));
            let (last, moved) = &mut state.aims[player];
            if (aim.0 as i32 - last.0 as i32).abs() > 64 || (aim.1 as i32 - last.1 as i32).abs() > 64 {
                *last = aim;
                *moved = Some(now);
            }
            if tex.is_null() || moved.is_none_or(|t| now.duration_since(t) > CROSSHAIR_IDLE) {
                continue;
            }
            let (x, y) = ((aim.0 as f32 + 32768.0) / 65535.0 * fw, (aim.1 as f32 + 32768.0) / 65535.0 * fh);
            let (cw, ch) = (image.w as f32 * fh / 1080.0, image.h as f32 * fh / 1080.0);
            quad(dev, tex, (x - cw / 2.0, y - ch / 2.0, cw, ch), (0.0, 1.0));
        }
        end(dev);
        set_rt(dev, 0, old_rt);
        set_ds(dev, old_ds);
        release(old_rt);
        release(old_ds);
        apply(res.states);
        release(target);
        pump(res.window);
        Some(res.chain)
    }
}

/// Screen rectangle of the game window's client area, its width and height.
fn screen_rect(window: P) -> Option<([i32; 4], u32, u32)> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if window.is_null() || unsafe { GetClientRect(window, &mut r) } == 0 || r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    unsafe { ClientToScreen(window, &mut origin) };
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    Some(([origin.x, origin.y, w, h], w as u32, h as u32))
}

unsafe extern "system" fn overlay_proc(hwnd: P, msg: u32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, HTTRANSPARENT, MA_NOACTIVATE, WM_MOUSEACTIVATE, WM_NCHITTEST};
    match msg {
        // clicks go to the game's window below
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The window showing the composed screen over the game's: topmost, never activated, not in
/// the taskbar, transparent to the mouse.
fn overlay_window() -> P {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, RegisterClassW, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    let class: Vec<u16> = "WalScreenOverlay\0".encode_utf16().collect();
    let instance = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(overlay_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    log!("doors: overlay window {hwnd:?}");
    hwnd
}

/// Keeps the overlay window shown over the game window's client area.
fn place_window(hwnd: P, [x, y, w, h]: [i32; 4]) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, HWND_TOPMOST, IsWindowVisible, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos};
    if hwnd.is_null() {
        return;
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(hwnd, &mut r) };
    if unsafe { IsWindowVisible(hwnd) } != 0 && [r.left, r.top, r.right - r.left, r.bottom - r.top] == [x, y, w, h] {
        return;
    }
    unsafe { SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
}

fn hide_window(hwnd: P) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    if !hwnd.is_null() {
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// Handles the overlay window's messages (it belongs to the game's render thread, which has no
/// message loop).
fn pump(hwnd: P) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW};
    if hwnd.is_null() {
        return;
    }
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) } != 0 {
        unsafe { DispatchMessageW(&msg) };
    }
}

/// A door's quad at `x` (`width` wide), clipped to `min_x`..`max_x`.
#[allow(clippy::too_many_arguments)]
unsafe fn door(dev: P, tex: P, x: f32, width: f32, min_x: f32, max_x: f32, y: f32, height: f32) {
    let (x0, x1) = (x.max(min_x), (x + width).min(max_x));
    if x1 <= x0 {
        return;
    }
    let u = ((x0 - x) / width, (x1 - x) / width);
    unsafe { quad(dev, tex, (x0, y, x1 - x0, height), u) };
}

/// Draws `tex` over `x, y, w, h` (texture columns `u.0`..`u.1`).
unsafe fn quad(dev: P, tex: P, (x, y, w, h): (f32, f32, f32, f32), u: (f32, f32)) {
    let (x0, y0, x1, y1) = (x - 0.5, y - 0.5, x + w - 0.5, y + h - 0.5);
    let v: [f32; 24] = [
        x0, y0, 0.0, 1.0, u.0, 0.0, //
        x1, y0, 0.0, 1.0, u.1, 0.0, //
        x0, y1, 0.0, 1.0, u.0, 1.0, //
        x1, y1, 0.0, 1.0, u.1, 1.0,
    ];
    unsafe {
        let set_texture: unsafe extern "system" fn(P, u32, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_TEXTURE));
        let draw_up: unsafe extern "system" fn(P, u32, u32, *const f32, u32) -> i32 = std::mem::transmute(method(dev, DEV_DRAW_PRIMITIVE_UP));
        set_texture(dev, 0, tex);
        draw_up(dev, D3DPT_TRIANGLESTRIP, 2, v.as_ptr(), 24);
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

/// A managed A8R8G8B8 texture of `image`.
unsafe fn texture(dev: P, image: &Image) -> P {
    unsafe {
        let create: unsafe extern "system" fn(P, u32, u32, u32, u32, u32, u32, *mut P, P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_TEXTURE));
        let mut tex: P = std::ptr::null_mut();
        if create(dev, image.w, image.h, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &mut tex, std::ptr::null_mut()) < 0 || tex.is_null() {
            return std::ptr::null_mut();
        }
        #[repr(C)]
        struct Locked {
            pitch: i32,
            bits: *mut u8,
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

/// The 16:9 picture set closest to the window height (the smallest at least as high, else the
/// highest): `<folder>/<W>x<H>/Landscape`.
fn load_pack(folder: &str, bezel: bool, height: u32) -> Result<Pack, String> {
    let mut sizes: Vec<(u32, u32)> = std::fs::read_dir(folder)
        .map_err(|e| format!("{folder}: {e}"))?
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let (w, h) = name.split_once('x')?;
            let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
            (w * 9 == h * 16 && std::path::Path::new(folder).join(&name).join("Landscape").is_dir()).then_some((w, h))
        })
        .collect();
    sizes.sort_by_key(|s| s.1);
    let size = *sizes.iter().find(|s| s.1 >= height).or(sizes.last()).ok_or_else(|| format!("{folder}: no <W>x<H>/Landscape picture set"))?;
    let dir = std::path::Path::new(folder).join(format!("{}x{}", size.0, size.1)).join("Landscape");
    let read = |name: &str| read_png(&dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()));
    let pack = Pack {
        size,
        bezel: if bezel { Some(read("Bezel.png")?) } else { None },
        frame: if bezel { Some(read("DoorsBezel.png")?) } else { None },
        left: read("DoorLeft.png")?,
        right: read("DoorRight.png")?,
    };
    log!("doors: pictures {} (bezel {bezel})", dir.display());
    Ok(pack)
}

/// A PNG as BGRA.
fn read_png(path: &std::path::Path) -> Result<Image, String> {
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
    // pictures one pixel larger than their screen (5120x1440's 5121x1441): cropped by the quad
    Ok(Image { w: info.width, h: info.height, bgra })
}
