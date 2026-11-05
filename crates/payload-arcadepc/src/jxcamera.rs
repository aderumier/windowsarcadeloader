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
//! `WAL_UNITY_LANDSCAPE=1` (`payload-common`'s `unity_landscape.rs`): the game on a 16:9
//! screen, its levels full screen, the menus, attract and loading screens a portrait column in
//! the bezel, hidden in the levels (`GameMain._State`).

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
        if wal_payload_common::unity_landscape::enabled() {
            game_state(m);
        }
        let src = m.replace(m.method(class, "GetSrcPos", 1), get_src_pos as *const () as usize);
        let h = m.replace(m.method(class, "GetHPos", 1), get_h_pos as *const () as usize);
        log!("jxcamera: the camera's gun positions are the payload's (GetSrcPos {src}, GetHPos {h}), the camera off ({camera:?})");
        return;
    }
    log!("jxcamera: the camera tracker not found");
}

// Landscape (`WAL_UNITY_LANDSCAPE=1`): the bezel hidden in the levels

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

pub fn init() {
    wal_payload_common::unity_landscape::start(in_level);
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
