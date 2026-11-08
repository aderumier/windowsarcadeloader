//! House of the Dead: Scarlet Dawn's graphics options (Unreal Engine 4.18, all effects at the
//! cabinet's level by default: too heavy for a small GPU). Written as console variables in the
//! game's `Saved\Config\WindowsNoEditor\Engine.ini` (`-NotInstalled`: the game's folder) before
//! the engine starts:
//!
//! - `WAL_HOTD_EXCLUSIVE=1`: the exclusive full screen mode (`r.FullScreenMode=0`) at the
//!   `-ResX`/`-ResY` mode (the Resolution option), which Wine scales to the screen (FSR
//!   upscaling); else the engine's borderless window, the desktop's size
//! - `WAL_HOTD_RENDER_SCALE`: the 3D picture's resolution in percent of the window's (the
//!   menus and HUD stay sharp), `WAL_HOTD_SHARPEN` its sharpening (0 to 1)
//! - `WAL_HOTD_SHADOWS`: `on` (the game's), `low`, `off`
//! - `WAL_HOTD_AA`: `taa` (the game's temporal anti-aliasing), `fxaa`, `off`
//! - `WAL_HOTD_AO`, `WAL_HOTD_SSR`, `WAL_HOTD_MOTION_BLUR`, `WAL_HOTD_DOF`, `WAL_HOTD_BLOOM`:
//!   `0` turns the effect off (ambient occlusion, screen space reflections, motion blur, depth
//!   of field, bloom and lens flares)
//!
//! The cut gore: `gore.rs` (`WAL_HOTD_GORE`).

use wal_payload_common::unreal_ini;

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty())
}

/// The console variables of the options.
fn settings() -> Vec<(&'static str, String)> {
    let mut s = Vec::new();
    if var("WAL_HOTD_EXCLUSIVE").as_deref() == Some("1") {
        s.push(("r.FullScreenMode", "0".to_string()));
    }
    if let Some(scale) = var("WAL_HOTD_RENDER_SCALE").and_then(|v| v.parse::<u32>().ok()).filter(|&v| (25..100).contains(&v)) {
        s.push(("r.ScreenPercentage", scale.to_string()));
    }
    if let Some(sharpen) = var("WAL_HOTD_SHARPEN").and_then(|v| v.parse::<f32>().ok()).filter(|v| (0.0..=1.0).contains(v)) {
        s.push(("r.Tonemapper.Sharpen", sharpen.to_string()));
    }
    match var("WAL_HOTD_SHADOWS").as_deref() {
        Some("off") => s.push(("r.ShadowQuality", "0".into())),
        Some("low") => s.extend([("r.ShadowQuality", "2".into()), ("r.Shadow.MaxResolution", "1024".into()), ("r.Shadow.CSM.MaxCascades", "1".into())]),
        _ => {}
    }
    match var("WAL_HOTD_AA").as_deref() {
        Some("off") => s.push(("r.PostProcessAAQuality", "0".into())),
        Some("fxaa") => s.extend([("r.DefaultFeature.AntiAliasing", "1".into()), ("r.PostProcessAAQuality", "2".into())]),
        _ => {}
    }
    let off = |key: &str| var(key).as_deref() == Some("0");
    if off("WAL_HOTD_AO") {
        s.push(("r.AmbientOcclusionLevels", "0".into()));
    }
    if off("WAL_HOTD_SSR") {
        s.push(("r.SSR.Quality", "0".into()));
    }
    if off("WAL_HOTD_MOTION_BLUR") {
        s.push(("r.MotionBlurQuality", "0".into()));
    }
    if off("WAL_HOTD_DOF") {
        s.push(("r.DepthOfFieldQuality", "0".into()));
    }
    if off("WAL_HOTD_BLOOM") {
        s.extend([("r.BloomQuality", "0".into()), ("r.LensFlareQuality", "0".into())]);
    }
    s
}

pub(crate) fn init() {
    let Ok(exe) = std::env::current_exe() else { return };
    // Binaries\Win64\<exe> -> Saved\Config\WindowsNoEditor\Engine.ini
    let Some(project) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) else { return };
    unreal_ini::system_settings(&project.join("Saved").join("Config").join("WindowsNoEditor").join("Engine.ini"), &settings());
}
