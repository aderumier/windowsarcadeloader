//! The camera guns of JX's Top Gun 2 (`WAL_ARCADEPC_BOARD=jxcamera`).
//!
//! A camera (`WebCamTexture`, OpenCV's blob detector) watches the screen and finds each gun's
//! spot (`WebCamTextureToMatHelperExample.Pos`, the camera's pixels), mapped to the screen by
//! the cabinet's settings (`Game.json`'s `Srn*`, the per-player offsets). The game asks
//! `GetHPos(i)` (from the screen's center: the cursors of the name entry and the operator's
//! view) and `GetSrcPos(i)` (Unity's screen pixels, y up: the weapons' aim): both replaced
//! ([`Mono::replace`](wal_payload_common::mono::Mono::replace)), answering where each player's
//! gun points in the picture. The camera is never opened (none on the box; this one's left
//! alone): `WebCamTextureToMatHelper.Initialize()` (the camera's start: never started, the
//! helper idles) and the OpenCV `VideoCapture` settings (`SetCapture`, `SetNormalCapture`,
//! `GetExposure`) are replaced by nothing.
//!
//! `WAL_JX_LANDSCAPE=1`: the game on a 16:9 screen, its levels full screen (the screen's size
//! for `Screen.SetResolution`, the UI canvases scaled by the screen's height / 1920), the menus,
//! attract and loading screens a portrait column in the bezel the payload draws (hidden in the
//! levels: `GameMain._State`).

use std::sync::atomic::Ordering;
use std::time::Duration;

use wal_payload_common::mono::mono;
use wal_payload_common::{desktop, log};

use crate::demul;

/// Where a gun off the picture aims: far outside it.
const OFF: (f32, f32) = (-10000.0, -10000.0);

/// The game's picture in Unity's pixels.
fn picture() -> (f32, f32) {
    desktop::size().map_or((1920.0, 1080.0), |(w, h)| (w as f32, h as f32))
}

/// Player `i`'s aim in Unity's screen pixels (from the bottom left).
fn src_pos(i: i32) -> (f32, f32) {
    let (w, h) = picture();
    match (0..2).contains(&i).then(|| demul::gun_of_player_in_picture(i as usize)).flatten() {
        Some((x, y)) => (x * w, (1.0 - y) * h),
        None => OFF,
    }
}

/// Stores a `Vector2` where Mono's caller wants it: Unity 2017's Mono returns a structure in
/// a buffer of the caller's, its address the first argument (before `this`), and in RAX.
fn vector2(ret: *mut f32, (x, y): (f32, f32)) -> *mut f32 {
    if !ret.is_null() {
        unsafe {
            *ret = x;
            *ret.add(1) = y;
        }
    }
    ret
}

/// `GetSrcPos(int)`.
extern "C" fn get_src_pos(ret: *mut f32, _this: usize, i: i32) -> *mut f32 {
    vector2(ret, src_pos(i))
}

/// `GetHPos(int)`: from the picture's center.
extern "C" fn get_h_pos(ret: *mut f32, _this: usize, i: i32) -> *mut f32 {
    let (x, y) = src_pos(i);
    let (w, h) = picture();
    vector2(ret, (x - w / 2.0, y - h / 2.0))
}

/// The camera's start and settings: nothing (`GetExposure`: false).
extern "C" fn nothing() -> usize {
    0
}

/// Replaces the tracker's positions once the game's assembly is loaded.
fn replace() {
    for _ in 0..3000 {
        std::thread::sleep(Duration::from_millis(100));
        let Some(m) = mono() else { continue };
        if !m.attach() {
            continue;
        }
        let class = m.class(m.image("Assembly-CSharp"), "OpenCVForUnityExample", "WebCamTextureToMatHelperExample");
        if class.is_null() {
            continue;
        }
        let helper = m.class(m.image("Assembly-CSharp"), "OpenCVForUnity.UnityUtils.Helper", "WebCamTextureToMatHelper");
        let none = nothing as *const () as usize;
        let camera = [
            m.replace(m.method(helper, "Initialize", 0), none),
            m.replace(m.method(class, "SetCapture", 1), none),
            m.replace(m.method(class, "SetNormalCapture", 2), none),
            m.replace(m.method(class, "GetExposure", 1), none),
        ];
        if landscape() {
            landscape_resolution(m);
            game_state(m);
        }
        let src = m.replace(m.method(class, "GetSrcPos", 1), get_src_pos as *const () as usize);
        let h = m.replace(m.method(class, "GetHPos", 1), get_h_pos as *const () as usize);
        log!("jxcamera: the camera's gun positions are the payload's (GetSrcPos {src}, GetHPos {h}), the camera off ({camera:?})");
        return;
    }
    log!("jxcamera: the camera tracker not found");
}

// Landscape (`WAL_JX_LANDSCAPE=1`): the game on a 16:9 screen, its levels full screen

/// `WAL_JX_LANDSCAPE=1`: the game's 1920x1080 (the screen's) instead of the cabinet's portrait
/// 1080x1920.
pub fn landscape() -> bool {
    std::env::var("WAL_JX_LANDSCAPE").is_ok_and(|v| v.trim() == "1")
}

/// The bezel's image (`WAL_BEZEL`, taken from the portrait setup), drawn by the payload.
static BEZEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Before the common start: the portrait setup undone (its fake 1080x1920 desktop, its bezel's
/// strip windows), the bezel kept for the payload's own drawing.
pub fn prepare() {
    if !landscape() {
        return;
    }
    if let Ok(bezel) = std::env::var("WAL_BEZEL") {
        let _ = BEZEL.set(bezel.trim().to_string());
    }
    for var in ["WAL_DESKTOP_SIZE", "WAL_BEZEL", "WAL_BEZEL_FIT"] {
        unsafe { std::env::remove_var(var) };
    }
}

/// `Screen.SetResolution(int, int, bool, int)`'s compiled code.
static SET_RESOLUTION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `Screen.SetResolution(int width, int height, bool fullscreen)` (the game's 1080x1920 at its
/// start): the screen's size, full screen. The 3D scenes take the screen's aspect (the cameras'
/// vertical field of view kept): wider, not stretched.
extern "C" fn set_resolution(width: i32, height: i32, _fullscreen: u8) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    log!("jxcamera: SetResolution({width}, {height}) -> {w}x{h}");
    let f: extern "C" fn(i32, i32, u8, i32) = unsafe { std::mem::transmute(SET_RESOLUTION.load(Ordering::Relaxed)) };
    f(w, h, 1, 0);
}

fn landscape_resolution(m: &wal_payload_common::mono::Mono) {
    let screen = m.class(m.image("UnityEngine.CoreModule"), "UnityEngine", "Screen");
    let four = m.compiled(m.method(screen, "SetResolution", 4));
    if four == 0 {
        return log!("jxcamera: Screen.SetResolution not found: the game portrait");
    }
    SET_RESOLUTION.store(four, Ordering::Relaxed);
    let ok = m.replace(m.method(screen, "SetResolution", 3), set_resolution as *const () as usize);
    log!("jxcamera: the game at the screen's size ({ok})");
}

/// The UI canvases (constant pixel size, laid out on 1080x1920 around the screen's center)
/// scaled by the screen's height / 1920 (1 on the cabinet's screen): their portrait layout in
/// the landscape screen's middle, the menus and the levels' scores, time and messages whole
/// (at 1080 high, its top and bottom were off the screen). `CanvasScaler.HandleConstantPixelSize`
/// (UnityEngine.UI, compiled at its first call: patched before) passes `Screen.height / 1920`
/// to `SetScaleFactor` instead of `m_ScaleFactor` (1; its `SetReferencePixelsPerUnit` call
/// dropped: 100, the canvas's default).
fn ui_by_height() {
    for _ in 0..20000 {
        std::thread::sleep(Duration::from_millis(1));
        let Some(m) = mono() else { continue };
        let image = m.image("UnityEngine.UI");
        if image.is_null() || !m.attach() {
            continue;
        }
        let scaler = m.class(image, "UnityEngine.UI", "CanvasScaler");
        // Screen.get_height's token: HandleScaleWithScreenSize's second call
        let screen_size = m.il_code(m.method(scaler, "HandleScaleWithScreenSize", 0));
        let constant = m.il_code(m.method(scaler, "HandleConstantPixelSize", 0));
        match (screen_size, constant) {
            (Some(ss), Some(il)) if ss.len() > 0x0E && ss[0x09] == 0x28 && il.len() == 26 && il[0x03] == 0x7B && il[0x08] == 0x28 && il[25] == 0x2A => {
                let get_height: [u8; 4] = ss[0x0A..0x0E].try_into().unwrap();
                let set_scale: [u8; 4] = il[0x09..0x0D].try_into().unwrap();
                // ldarg.0; call Screen.get_height; conv.r4; ldc.r4 1920; div; call SetScaleFactor;
                // nops; ret (Mono decodes the whole body: the ret last)
                let mut body = vec![0x02, 0x28];
                body.extend_from_slice(&get_height);
                body.extend_from_slice(&[0x6B, 0x22, 0x00, 0x00, 0xF0, 0x44, 0x5B, 0x28]);
                body.extend_from_slice(&set_scale);
                body.resize(25, 0x00);
                body.push(0x2A);
                il.copy_from_slice(&body);
                log!("jxcamera: UI canvases scaled by the screen's height / 1920");
            }
            _ => log!("jxcamera: CanvasScaler's methods not the expected ones: the UI unscaled"),
        }
        return;
    }
}

/// `GameMain._State`'s address (the game's state machine: `State`), once found.
static STATE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// `LoadManager.Instance`'s address (each scene has its loading screen: the instance changes)
/// and `IsLoading`'s offset in it (a scene loading), once found.
static LOADING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static LOADING_OFFSET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The states of a level's scene: `PLAYGAME`, `PAUSEGAME`, `GAMEOVER`, `ACCOUNT`,
/// `PLAYERRANK` (the game's `State`: LOADING 0, DEMO 1, SELECTTYPE 2, SELECTMAP 3, LOADGAME 4,
/// PLAYGAME 5, PAUSEGAME 6, GAMEOVER 7, ACCOUNT 8, RANKING 9, WAITPLAYER 10, PLAYERRANK 11).
/// The level's state is set as its loading screen shows (`UpdateLoadGame`), the screen hidden
/// a second after the scene's load (`LoadManager.ReCover`): a level once it is in such a state
/// and no scene has loaded for 1.5 s.
fn in_level() -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST_MENU: Mutex<Option<Instant>> = Mutex::new(None);
    let (state, loading) = (STATE.load(Ordering::Relaxed), LOADING.load(Ordering::Relaxed));
    if state == 0 {
        return false;
    }
    let level = matches!(unsafe { (state as *const i32).read_volatile() }, 5..=8 | 11);
    let loader = if loading == 0 { 0 } else { unsafe { (loading as *const usize).read_volatile() } };
    let loads = loader != 0 && unsafe { ((loader + LOADING_OFFSET.load(Ordering::Relaxed)) as *const u8).read_volatile() } != 0;
    let mut last = LAST_MENU.lock().unwrap();
    let now = Instant::now();
    if !level || loads || last.is_none() {
        *last = Some(now);
    }
    level && last.is_some_and(|t| now.duration_since(t).as_secs_f32() > 1.5)
}

/// Finds `GameMain`'s singleton (`MonoSingletonManager<GameMain>.instance`, set at the game's
/// start, kept across scenes) and its `_State`.
fn game_state(m: &wal_payload_common::mono::Mono) {
    let image = m.image("Assembly-CSharp");
    let singleton = m.class_of_type_name(image, "MonoSingletonManager`1[[GameMain, Assembly-CSharp]]");
    let state = m.field(m.class(image, "", "GameMain"), "_State");
    let instance = m.static_addr(singleton, m.field(singleton, "instance"));
    if instance.is_null() || state.is_null() {
        return log!("jxcamera: GameMain's state not found: the bezel always drawn");
    }
    let (instance, offset) = (instance as usize, m.offset(state));
    let load = m.class(image, "", "LoadManager");
    let load_instance = m.static_addr(load, m.field(load, "Instance")) as usize;
    let is_loading = m.field(load, "IsLoading");
    let load_offset = if is_loading.is_null() { 0 } else { m.offset(is_loading) };
    std::thread::spawn(move || {
        for _ in 0..6000 {
            let obj = unsafe { (instance as *const usize).read_volatile() };
            if obj != 0 {
                STATE.store(obj + offset, Ordering::Relaxed);
                if load_instance != 0 && load_offset != 0 {
                    LOADING_OFFSET.store(load_offset, Ordering::Relaxed);
                    LOADING.store(load_instance, Ordering::Relaxed);
                }
                return log!("jxcamera: GameMain's state found: the bezel hidden in the levels");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}

/// The bezel drawn on the game's picture (Direct3D 11): its parts left and right of its
/// opening against the portrait column (the screen's height, 9:16) at the screen's middle, the
/// image's scale the screen's height (the portrait setup's `WAL_BEZEL_FIT`), black beyond;
/// hidden in the levels (full screen).
fn bezel() {
    use wal_payload_common::images_d3d11::{self, Image};
    let Some(path) = BEZEL.get().filter(|p| !p.is_empty()) else { return };
    let base = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let image = match images_d3d11::read_png(&base.join(path)) {
        Ok(i) => i,
        Err(e) => return log!("jxcamera: bezel {e}"),
    };
    let (w, h) = (image.w as usize, image.h as usize);
    // the opening: the dark or transparent columns around the middle row's center
    let px = |x: usize| &image.bgra[(h / 2 * w + x) * 4..][..4];
    let dark = |x: usize| {
        let p = px(x);
        p[3] < 16 || (p[0] < 16 && p[1] < 16 && p[2] < 16)
    };
    let (mut l, mut r) = (w / 2, w / 2);
    while l > 0 && dark(l - 1) {
        l -= 1;
    }
    while r < w && dark(r) {
        r += 1;
    }
    if r - l < w / 8 {
        return log!("jxcamera: bezel {path}: no opening");
    }
    let part = |x0: usize, x1: usize| Image {
        w: (x1 - x0) as u32,
        h: h as u32,
        bgra: (0..h).flat_map(|y| image.bgra[(y * w + x0) * 4..(y * w + x1) * 4].to_vec()).collect(),
    };
    let black = Image { w: 1, h: 1, bgra: vec![0, 0, 0, 255] };
    let images = vec![part(0, l), part(r, w), black];
    let _ = GEOMETRY.set((l as f32 / h as f32, (w - r) as f32 / h as f32));
    log!("jxcamera: bezel {path} ({w}x{h}, opening x {l}..{r}) around the menus");
    images_d3d11::init(images, bezel_quads);
}

/// The bezel's parts' widths, in the image's heights.
static GEOMETRY: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();

fn bezel_quads() -> Vec<wal_payload_common::images_d3d11::Quad> {
    use wal_payload_common::images_d3d11::Quad;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let Some(&(left, right)) = GEOMETRY.get() else { return Vec::new() };
    if in_level() {
        return Vec::new();
    }
    // fractions of the picture's width: the column (9:16 of its height), the parts beside
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) };
    let aspect = sh / sw;
    let column = aspect * 9.0 / 16.0;
    let (gl, gr) = (0.5 - column / 2.0, 0.5 + column / 2.0);
    let (lw, rw) = (left * aspect, right * aspect);
    vec![
        Quad { image: 2, rect: [0.0, 0.0, (gl - lw).max(0.0), 1.0] },
        Quad { image: 2, rect: [gr + rw, 0.0, (1.0 - gr - rw).max(0.0), 1.0] },
        Quad { image: 0, rect: [gl - lw, 0.0, lw, 1.0] },
        Quad { image: 1, rect: [gr, 0.0, rw, 1.0] },
    ]
}

pub fn init() {
    if landscape() {
        std::thread::spawn(ui_by_height);
        bezel();
    }
    std::thread::spawn(replace);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector2_layout() {
        let mut v = [0f32; 2];
        assert_eq!(vector2(v.as_mut_ptr(), (1.5, -2.0)), v.as_mut_ptr());
        assert_eq!(v, [1.5, -2.0]);
    }
}
