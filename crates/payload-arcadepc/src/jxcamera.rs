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
        let src = m.replace(m.method(class, "GetSrcPos", 1), get_src_pos as *const () as usize);
        let h = m.replace(m.method(class, "GetHPos", 1), get_h_pos as *const () as usize);
        log!("jxcamera: the camera's gun positions are the payload's (GetSrcPos {src}, GetHPos {h}), the camera off ({camera:?})");
        return;
    }
    log!("jxcamera: the camera tracker not found");
}

pub fn init() {
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
