//! Tomb Raider's picture options, each off with `=0` (or `off`), lighter on small GPUs:
//! `WAL_TOMBRAIDER_POSTPROCESS` every camera's post-processing (Unity's post-processing stack:
//! ambient occlusion, bloom, depth of field, colour grading; the menus' cameras have their own,
//! the gameplay camera's `EnablePostProcess` does not reach them), `WAL_TOMBRAIDER_FOG` the
//! volumetric fog, `WAL_TOMBRAIDER_REFLECTION` the water's planar reflection (a second render of
//! the scene), `WAL_TOMBRAIDER_SHADOWS` the real-time shadows (`QualitySettings.shadows`;
//! `low`: kept, at the low resolution without cascades, the game's are high with 2 cascades).
//! Every second from the main thread (`sxCoreGetData`): the scene's enabled components are
//! disabled (the game's cinematics turn the gameplay camera's on again).
//! On the integrated Radeon 860M, the game mode screen at 1920x1080: 22.6 ms a frame, 4.4 ms
//! of shadows, 3.6 ms of post-processing, 1.2 ms of fog, 0.4 ms of water reflection.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use wal_payload_common::log;
use wal_payload_common::mono::{self, Mono};

fn off(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| matches!(v.trim(), "0" | "off"))
}

/// The components turned off: (option, namespace, class).
const EFFECTS: [(&str, &str, &str); 3] = [
    ("WAL_TOMBRAIDER_POSTPROCESS", "UnityEngine.Rendering.PostProcessing", "PostProcessLayer"),
    ("WAL_TOMBRAIDER_FOG", "VolumetricFogAndMist", "VolumetricFog"),
    ("WAL_TOMBRAIDER_REFLECTION", "UnityStandardAssets.Water", "PlanarReflection"),
];

/// The crosshairs below 1920x1080: `CrosshairWindow.CrosshairMove` places a crosshair at the
/// aim's pixel (`ViewportToScreenPoint`) as its anchored position, in the units of a canvas scaled
/// from 1920x1080 (smaller screen modes: drawn short of the aim, towards the bottom left; the
/// shot's ray is the viewport's). `Player.MoveCrosshair` (its one caller, through the player's
/// `OnCrosshairMove`) is replaced: the viewport divided by the canvas's scale factor.
static SCALE: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0
static MOVE_FIELDS: AtomicUsize = AtomicUsize::new(0); // m_CrosshairMove | m_ID << 32

/// Mono's managed calls take the `Vector3` by value on the stack after the register arguments
/// (`this`, the damage type): the 5th and 6th stack slots (x and y, then z).
unsafe extern "system" fn move_crosshair(player: mono::P, _kind: i32, _r8: usize, _r9: usize, xy: u64, z: u64) {
    let Some(m) = mono::mono() else { return };
    let fields = MOVE_FIELDS.load(Ordering::Relaxed);
    let (delegate, id) = unsafe {
        (mono::read::<usize>(player as *const u8, fields & 0xFFFF_FFFF), mono::read::<i32>(player as *const u8, fields >> 32))
    };
    if delegate == 0 {
        return;
    }
    let scale = f32::from_bits(SCALE.load(Ordering::Relaxed));
    let (x, y, z) = (f32::from_bits(xy as u32), f32::from_bits((xy >> 32) as u32), f32::from_bits(z as u32));
    let (mut v, mut id) = ([x / scale, y / scale, z], id);
    let invoke = m.method(m.class_of(delegate as mono::P), "Invoke", 2);
    m.invoke(invoke, delegate as mono::P, &mut [(&raw mut v).cast(), (&raw mut id).cast()]);
}

fn crosshairs(m: &Mono, core: mono::P) {
    static REPLACED: std::sync::Once = std::sync::Once::new();
    let image = m.image("Assembly-CSharp");
    let player = m.class(image, "", "Player");
    let window = m.class(image, "", "CrosshairWindow");
    if player.is_null() || window.is_null() {
        return;
    }
    REPLACED.call_once(|| {
        let (delegate, id) = (m.field(player, "m_CrosshairMove"), m.field(player, "m_ID"));
        let ok = !delegate.is_null()
            && !id.is_null()
            && {
                MOVE_FIELDS.store(m.offset(delegate) | m.offset(id) << 32, Ordering::Relaxed);
                m.replace(m.method(player, "MoveCrosshair", 2), move_crosshair as *const () as usize)
            };
        log!("tombraider: crosshairs at the aim below 1920x1080 ({})", if ok { "ok" } else { "Player.MoveCrosshair not found" });
    });
    // the crosshair window's canvas scale factor
    let find = m.method(m.class(core, "UnityEngine", "Object"), "FindObjectOfType", 1);
    let Some(w) = m.invoke(find, std::ptr::null_mut(), &mut [m.type_object(window)]) else { return };
    if w.is_null() {
        return;
    }
    let canvas = m.class(m.image("UnityEngine.UIModule"), "UnityEngine", "Canvas");
    let in_parent = m.method(m.class(core, "UnityEngine", "Component"), "GetComponentInParent", 1);
    let Some(c) = m.invoke(in_parent, w, &mut [m.type_object(canvas)]) else { return };
    if c.is_null() {
        return;
    }
    let Some(r) = m.invoke(m.method(canvas, "get_scaleFactor", 0), c, &mut []) else { return };
    let scale = unsafe { mono::read::<f32>(r as *const u8, mono::BOXED) };
    if scale > 0.1 && SCALE.swap(scale.to_bits(), Ordering::Relaxed) != scale.to_bits() {
        log!("tombraider: crosshair canvas scale {scale}");
    }
}

/// The main thread's call (every frame).
pub fn frame(m: &Mono) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed().as_secs() < 1) {
            return;
        }
        *last = Some(Instant::now());
    }
    let core = m.image("UnityEngine.CoreModule");
    crosshairs(m, core);
    if std::env::var("WAL_TOMBRAIDER_SHADOWS").is_ok_and(|v| v.trim() == "low") {
        let quality = m.class(core, "UnityEngine", "QualitySettings");
        let get = |name| match m.invoke(m.method(quality, name, 0), std::ptr::null_mut(), &mut []) {
            Some(r) if !r.is_null() => unsafe { mono::read::<i32>(r as *const u8, mono::BOXED) },
            _ => -1,
        };
        // ShadowResolution.Low, no cascades
        for (getter, setter, value) in [("get_shadowResolution", "set_shadowResolution", 0), ("get_shadowCascades", "set_shadowCascades", 1)] {
            let was = get(getter);
            if was >= 0 && was != value {
                let mut v: i32 = value;
                m.invoke(m.method(quality, setter, 1), std::ptr::null_mut(), &mut [(&raw mut v).cast()]);
                log!("tombraider: {} {was} -> {value}", &setter[4..]);
            }
        }
    }
    if off("WAL_TOMBRAIDER_SHADOWS") {
        let quality = m.class(core, "UnityEngine", "QualitySettings");
        let mut disable: i32 = 0; // ShadowQuality.Disable
        let shadows = m.invoke(m.method(quality, "get_shadows", 0), std::ptr::null_mut(), &mut []);
        if shadows.is_some_and(|r| !r.is_null() && unsafe { mono::read::<i32>(r as *const u8, mono::BOXED) } != 0) {
            m.invoke(m.method(quality, "set_shadows", 1), std::ptr::null_mut(), &mut [(&raw mut disable).cast()]);
            log!("tombraider: shadows off");
        }
    }
    if !EFFECTS.iter().any(|(key, ..)| off(key)) {
        return;
    }
    let image = m.image("Assembly-CSharp");
    let find = m.method(m.class(core, "UnityEngine", "Object"), "FindObjectsOfType", 1);
    let behaviour = m.class(core, "UnityEngine", "Behaviour");
    let (get, set) = (m.method(behaviour, "get_enabled", 0), m.method(behaviour, "set_enabled", 1));
    if find.is_null() || get.is_null() || set.is_null() {
        return;
    }
    let mut count = 0;
    for (key, namespace, name) in EFFECTS {
        if !off(key) {
            continue;
        }
        let class = m.class(image, namespace, name);
        if class.is_null() {
            continue;
        }
        let Some(array) = m.invoke(find, std::ptr::null_mut(), &mut [m.type_object(class)]) else { continue };
        if array.is_null() {
            continue;
        }
        // an array's length follows its header and bounds
        let len = unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA - size_of::<usize>()) };
        for i in 0..len {
            let obj = unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA + i * size_of::<usize>()) } as mono::P;
            let on = m.invoke(get, obj, &mut []).is_some_and(|r| !r.is_null() && unsafe { mono::read::<u8>(r as *const u8, mono::BOXED) } != 0);
            if on {
                let mut no: u8 = 0;
                m.invoke(set, obj, &mut [(&raw mut no).cast()]);
                count += 1;
            }
        }
    }
    if count > 0 {
        log!("tombraider: {count} post-processing, fog or reflection components off");
    }
}
