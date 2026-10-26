//! The "Boys" cabinet (Skull of Shadow: Unity 2021.3, Mono, 64-bit): `WAL_ARCADEPC_BOARD=boys`.
//!
//! The game imports its hardware through Mono (`payload-common`'s `mono_imports.rs`), resolved
//! to the payload's:
//!
//! - **Board** (`BoysUSB.dll`, hidapi: `hid_open`, `hid_write`, `hid_read`, `hid_close`): 32-byte
//!   reports, `AA`, the inputs at their `StreamingAssets/Top/TopUart.ini` places (bit n of a
//!   byte: `1 << (n - 1)`), the frame count at 30, the sum's complement of the 31 bytes before at
//!   31. Byte 1 the operator's keys (up 1, right 2, left / service coin 3, menu / down 4), byte 2
//!   the coins (player n: bit 8 - n), byte 3 the triggers, byte 4 the start keys (same bits),
//!   byte 10 the guns' lasers seen (`GunShot`, player n: bit 4 - n). The PC's reports (lamps,
//!   motors, tickets, the lasers' timing) are ignored.
//! - **Gun camera** (`JHCap2.dll`): a camera filming the screen, each gun's laser spot found in
//!   its pictures when the board says that gun's laser is on (`GunShot`). The camera answers
//!   black 320x240 pictures (no camera error); the game's tracking of a laser seen
//!   (`Boys.CamGun3.GunLogic.ProcessRayTracking`) is replaced by the payload's, which puts each
//!   gun's place on the picture (0..1 from the top left) where the tracking puts the spot found
//!   (`RayPointTread`, seen 0.3 s: `IsGetRayPoint`); the board reports player 1's laser on all
//!   the time. The dump's aim calibration (`TopData.db`, `UartMouse_5_<player>`: the service
//!   menu's five targets at 25 % / 75 % of the screen, scale 0.5) maps it unchanged.
//! - **Dongle.** The developer's switch `C:\Boys` (the HASP checks and the licence time
//!   skipped): the payload creates it.
//!
//! Natives: `trigger start coin` (each player), `menu up left right` (the operator's).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};

const PLAYERS: usize = 4;
const REPORT: usize = 32;
const CAM_W: i32 = 320;
const CAM_H: i32 = 240;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator's key: its bit of byte 1.
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("up", Native::Key(0x01)),
    ("right", Native::Key(0x02)),
    ("left", Native::Key(0x04)),
    ("menu", Native::Key(0x08)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "menu"),
    ("down", "menu"),
    ("up", "up"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

// The board

static FRAMES: AtomicU8 = AtomicU8::new(0);

/// The board's report: the inputs now.
fn report() -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[0] = 0xAA;
    for player in 0..PLAYERS {
        let bit = 0x80 >> player;
        map().for_each_pressed(&input(player), |n| match n {
            Native::Trigger => r[3] |= bit,
            Native::Start => r[4] |= bit,
            Native::Coin => r[2] |= bit,
            Native::Key(key) if player == 0 => r[1] |= key,
            Native::Key(_) => {}
        });
    }
    // player 1's laser on: the tracking runs (the payload's, every gun)
    r[10] |= 0x08;
    r[30] = FRAMES.fetch_add(1, Ordering::Relaxed);
    r[31] = !r[..31].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    r
}

/// The device `hid_open` gives: any non-null pointer.
static DEVICE: u8 = 0;

extern "C" fn hid_open(vendor: u16, product: u16, _serial: *const c_void) -> *const c_void {
    log!("boys: hid_open {vendor:04X}:{product:04X}");
    &DEVICE as *const u8 as *const c_void
}

extern "C" fn hid_close(_device: *const c_void) {}

extern "C" fn hid_write(_device: *const c_void, _data: *const u8, length: i32) -> i32 {
    length
}

extern "C" fn hid_read(_device: *const c_void, data: *mut u8, length: i32) -> i32 {
    let r = report();
    let n = (length.max(0) as usize).min(REPORT);
    unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), data, n) };
    n as i32
}

// The camera: black pictures

extern "system" fn cam_ok() -> i32 {
    0
}

extern "system" fn cam_count(count: *mut i32) -> i32 {
    unsafe { *count = 1 };
    0
}

extern "system" fn cam_image_size(_index: i32, width: *mut i32, height: *mut i32) -> i32 {
    unsafe {
        *width = CAM_W;
        *height = CAM_H;
    }
    0
}

extern "system" fn cam_buffer_size(_index: i32, size: *mut i32, _option: i32) -> i32 {
    unsafe { *size = CAM_W * CAM_H };
    0
}

extern "system" fn cam_set_resolution(_index: i32, _mode: i32, width: *mut i32, height: *mut i32) -> i32 {
    cam_image_size(0, width, height)
}

/// A picture: black, at the camera's pace (the game asks again at once).
extern "system" fn cam_query_image(_index: i32, buffer: *mut u8, size: *mut i32, _option: i32) -> i32 {
    std::thread::sleep(Duration::from_millis(16));
    let n = (CAM_W * CAM_H) as usize;
    unsafe {
        std::ptr::write_bytes(buffer, 0, n);
        *size = n as i32;
    }
    0
}

// The laser tracking: each gun's place on the picture

/// `GunLogic`'s fields: `RayPointTread` (`Vector3[]`), `IsGetRayPoint` (`float[]`).
static RAY_POINTS: AtomicUsize = AtomicUsize::new(0);
static RAY_SEEN: AtomicUsize = AtomicUsize::new(0);

/// Where a managed array's elements start (after its object header, bounds and length).
const ARRAY_DATA: usize = 4 * size_of::<usize>();

/// `GunLogic.ProcessRayTracking(index, ray, recursion)`: every gun on the picture tracked.
extern "C" fn track(this: *mut u8, _index: i32, _ray: i32, _recursion: i32) {
    let (points, seen) = (RAY_POINTS.load(Ordering::Relaxed), RAY_SEEN.load(Ordering::Relaxed));
    let points = unsafe { *(this.add(points) as *const *mut u8) };
    let seen = unsafe { *(this.add(seen) as *const *mut u8) };
    if points.is_null() || seen.is_null() {
        return;
    }
    for player in 0..PLAYERS {
        if let Some((x, y)) = gun::gun_of_player_in_picture(player) {
            unsafe {
                let p = points.add(ARRAY_DATA + player * 12) as *mut f32;
                *p = x;
                *p.add(1) = y;
                *p.add(2) = 0.0;
                *(seen.add(ARRAY_DATA + player * 4) as *mut f32) = 0.3;
            }
        }
    }
    // the tracking thread's pace
    std::thread::sleep(Duration::from_millis(8));
}

/// Replaces the tracking once the game's domain runs its camera (`GunLogic`'s instance).
fn replace_tracking() {
    use wal_payload_common::mono::mono;
    for _ in 0..3000 {
        std::thread::sleep(Duration::from_millis(100));
        let Some(m) = mono() else { continue };
        if !m.attach() {
            continue;
        }
        let logic = m.class(m.image("Assembly-CSharp"), "Boys.CamGun3", "GunLogic");
        if logic.is_null() {
            continue;
        }
        let instance = m.static_addr(logic, m.field(logic, "_instance"));
        if instance.is_null() || unsafe { *(instance as *const usize) } == 0 {
            continue;
        }
        RAY_POINTS.store(m.offset(m.field(logic, "RayPointTread")), Ordering::Relaxed);
        RAY_SEEN.store(m.offset(m.field(logic, "IsGetRayPoint")), Ordering::Relaxed);
        let ok = m.replace(m.method(logic, "ProcessRayTracking", 3), track as *const () as usize);
        log!("boys: the laser tracking is the payload's ({ok})");
        return;
    }
    log!("boys: the camera's tracking not found");
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match module {
        "boysusb.dll" => match name {
            "hid_open" => f(hid_open as *const () as usize),
            "hid_close" => f(hid_close as *const () as usize),
            "hid_write" => f(hid_write as *const () as usize),
            "hid_read" => f(hid_read as *const () as usize),
            _ => None,
        },
        // Mono asks the Unicode name (`...W`) first: the same function
        "jhcap2.dll" => match name.strip_suffix(['W', 'A']).unwrap_or(name) {
            "CameraGetCount" => f(cam_count as *const () as usize),
            "CameraGetImageSize" | "CameraGetResolutionMax" => f(cam_image_size as *const () as usize),
            "CameraGetImageBufferSize" => f(cam_buffer_size as *const () as usize),
            "CameraSetResolution" => f(cam_set_resolution as *const () as usize),
            "CameraQueryImage" => f(cam_query_image as *const () as usize),
            _ if name.starts_with("Camera") => f(cam_ok as *const () as usize),
            _ => None,
        },
        _ => None,
    }
}

pub fn init() {
    // the developer's switch: no dongle, no licence time
    if std::fs::metadata("C:\\Boys").is_err() {
        match std::fs::write("C:\\Boys", b"") {
            Ok(()) => log!("boys: C:\\Boys created"),
            Err(e) => log!("boys: C:\\Boys: {e}"),
        }
    }
    mono_imports::hook(resolve);
    std::thread::spawn(replace_tracking);
    log!("boys: board (BoysUSB), gun camera (JHCap2)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum() {
        let mut r = [0u8; REPORT];
        r[0] = 0xAA;
        r[31] = !r[..31].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        assert_eq!(r[31], !0xAA);
    }
}
