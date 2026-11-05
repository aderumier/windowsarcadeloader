//! A portrait Unity cabinet game on a 16:9 screen: `WAL_UNITY_LANDSCAPE=1`.
//!
//! The game asks its cabinet's 1080x1920 (`Screen.SetResolution(1080, 1920, true)`) and lays
//! its UI out on it (constant pixel size canvases, around the screen's center). In landscape:
//!
//! - **Before the common start** ([`prepare`]): the portrait setup undone (`WAL_DESKTOP_SIZE`,
//!   `WAL_BEZEL`, `WAL_BEZEL_FIT` removed from the environment), the bezel's image kept.
//! - **Resolution.** `Screen.SetResolution(int, int, bool)` replaced: the screen's size, full
//!   screen (the 4-argument overload's compiled code). The cameras keep their vertical field of
//!   view: the 3D scenes take the wider aspect, not stretched.
//! - **UI.** `CanvasScaler.HandleConstantPixelSize` (UnityEngine.UI, compiled at its first call:
//!   its IL patched before) passes `Screen.height / 1920` to `SetScaleFactor` instead of
//!   `m_ScaleFactor` (1; its `SetReferencePixelsPerUnit` call dropped: 100, the canvas's
//!   default): the portrait layout whole in the screen's middle (at 1080 high its top and
//!   bottom were off the screen), 1 on the cabinet's screen. The instructions are found by
//!   pattern (debug and release builds of the DLL differ).
//! - **Bezel.** Drawn on the game's picture (`images_d3d11.rs`): the image's parts left and
//!   right of its opening against the portrait column (9:16 of the height) at the screen's
//!   middle, the image's scale the screen's height (the portrait setup's `WAL_BEZEL_FIT`), black
//!   beyond; hidden while the game's `in_level` says a level plays (full screen).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::images_d3d11::{self, Image, Quad};
use crate::log;
use crate::mono::{Mono, mono};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// `WAL_UNITY_LANDSCAPE=1`.
pub fn enabled() -> bool {
    std::env::var("WAL_UNITY_LANDSCAPE").is_ok_and(|v| v.trim() == "1")
}

/// The bezel's image (`WAL_BEZEL`, taken from the portrait setup).
static BEZEL: OnceLock<String> = OnceLock::new();

/// Before the common start: the portrait setup undone, the bezel kept for the payload's own
/// drawing.
pub fn prepare() {
    if !enabled() {
        return;
    }
    if let Ok(bezel) = std::env::var("WAL_BEZEL") {
        let _ = BEZEL.set(bezel.trim().to_string());
    }
    for var in ["WAL_DESKTOP_SIZE", "WAL_BEZEL", "WAL_BEZEL_FIT"] {
        unsafe { std::env::remove_var(var) };
    }
}

/// The game's level test (the bezel hidden while it says so).
static IN_LEVEL: OnceLock<fn() -> bool> = OnceLock::new();

/// Starts the landscape changes; `in_level`: whether a level plays now (called at each frame).
pub fn start(in_level: fn() -> bool) {
    if !enabled() {
        return;
    }
    let _ = IN_LEVEL.set(in_level);
    std::thread::spawn(ui_by_height);
    std::thread::spawn(resolution);
    bezel();
}

/// `Screen.SetResolution(int, int, bool, int)`'s compiled code.
static SET_RESOLUTION: AtomicUsize = AtomicUsize::new(0);

/// `Screen.SetResolution(int width, int height, bool fullscreen)`: the screen's size, full
/// screen.
extern "C" fn set_resolution(width: i32, height: i32, _fullscreen: u8) {
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    log!("landscape: SetResolution({width}, {height}) -> {w}x{h}");
    let f: extern "C" fn(i32, i32, u8, i32) = unsafe { std::mem::transmute(SET_RESOLUTION.load(Ordering::Relaxed)) };
    f(w, h, 1, 0);
}

/// Replaces `Screen.SetResolution(int, int, bool)` once UnityEngine is loaded.
fn resolution() {
    for _ in 0..20000 {
        std::thread::sleep(Duration::from_millis(1));
        let Some(m) = mono() else { continue };
        let image = m.image("UnityEngine.CoreModule");
        if image.is_null() || !m.attach() {
            continue;
        }
        let screen = m.class(image, "UnityEngine", "Screen");
        let four = m.compiled(m.method(screen, "SetResolution", 4));
        if four == 0 {
            return log!("landscape: Screen.SetResolution not found: the game portrait");
        }
        SET_RESOLUTION.store(four, Ordering::Relaxed);
        let ok = m.replace(m.method(screen, "SetResolution", 3), set_resolution as *const () as usize);
        return log!("landscape: the game at the screen's size ({ok})");
    }
}

/// The position of `pattern` (`None`: any byte) in `code`.
fn find(code: &[u8], pattern: &[Option<u8>]) -> Option<usize> {
    code.windows(pattern.len()).position(|w| w.iter().zip(pattern).all(|(b, p)| p.is_none_or(|p| *b == p)))
}

/// `HandleConstantPixelSize`'s new body: `ldarg.0; call Screen.get_height; conv.r4; ldc.r4 1920;
/// div; call SetScaleFactor`, nops, `ret` (Mono decodes the whole body: the `ret` last), or
/// `None` if the methods are not the expected ones.
fn scale_by_height(screen_size: &[u8], constant: &[u8]) -> Option<Vec<u8>> {
    const T: Option<u8> = None;
    // Screen.get_width() / get_height() as floats: `call w; conv.r4; call h; conv.r4`
    let at = find(screen_size, &[Some(0x28), T, T, T, T, Some(0x6B), Some(0x28), T, T, T, T, Some(0x6B)])?;
    let get_height = &screen_size[at + 7..at + 11];
    // `ldarg.0; ldarg.0; ldfld m_ScaleFactor; call SetScaleFactor`
    let at = find(constant, &[Some(0x02), Some(0x02), Some(0x7B), T, T, T, T, Some(0x28), T, T, T, T])?;
    let set_scale = &constant[at + 8..at + 12];
    if constant.last() != Some(&0x2A) || constant.len() < 19 {
        return None;
    }
    let mut body = vec![0x02, 0x28];
    body.extend_from_slice(get_height);
    body.extend_from_slice(&[0x6B, 0x22, 0x00, 0x00, 0xF0, 0x44, 0x5B, 0x28]);
    body.extend_from_slice(set_scale);
    body.resize(constant.len() - 1, 0x00);
    body.push(0x2A);
    Some(body)
}

/// Patches `CanvasScaler.HandleConstantPixelSize` as soon as UnityEngine.UI is loaded (before
/// its first call).
fn ui_by_height() {
    for _ in 0..20000 {
        std::thread::sleep(Duration::from_millis(1));
        let Some(m) = mono() else { continue };
        let image = m.image("UnityEngine.UI");
        if image.is_null() || !m.attach() {
            continue;
        }
        let scaler = m.class(image, "UnityEngine.UI", "CanvasScaler");
        let screen_size = m.il_code(m.method(scaler, "HandleScaleWithScreenSize", 0));
        let constant = m.il_code(m.method(scaler, "HandleConstantPixelSize", 0));
        match (screen_size, constant) {
            (Some(ss), Some(il)) => match scale_by_height(ss, il) {
                Some(body) => {
                    il.copy_from_slice(&body);
                    log!("landscape: UI canvases scaled by the screen's height / 1920");
                }
                None => log!("landscape: CanvasScaler's methods not the expected ones: the UI unscaled"),
            },
            _ => log!("landscape: CanvasScaler not found: the UI unscaled"),
        }
        return;
    }
}

/// The bezel's parts' widths, in the image's heights.
static GEOMETRY: OnceLock<(f32, f32)> = OnceLock::new();

fn bezel() {
    let Some(path) = BEZEL.get().filter(|p| !p.is_empty()) else { return };
    let base = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let image = match images_d3d11::read_png(&base.join(path)) {
        Ok(i) => i,
        Err(e) => return log!("landscape: bezel {e}"),
    };
    let (w, h) = (image.w as usize, image.h as usize);
    // the opening: the dark or transparent columns around the middle row's center
    let dark = |x: usize| {
        let p = &image.bgra[(h / 2 * w + x) * 4..][..4];
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
        return log!("landscape: bezel {path}: no opening");
    }
    let part = |x0: usize, x1: usize| Image {
        w: (x1 - x0) as u32,
        h: h as u32,
        bgra: (0..h).flat_map(|y| image.bgra[(y * w + x0) * 4..(y * w + x1) * 4].to_vec()).collect(),
    };
    let black = Image { w: 1, h: 1, bgra: vec![0, 0, 0, 255] };
    let images = vec![part(0, l), part(r, w), black];
    let _ = GEOMETRY.set((l as f32 / h as f32, (w - r) as f32 / h as f32));
    log!("landscape: bezel {path} ({w}x{h}, opening x {l}..{r}) around the portrait screens");
    images_d3d11::init(images, bezel_quads);
}

fn bezel_quads() -> Vec<Quad> {
    let Some(&(left, right)) = GEOMETRY.get() else { return Vec::new() };
    if IN_LEVEL.get().is_some_and(|f| f()) {
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

/// Reads a pointer-sized value at `at` (0: none).
pub fn read_ptr(at: usize) -> usize {
    if at == 0 { 0 } else { unsafe { (at as *const usize).read_volatile() } }
}

/// The class (`MonoClass*`) of the object at `obj`: its vtable's first field.
pub fn class_of(obj: usize) -> usize {
    read_ptr(read_ptr(obj))
}

/// A Mono array's first element's offset (its header: vtable, sync, bounds, length).
pub const ARRAY_DATA: usize = 4 * size_of::<usize>();

/// Waits for Mono and UnityEngine's game assembly (`Assembly-CSharp`), then calls `f` with them
/// on this thread (attached).
pub fn with_game_assembly(f: impl FnOnce(&'static Mono, *mut std::ffi::c_void) + Send + 'static) {
    std::thread::spawn(move || {
        for _ in 0..3000 {
            std::thread::sleep(Duration::from_millis(100));
            let Some(m) = mono() else { continue };
            let image = m.image("Assembly-CSharp");
            if image.is_null() || !m.attach() {
                continue;
            }
            return f(m, image);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_by_height_release_build() {
        // Top Gun Arcade's UnityEngine.UI (release): 25 bytes
        let constant = [0x02, 0x02, 0x7B, 1, 0, 0, 4, 0x28, 2, 0, 0, 6, 0x02, 0x02, 0x7B, 3, 0, 0, 4, 0x28, 5, 0, 0, 6, 0x2A];
        let screen = [0x12, 0x00, 0x28, 7, 0, 0, 10, 0x6B, 0x28, 8, 0, 0, 10, 0x6B, 0x28];
        let body = scale_by_height(&screen, &constant).unwrap();
        assert_eq!(body.len(), 25);
        assert_eq!(&body[..6], &[0x02, 0x28, 8, 0, 0, 10]);
        assert_eq!(&body[13..18], &[0x28, 2, 0, 0, 6]);
        assert_eq!(body[24], 0x2A);
    }
}
