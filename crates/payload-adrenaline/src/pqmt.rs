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

/// Where `player`'s finger is: its gun's place in the game window, frame coordinates.
fn finger(player: usize) -> Option<(i32, i32)> {
    let (fx, fy) = gun::gun_of_player_in_picture(player)?;
    let (_, _, w, h) = gun::client_area(gun::find_window())?;
    let (_, rh) = resolution();
    Some(((fx * w) as i32, rh - ((1.0 - fy) * h) as i32))
}

/// Attaches the calling thread to the game's Mono domain: Unity 4's Mono needs it for the
/// game's callbacks (a thread of its own, unattached: a crash in the callback's wrapper). The
/// thread must start after Mono's load: an earlier one has no slot in its static TLS (a crash in
/// `mono_thread_attach`).
fn attach() -> bool {
    let domain = DOMAIN.load(Ordering::Relaxed);
    let Some(f) = mono_export(c"mono_thread_attach").filter(|_| domain != 0) else {
        return false;
    };
    let thread_attach: unsafe extern "C" fn(P) -> P = unsafe { std::mem::transmute(f) };
    !unsafe { thread_attach(domain as P) }.is_null()
}

/// Sends the players' fingers to the game.
fn touch_thread() {
    let mut attached = false;
    let mut down = [false; PLAYERS];
    let mut last = [(0, 0); PLAYERS];
    let mut frame_id = 0i32;
    let mut sent = false;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let callback = POINT_FRAME.load(Ordering::Relaxed);
        if callback == 0 || !CONNECTED.load(Ordering::Relaxed) {
            down = [false; PLAYERS];
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
        for player in 0..PLAYERS {
            let at = if held(player, Native::Touch) { finger(player) } else { None };
            let (event, (x, y)) = match (at, down[player]) {
                (Some(at), false) => (TP_DOWN, at),
                (Some(at), true) => (TP_MOVE, at),
                (None, true) => (TP_UP, last[player]),
                (None, false) => continue,
            };
            down[player] = event != TP_UP;
            last[player] = (x, y);
            points.push(TouchPoint { point_event: event, id: player as u16, x, y, dx: 16, dy: 16 });
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
