//! The PQ Labs multi-touch frame of Skylanders Cloud Patrol: `PQMTClient.dll`, the client of
//! the frame's server (TCP 127.0.0.1:21555), replaced.
//!
//! `PqmtScreenGestures` connects (`ConnectServer`, `SendRequest`), asks the frame's resolution
//! (`GetServerResolution`: its callback) and gets the touch points through its point frame
//! callback (`SetOnReceivePointFrame`, cdecl): `TouchPoint { point_event (0 down, 1 move, 2 up),
//! id, x, y, dx, dy }`, from the frame's own thread as here. A finger missing from a frame is
//! lifted.
//!
//! Each player's finger (id = the player) is down where the gun points while its `touch` is
//! held, lifted when released or off the picture; frames every 16 ms while a finger is down.
//!
//! Coordinates: the game maps a point (x right, y down from the top left) to Unity's screen
//! (`x`, `height - y`) scaled by the desktop's size divided by the frame's resolution, in
//! integers: the reported resolution is 0.6 times the screen's (the scale 1 for a desktop
//! between 0.6 and 1.2 times the screen), the points in the game window's client pixels.
//!
//! Infinity Blade FX (Unreal Engine 3, no Mono): the game imports the client
//! ([`hook_imports`]), never asks the resolution; the points are the game window's client
//! pixels. Its sword is swung by swipes: `WAL_GUN_SWIPE=<gain>` (the profile's "Gun swipes"
//! option) moves the finger `gain` times as far as the gun from where the trigger was pulled,
//! so a short flick of a gun held on the screen is a full swipe, in the gun's direction.
//! Fixed fingers, wherever the gun points (ids: the player + 4, 8, 12): `shield` (the profile's
//! right click / gun's second button) on the block zone (`SwordShieldBlock`: x 0.41-0.59,
//! y 0.80-1.0), `dodge_left` / `dodge_right` (left / right: the arrow keys, the gun's d-pad) on
//! the dodge arrows (`SwordDodgeLeft` / `Right`: the bottom corners, 0.11 x 0.12).
//!
//! Fruit Ninja FX 2 (native, the same imports; it asks the resolution but takes client pixels):
//! the fruit is cut by swipes, a light gun's shot is a point. `WAL_GUN_SLICE=<length>` (the
//! profile's "Gun slice" option, a fraction of the picture's height) makes each trigger pull a
//! slice ([`Slicer`]): down at the aim, out half the length, back through the aim to the other
//! side, back to the aim, in 8 frames (~130 ms), whether the trigger is still held or not; held,
//! the finger then follows the gun.

use std::ffi::{CStr, c_void};
use std::sync::{Mutex, Once};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{gun, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use crate::{Native, PLAYERS, held};

type P = *mut c_void;

const TP_DOWN: u16 = 0;
const TP_MOVE: u16 = 1;
const TP_UP: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct TouchPoint {
    point_event: u16,
    id: u16,
    x: i32,
    y: i32,
    dx: u16,
    dy: u16,
}

type PointFrame = unsafe extern "C" fn(frame_id: i32, time_stamp: i32, count: i32, points: *const TouchPoint, object: P);
type Resolution = unsafe extern "C" fn(x: i32, y: i32, object: P);

/// The game's point frame callback and its object.
static POINT_FRAME: AtomicUsize = AtomicUsize::new(0);
static POINT_FRAME_OBJECT: AtomicUsize = AtomicUsize::new(0);
/// The game's Mono domain (the point frame callback's), attached to by the touch thread.
static DOMAIN: AtomicUsize = AtomicUsize::new(0);
/// Connected (between `ConnectServer` and `DisconnectServer`).
static CONNECTED: AtomicBool = AtomicBool::new(false);
/// The other callbacks' (server break, errors, device info, gestures), returned when replaced.
static OTHERS: Mutex<[usize; 4]> = Mutex::new([0; 4]);
/// A native game (the executable imports the client): no Mono, client pixel points.
static NATIVE: AtomicBool = AtomicBool::new(false);

/// `WAL_GUN_SWIPE`: how many times the gun's moves since the trigger the finger moves (1: as
/// far, where the gun points).
fn swipe_gain() -> f32 {
    static GAIN: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *GAIN.get_or_init(|| std::env::var("WAL_GUN_SWIPE").ok().and_then(|v| v.trim().parse().ok()).filter(|g: &f32| *g >= 1.0).unwrap_or(1.0))
}

/// `WAL_GUN_SLICE`: the length of the slice a trigger pull draws (a fraction of the picture's
/// height; 0: none, the finger only follows the gun).
fn slice_length() -> f32 {
    static LENGTH: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *LENGTH.get_or_init(|| std::env::var("WAL_GUN_SLICE").ok().and_then(|v| v.trim().parse().ok()).filter(|l: &f32| *l >= 0.0).unwrap_or(0.0).min(1.0))
}

/// The frames of a slice (16 ms each).
const SLICE_FRAMES: usize = 8;
/// The gun's places kept for its direction (16 ms apart).
const TRAIL: usize = 6;

/// A player's slices: at a trigger pull the finger goes down where the gun points, out half the
/// slice's length one way, back through the aim to the other side and back to the aim, in
/// [`SLICE_FRAMES`] frames: a light gun's shot is a slice through the aim (the fruit there cut),
/// a quick pull too, and a button there gets its finger down and up on it. The slice's direction
/// is the gun's move over its last [`TRAIL`] frames, else diagonals in turn.
#[derive(Default)]
struct Slicer {
    /// The gun's last places on the picture.
    trail: std::collections::VecDeque<(f32, f32)>,
    /// The slice: its aim, half its extent (fractions of the picture), its next frame.
    slice: Option<((f32, f32), (f32, f32), usize)>,
    /// The diagonal of the next still gun's slice.
    rising: bool,
    /// The slice's last frame was the previous one.
    ended: bool,
    finished: bool,
}

impl Slicer {
    fn track(&mut self, aim: Option<(f32, f32)>) {
        match aim {
            Some(at) => {
                if self.trail.len() == TRAIL {
                    self.trail.pop_front();
                }
                self.trail.push_back(at);
            }
            None => self.trail.clear(),
        }
    }

    fn start(&mut self, aim: Option<(f32, f32)>, length: f32) {
        let Some(aim) = aim else { return };
        // pixels: the picture's aspect
        let aspect = gun::client_area(gun::find_window()).map_or(16.0 / 9.0, |(_, _, w, h)| w / h);
        let moved = match (self.trail.front(), self.trail.back()) {
            (Some(a), Some(b)) => ((b.0 - a.0) * aspect, b.1 - a.1),
            _ => (0.0, 0.0),
        };
        let norm = moved.0.hypot(moved.1);
        let dir = if norm > 0.02 {
            (moved.0 / norm, moved.1 / norm)
        } else {
            self.rising = !self.rising;
            (0.866, if self.rising { -0.5 } else { 0.5 })
        };
        let half = length / 2.0;
        self.slice = Some((aim, (dir.0 * half / aspect, dir.1 * half), 0));
    }

    /// The finger's place in this frame of the slice, `None` without one.
    fn next(&mut self) -> Option<(f32, f32)> {
        let Some((aim, half, frame)) = self.slice.as_mut() else {
            self.ended = std::mem::take(&mut self.finished);
            return None;
        };
        self.ended = false;
        // along the stroke: 0 (the aim), out to 1, back through 0 to -1, back to 0
        let t = *frame as f32 / SLICE_FRAMES as f32;
        let s = if t < 0.25 { 4.0 * t } else if t < 0.75 { 2.0 - 4.0 * t } else { 4.0 * t - 4.0 };
        let at = ((aim.0 + half.0 * s).clamp(0.0, 0.999), (aim.1 + half.1 * s).clamp(0.0, 0.999));
        *frame += 1;
        if *frame > SLICE_FRAMES {
            self.slice = None;
            self.finished = true;
        }
        Some(at)
    }

    /// The slice ended in the previous frame (the finger, still down, follows the gun from here):
    /// valid after [`Slicer::next`].
    fn ended(&self) -> bool {
        self.ended
    }
}

/// The reported resolution: 0.6 times the screen's.
fn resolution() -> (i32, i32) {
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    (((w * 3 + 4) / 5).max(1), ((h * 3 + 4) / 5).max(1))
}

extern "C" fn connect_server(_ip: *const u8, port: i32) -> i32 {
    log!("pqmt: ConnectServer(port {port}): the touch frame");
    CONNECTED.store(true, Ordering::Relaxed);
    0
}

extern "C" fn disconnect_server() -> i32 {
    log!("pqmt: DisconnectServer");
    CONNECTED.store(false, Ordering::Relaxed);
    0
}

extern "C" fn send_request(_request: P) -> i32 {
    0
}

extern "C" fn send_threshold(_threshold: i32) -> i32 {
    0
}

extern "C" fn set_raw_data_resolution(_x: i32, _y: i32) -> i32 {
    0
}

extern "C" fn get_server_resolution(callback: usize, object: P) -> i32 {
    let (w, h) = resolution();
    log!("pqmt: resolution {w}x{h}");
    if callback != 0 {
        let callback: Resolution = unsafe { std::mem::transmute(callback) };
        unsafe { callback(w, h, object) };
    }
    0
}

/// A `mono.dll` export.
fn mono_export(name: &CStr) -> Option<usize> {
    let mono = unsafe { GetModuleHandleW(windows_sys::w!("mono.dll")) };
    (!mono.is_null()).then(|| unsafe { GetProcAddress(mono, name.as_ptr().cast()) }).flatten().map(|f| f as usize)
}

extern "C" fn set_on_receive_point_frame(callback: usize, object: P) -> usize {
    // the game's managed code calls: its thread is in the game's domain
    if let Some(f) = mono_export(c"mono_domain_get") {
        let domain_get: unsafe extern "C" fn() -> P = unsafe { std::mem::transmute(f) };
        DOMAIN.store(unsafe { domain_get() } as usize, Ordering::Relaxed);
    }
    // a thread started after Mono's load (the payload's earlier threads lack its TLS)
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(touch_thread);
    });
    POINT_FRAME_OBJECT.store(object as usize, Ordering::Relaxed);
    POINT_FRAME.swap(callback, Ordering::Relaxed)
}

macro_rules! set_other {
    ($name:ident, $i:literal) => {
        extern "C" fn $name(callback: usize, _object: P) -> usize {
            std::mem::replace(&mut OTHERS.lock().unwrap()[$i], callback)
        }
    };
}

set_other!(set_on_server_break, 0);
set_other!(set_on_receive_error, 1);
set_other!(set_on_get_device_info, 2);
set_other!(set_on_receive_gesture, 3);

extern "C" fn get_gesture_name(_gesture: P) -> *const u8 {
    c"".as_ptr().cast()
}

/// Where `player`'s finger is: its place in the game window (`at`, fractions from the top left),
/// frame coordinates.
fn finger((fx, fy): (f32, f32)) -> Option<(i32, i32)> {
    let (_, _, w, h) = gun::client_area(gun::find_window())?;
    if NATIVE.load(Ordering::Relaxed) {
        return Some(((fx * w) as i32, (fy * h) as i32));
    }
    let (_, rh) = resolution();
    Some(((fx * w) as i32, rh - ((1.0 - fy) * h) as i32))
}

/// The fixed fingers: their native and place (the zone's centre, fractions of the picture).
const FIXED: [(Native, (f32, f32)); 3] = [(Native::Shield, (0.5, 0.9)), (Native::DodgeLeft, (0.055, 0.94)), (Native::DodgeRight, (0.945, 0.94))];
/// Fingers: each player's, then each fixed finger of each player.
const FINGERS: usize = PLAYERS * (1 + FIXED.len());

/// The finger's place for a gun at `gun` pulled at `anchor`: its moves since times the swipe
/// gain, on the picture.
fn swiped(gun: (f32, f32), anchor: (f32, f32), gain: f32) -> (f32, f32) {
    let at = |g: f32, a: f32| (a + (g - a) * gain).clamp(0.0, 0.999);
    (at(gun.0, anchor.0), at(gun.1, anchor.1))
}

/// Attaches the calling thread to the game's Mono domain: Unity 4's Mono needs it for the
/// game's callbacks (a thread of its own, unattached: a crash in the callback's wrapper). The
/// thread must start after Mono's load: an earlier one has no slot in its static TLS (a crash in
/// `mono_thread_attach`).
fn attach() -> bool {
    if NATIVE.load(Ordering::Relaxed) {
        return true;
    }
    let domain = DOMAIN.load(Ordering::Relaxed);
    let Some(f) = mono_export(c"mono_thread_attach").filter(|_| domain != 0) else {
        return false;
    };
    let thread_attach: unsafe extern "C" fn(P) -> P = unsafe { std::mem::transmute(f) };
    !unsafe { thread_attach(domain as P) }.is_null()
}

/// Finger `id`'s point for this frame, at `at` (`None`: lifted): down, moved or up, none if it
/// stays lifted.
fn touch_point(id: usize, at: Option<(i32, i32)>, down: &mut [bool], last: &mut [(i32, i32)]) -> Option<TouchPoint> {
    let (event, (x, y)) = match (at, down[id]) {
        (Some(at), false) => (TP_DOWN, at),
        (Some(at), true) => (TP_MOVE, at),
        (None, true) => (TP_UP, last[id]),
        (None, false) => return None,
    };
    down[id] = event != TP_UP;
    last[id] = (x, y);
    Some(TouchPoint { point_event: event, id: id as u16, x, y, dx: 16, dy: 16 })
}

/// Sends the players' fingers to the game.
fn touch_thread() {
    let mut attached = false;
    let mut down = [false; FINGERS];
    let mut last = [(0, 0); FINGERS];
    let mut anchor = [(0.0, 0.0); PLAYERS];
    let gain = swipe_gain();
    let slice_len = slice_length();
    let mut slicer: [Slicer; PLAYERS] = Default::default();
    let mut was_pressed = [false; PLAYERS];
    let mut frame_id = 0i32;
    let mut sent = false;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let callback = POINT_FRAME.load(Ordering::Relaxed);
        if callback == 0 || !CONNECTED.load(Ordering::Relaxed) {
            down = [false; FINGERS];
            continue;
        }
        if !attached {
            attached = attach();
            if !attached {
                continue;
            }
            log!("pqmt: touch thread attached to the game's domain");
        }
        let mut points = Vec::new();
        for id in 0..FINGERS {
            let player = id % PLAYERS;
            if id >= PLAYERS {
                let (native, place) = FIXED[id / PLAYERS - 1];
                let at = if held(player, native) { finger(place) } else { None };
                if let Some(point) = touch_point(id, at, &mut down, &mut last) {
                    points.push(point);
                }
                continue;
            }
            let pressed = held(player, Native::Touch);
            let aim = gun::gun_of_player_in_picture(player);
            slicer[player].track(aim);
            if pressed && !was_pressed[player] && slice_len > 0.0 {
                slicer[player].start(aim, slice_len);
            }
            was_pressed[player] = pressed;
            // a slice runs to its end, the trigger held or not
            let sliced = slicer[player].next();
            let gun = if pressed && sliced.is_none() { aim } else { None };
            if let Some(gun) = gun
                && (!down[player] || slicer[player].ended())
            {
                anchor[player] = gun;
            }
            let at = sliced.or_else(|| gun.map(|gun| swiped(gun, anchor[player], gain))).and_then(finger);
            if pressed && at.is_none() {
                static ONCE: Once = Once::new();
                ONCE.call_once(|| log!("pqmt: P{} touch held off the picture (gun {gun:?}, window {:?})", player + 1, gun::client_area(gun::find_window())));
            }
            if let Some(point) = touch_point(id, at, &mut down, &mut last) {
                points.push(point);
            }
        }
        if points.is_empty() {
            continue;
        }
        if !sent {
            sent = true;
            log!("pqmt: first touch at {},{}", points[0].x, points[0].y);
        }
        frame_id = frame_id.wrapping_add(1);
        let callback: PointFrame = unsafe { std::mem::transmute(callback) };
        let object = POINT_FRAME_OBJECT.load(Ordering::Relaxed) as P;
        unsafe { callback(frame_id, frame_id.wrapping_mul(16), points.len() as i32, points.as_ptr(), object) };
    }
}

/// The client's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "ConnectServer" => connect_server as *const () as usize,
        "DisconnectServer" => disconnect_server as *const () as usize,
        "SendRequest" => send_request as *const () as usize,
        "SendThreshold" => send_threshold as *const () as usize,
        "SetRawDataResolution" => set_raw_data_resolution as *const () as usize,
        "GetServerResolution" => get_server_resolution as *const () as usize,
        "SetOnReceivePointFrame" => set_on_receive_point_frame as *const () as usize,
        "SetOnServerBreak" => set_on_server_break as *const () as usize,
        "SetOnReceiveError" => set_on_receive_error as *const () as usize,
        "SetOnGetDeviceInfo" => set_on_get_device_info as *const () as usize,
        "SetOnReceiveGesture" => set_on_receive_gesture as *const () as usize,
        "GetGestureName" => get_gesture_name as *const () as usize,
        _ => return None,
    };
    Some(f)
}

pub fn init() {
    log!("pqmt: PQ Labs touch frame");
}

/// A native game's imports of the client pointed at ours: whether the executable imports it.
pub fn hook_imports() -> bool {
    const NAMES: [&str; 12] = [
        "ConnectServer",
        "DisconnectServer",
        "SendRequest",
        "SendThreshold",
        "SetRawDataResolution",
        "GetServerResolution",
        "SetOnReceivePointFrame",
        "SetOnServerBreak",
        "SetOnReceiveError",
        "SetOnGetDeviceInfo",
        "SetOnReceiveGesture",
        "GetGestureName",
    ];
    let mut hooked = 0;
    for name in NAMES {
        if let Some(f) = resolve(name)
            && unsafe { wal_payload_common::iat::hook("PQMTClient.dll", name, f) }.is_some()
        {
            hooked += 1;
        }
    }
    if hooked > 0 {
        NATIVE.store(true, Ordering::Relaxed);
        log!("pqmt: the executable's {hooked} client imports hooked (native game), swipe gain {}, slice {}", swipe_gain(), slice_length());
    }
    hooked > 0
}
