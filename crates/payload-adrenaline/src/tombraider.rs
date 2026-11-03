//! Tomb Raider's picture options: `WAL_TOMBRAIDER_POSTPROCESS=0` turns the gameplay camera's
//! post-processing (Unity's post-processing stack: ambient occlusion, bloom, depth of field,
//! colour grading) and volumetric fog off, lighter on small GPUs. The game's own
//! `GameplayCamera.EnablePostProcess(false)` (its cinematics turn them off and on again), called
//! every second from the main thread (`sxCoreGetData`) on the scene's camera
//! (`SBK.SceneSingleton<GameplayCamera>.m_Main`).

use std::sync::Mutex;
use std::time::Instant;

use wal_payload_common::log;
use wal_payload_common::mono::{self, Mono};

fn enabled() -> bool {
    std::env::var("WAL_TOMBRAIDER_POSTPROCESS").is_ok_and(|v| v.trim() == "0")
}

/// The main thread's call (every frame).
pub fn frame(m: &Mono) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    static CAMERA: Mutex<usize> = Mutex::new(0);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed().as_secs() < 1) {
            return;
        }
        *last = Some(Instant::now());
    }
    if !enabled() {
        return;
    }
    let image = m.image("Assembly-CSharp");
    let singleton = m.class_of_type_name(image, "SBK.SceneSingleton`1[[GameplayCamera, Assembly-CSharp]]");
    let main = m.static_addr(singleton, m.field(singleton, "m_Main"));
    if main.is_null() {
        return;
    }
    let camera = unsafe { mono::read::<usize>(main, 0) };
    if camera == 0 {
        return;
    }
    let method = m.method(m.class(image, "", "GameplayCamera"), "EnablePostProcess", 1);
    if method.is_null() {
        return;
    }
    let mut off: u8 = 0;
    let ok = m.invoke(method, camera as mono::P, &mut [(&raw mut off).cast()]).is_some();
    let mut seen = CAMERA.lock().unwrap();
    if *seen != camera {
        *seen = camera;
        log!("tombraider: post-processing and volumetric fog off ({})", if ok { "ok" } else { "threw" });
    }
}
