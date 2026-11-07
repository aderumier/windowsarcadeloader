//! A lower render resolution for a Unity HDRP game (Need for Speed Heat Takedown: Unity 2021.2,
//! HDRP 12), upscaled in HDRP's final pass (Catmull-Rom): `WAL_HDRP_RENDER_SCALE` (0.5..1, 1 the
//! game's own). Not FSR or CAS: HDRP 12's compute upscalers (`EdgeAdaptiveSpatialUpsampling`,
//! `ContrastAdaptiveSharpeningPass`) showed only the picture's top-left part at the screen's size
//! (zoomed by 1/scale, the car off center, the right and the top cut).
//!
//! HDRP's dynamic resolution, forced: the pipeline asset's `m_RenderPipelineSettings
//! .dynamicResolutionSettings` (read by `HDRenderPipeline` each frame for each camera) set to
//! enabled, software, `forceResolution` at the scale (`forcedPercentage`), the `CatmullRom`
//! upscaler; written from a payload thread attached to Mono,
//! the asset found from `RenderPipelineManager.s_currentPipeline` (`HDRenderPipeline.m_Asset`),
//! again whenever the game's values come back (a new pipeline). A camera only scales when its
//! `HDAdditionalCameraData.allowDynamicResolution` is set (none of the game's):
//! `DynamicResolutionHandler.SetCurrentCameraRequest(bool)`'s IL (`ldarg.0 ldarg.1 stfld ret`)
//! patched to store true (`ldc.i4.1`), before HDRP's first frame compiles it.
//!
//! The game's own post-processing effects (its custom HDRP volume components), on unless
//! `WAL_NFS_RADIAL_BLUR=0` (the speed blur: `RadialBlur_PostProcess`, `RadialBlur`) or
//! `WAL_NFS_VIGNETTE=0` (`GrayscaleVignette`): their `IsActive()` made to return false, so HDRP
//! skips them.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use wal_payload_common::mono::{BOXED, mono};

/// `WAL_HDRP_RENDER_SCALE`, when under 1.
fn scale() -> Option<f32> {
    static SCALE: OnceLock<Option<f32>> = OnceLock::new();
    *SCALE.get_or_init(|| {
        let s = std::env::var("WAL_HDRP_RENDER_SCALE").ok()?.trim().parse::<f32>().ok()?.clamp(0.5, 1.0);
        (s < 0.999).then_some(s)
    })
}

/// At the game's first `DllImport`s (Mono loaded, its images too, HDRP's first frame not
/// rendered yet): the cameras' patch until done, the settings thread once.
pub fn init() {
    effects();
    static PATCHED: AtomicBool = AtomicBool::new(false);
    static STARTED: AtomicBool = AtomicBool::new(false);
    if scale().is_none() || PATCHED.load(Ordering::Relaxed) {
        return;
    }
    let Some(m) = mono() else { return };
    let core = m.image("Unity.RenderPipelines.Core.Runtime");
    if core.is_null() {
        return;
    }
    PATCHED.store(true, Ordering::Relaxed);
    let handler = m.class(core, "UnityEngine.Rendering", "DynamicResolutionHandler");
    match m.il_code(m.method(handler, "SetCurrentCameraRequest", 1)) {
        Some(il) if il.len() == 8 && il[..3] == [0x02, 0x03, 0x7D] => {
            il[1] = 0x17; // ldc.i4.1
            log!("hdrp: dynamic resolution allowed on every camera");
        }
        _ => log!("hdrp: DynamicResolutionHandler.SetCurrentCameraRequest not the expected one: no render scale"),
    }
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(settings_thread);
    }
}

/// The game's post-processing effects turned off by the profile, once its assembly is loaded.
fn effects() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let Some(m) = mono() else { return };
    let image = m.image("Assembly-CSharp");
    if image.is_null() {
        return;
    }
    DONE.store(true, Ordering::Relaxed);
    let effects: [(&str, &[&str]); 2] =
        [("WAL_NFS_RADIAL_BLUR", &["RadialBlur_PostProcess", "RadialBlur"]), ("WAL_NFS_VIGNETTE", &["GrayscaleVignette"])];
    for (env, classes) in effects {
        if std::env::var(env).map_or(true, |v| v.trim() != "0") {
            continue;
        }
        for class in classes {
            match m.il_code(m.method(m.class(image, "", class), "IsActive", 0)) {
                Some(il) if il.len() >= 2 && il[il.len() - 1] == 0x2A => {
                    // nops, ldc.i4.0 ret (Mono decodes the whole body: no bytes left after the ret)
                    let n = il.len();
                    il.fill(0x00);
                    il[n - 2..].copy_from_slice(&[0x16, 0x2A]);
                    log!("hdrp: {class} off ({env}=0)");
                }
                _ => log!("hdrp: {class}.IsActive not the expected one: left on"),
            }
        }
    }
}

fn settings_thread() {
    let Some(m) = mono() else { return };
    if !m.attach() {
        log!("hdrp: cannot attach to Mono: no render scale");
        return;
    }
    let scale = scale().unwrap_or(1.0);
    let mut logged = false;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let core = m.image("UnityEngine.CoreModule");
        let hd = m.image("Unity.RenderPipelines.HighDefinition.Runtime");
        let drs = m.image("Unity.RenderPipelines.Core.Runtime");
        let manager = m.class(core, "UnityEngine.Rendering", "RenderPipelineManager");
        let pipeline_class = m.class(hd, "UnityEngine.Rendering.HighDefinition", "HDRenderPipeline");
        let asset_class = m.class(hd, "UnityEngine.Rendering.HighDefinition", "HDRenderPipelineAsset");
        let settings_class = m.class(hd, "UnityEngine.Rendering.HighDefinition", "RenderPipelineSettings");
        let drs_class = m.class(drs, "UnityEngine.Rendering", "GlobalDynamicResolutionSettings");
        let current = m.static_addr(manager, m.field(manager, "s_currentPipeline"));
        let fields = [
            m.field(pipeline_class, "m_Asset"),
            m.field(asset_class, "m_RenderPipelineSettings"),
            m.field(settings_class, "dynamicResolutionSettings"),
        ];
        let drs_field = |name| m.field(drs_class, name);
        let names = ["enabled", "maxPercentage", "minPercentage", "dynResType", "upsampleFilter", "forceResolution", "forcedPercentage"];
        let drs_fields: Vec<_> = names.iter().map(|n| drs_field(n)).collect();
        if current.is_null() || fields.iter().chain(&drs_fields).any(|f| f.is_null()) {
            continue;
        }
        unsafe {
            let pipeline = (current as *const usize).read();
            if pipeline == 0 {
                continue;
            }
            let asset = ((pipeline + m.offset(fields[0])) as *const usize).read();
            if asset == 0 {
                continue;
            }
            // structs inline: the offsets of their fields count a boxed object's header
            let base = asset + m.offset(fields[1]) + m.offset(fields[2]) - BOXED;
            let at = |i: usize| base + m.offset(drs_fields[i]) - BOXED;
            let percent = scale * 100.0;
            if *(at(0) as *const u8) == 1 && *(at(5) as *const u8) == 1 && (*(at(6) as *const f32) - percent).abs() < 0.01 {
                continue;
            }
            if !logged {
                log!(
                    "hdrp: dynamic resolution {}, {}..{}%, type {}, filter {}, forced {} {}%",
                    *(at(0) as *const u8),
                    *(at(2) as *const f32),
                    *(at(1) as *const f32),
                    *(at(3) as *const u8),
                    *(at(4) as *const u8),
                    *(at(5) as *const u8),
                    *(at(6) as *const f32)
                );
            }
            *(at(1) as *mut f32) = percent;
            *(at(2) as *mut f32) = percent;
            *(at(3) as *mut u8) = 0; // DynamicResolutionType.Software
            *(at(4) as *mut u8) = 1; // DynamicResUpscaleFilter.CatmullRom
            *(at(6) as *mut f32) = percent;
            *(at(5) as *mut u8) = 1;
            *(at(0) as *mut u8) = 1;
            if !logged {
                log!("hdrp: render scale {scale} (Catmull-Rom upscaling)");
                logged = true;
            }
        }
    }
}
