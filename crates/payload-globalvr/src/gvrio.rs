//! Global VR "Zeus" cabinets (Need for Speed Carbon): `GvrIO.dll`, the message library between
//! the game and the cabinet's controls.
//!
//! The original reads its configuration (`Config\GvrIO.xml` under the cabinet's `GameRoot`),
//! the USBIO board, the wheel (its `Steering.dll` plug-in: Happ force feedback wheel over HID)
//! and the keyboard, and loads its plug-ins (dongle, card dispenser). This one is the cabinet
//! fed by the virtual sticks, installed under the same name (profile `payload_install`).
//!
//! The game creates a `cGvrIO`, calls `Initialize(device type, hwnd, callback)` and talks with
//! messages both ways:
//!
//! | offset | content |
//! |---|---|
//! | 0x00 | u32 kind |
//! | 0x04 | u32 message id |
//! | 0x08 | i32 value (axis index, lamp value...) |
//! | 0x0C | pointer (name of the event, lamp or button) |
//! | 0x14 | u8 handled (set by the receiver) |
//!
//! * The callback gets id 0 once the controls are ready (the game polls nothing before), then
//!   id 0x0B for each button change: the event name, `onGvrStartButtonDown`,
//!   `onGvrNosButtonUp`...
//! * The button events come from a thread (as the original's), 60 times a second.
//! * `GvrIOMessageSend(message)` answers a pointer to an i32: id 1 is the race's per-frame
//!   update, id 5 the axes (value 0
//!   wheel -127..=127, 1 gas 0..=255, 2 brake 0..=255) and id 0x42 a button by name (the H
//!   shifter of the manual transmission: `ShifterLeft` / `Right` / `Up` / `Down`; the
//!   frontend's `IsButtonDown`: `Music`...). The other
//!   ids (lamps by name, force feedback settings) are answered 0.
//! * The cabinet frontend (its `ZeusIOPlugIn.dll`) also asks the original's plug-ins: the
//!   dongle (`Dongle.dll`: 0x23 present, answered by the callback's 0x24 too, 0x25 present, 0x26
//!   region, 0x27 game, 0x28 version, 0x29 HASP id) and the card dispenser (`CardDispenser.dll`:
//!   0x3E status, a value, 1 = no card), and GvrIO's own board information its startup checks
//!   read: 0x60 board name, 0x2F / 0x30 version, 0x6F product id, 0x2A bad buttons, 0x39 / 0x3A
//!   axis (value) minimum / maximum (the calibration range), 0x54 axis position, 0x4D dongles.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};

#[repr(C)]
pub struct Message {
    kind: u32,
    id: u32,
    value: i32,
    data: *const u8,
    _reserved: u32,
    handled: u8,
}

type Callback = unsafe extern "C" fn(*mut Message) -> *mut c_void;

const ID_READY: u32 = 0;
const ID_UPDATE: u32 = 1;
const ID_AXIS: u32 = 5;
const ID_EVENT: u32 = 0x0B;
const ID_BUTTON: u32 = 0x42;
const ID_DONGLE_CHECK: u32 = 0x23;
const ID_DONGLE_CHECKED: u32 = 0x24;
const ID_DONGLE_PRESENT: u32 = 0x25;
const ID_DONGLE_REGION: u32 = 0x26;
const ID_DONGLE_GAME: u32 = 0x27;
const ID_DONGLE_VERSION: u32 = 0x28;
const ID_DONGLE_ID: u32 = 0x29;
const ID_DISPENSER_STATUS: u32 = 0x3E;
const ID_BAD_BUTTONS: u32 = 0x2A;
const ID_BOARD_MAJOR: u32 = 0x2F;
const ID_BOARD_MINOR: u32 = 0x30;
const ID_AXIS_MIN: u32 = 0x39;
const ID_AXIS_MAX: u32 = 0x3A;
const ID_DONGLE_COUNT: u32 = 0x4D;
const ID_AXIS_CALIBRATED: u32 = 0x54;
const ID_BOARD_NAME: u32 = 0x60;
const ID_BOARD_PRODUCT: u32 = 0x6F;

/// The board the frontend accepts: a GvrIOmini 1.x (minor below 5), a known product id.
const BOARD_NAME: &CStr = c"GvrIOmini";
const BOARD_VERSION: (i32, i32) = (1, 2);
const BOARD_PRODUCT: i32 = 7021;

/// The dongle's answers, what the frontend checks (its DongleError.am script: game, version,
/// region "XX"; the game accepts XX too).
static PRESENT: u8 = 1;
static DONGLE_ID: u32 = 0x5741_4c00;
const REGION: &CStr = c"XX";
const GAME: &CStr = c"CARBON";
const VERSION: &CStr = c"1.0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u32);

const START: u32 = 0x1;
const VIEW: u32 = 0x2;
const LOOKBACK: u32 = 0x4;
const NOS: u32 = 0x8;
const MUSIC: u32 = 0x10;
const EBRAKE: u32 = 0x20;
const SHIFT_DOWN: u32 = 0x40;
const SHIFT_UP: u32 = 0x80;
const OPERATOR: u32 = 0x100;

const NATIVES: &[(&str, Native)] = &[
    ("start", Native(START)),
    ("view", Native(VIEW)),
    ("lookback", Native(LOOKBACK)),
    ("nos", Native(NOS)),
    ("music", Native(MUSIC)),
    ("ebrake", Native(EBRAKE)),
    ("shiftdown", Native(SHIFT_DOWN)),
    ("shiftup", Native(SHIFT_UP)),
    ("operator", Native(OPERATOR)),
];

/// Virtual stick -> cabinet, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "view"),
    ("b2", "music"),
    ("b3", "nos"),
    ("b4", "lookback"),
    ("b5", "shiftdown"),
    ("b6", "shiftup"),
    ("b7", "ebrake"),
    ("test", "operator"),
];

/// The buttons sent as events, and their names: `onGvr<name>ButtonDown` / `Up` (the game) and
/// `onGvr<name>Pressed` (the frontend's GvrIO.xml events).
const EVENTS: &[(u32, &str)] = &[
    (START, "Start"),
    (VIEW, "View"),
    (LOOKBACK, "LookBack"),
    (NOS, "Nos"),
    (MUSIC, "Music"),
    (EBRAKE, "Ebrake"),
    (OPERATOR, "Operator"),
];

/// The frontend's menu selection: an index the wheel steps (0x0F / 0x10 read X / Y, 0x11 /
/// 0x12 set them, 0x13 / 0x14 set their range), each change sent as `onSelectionChanged`.
static SELECTION_X: AtomicI32 = AtomicI32::new(0);
static SELECTION_Y: AtomicI32 = AtomicI32::new(0);
static RANGE_X: AtomicI32 = AtomicI32::new(0);
static RANGE_Y: AtomicI32 = AtomicI32::new(0);
/// Updates until the held wheel steps the selection again.
static REPEAT: AtomicI32 = AtomicI32::new(0);
/// The wheel's direction at the last update (-1, 0, 1).
static WHEEL_DIR: AtomicI32 = AtomicI32::new(0);
const ID_SELECTION_X: u32 = 0x0F;
const ID_SELECTION_Y: u32 = 0x10;
const ID_SET_SELECTION_X: u32 = 0x11;
const ID_SET_SELECTION_Y: u32 = 0x12;
const ID_RANGE_X: u32 = 0x13;
const ID_RANGE_Y: u32 = 0x14;
const ID_AXIS_RAW: u32 = 0x06;

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static CALLBACK: AtomicUsize = AtomicUsize::new(0);
static BUTTONS: AtomicU32 = AtomicU32::new(0);
/// The manual transmission's gear, 1..=6.
static GEAR: AtomicU32 = AtomicU32::new(1);
/// The answers: the game reads them right after the call.
static ANSWER: AtomicI32 = AtomicI32::new(0);
static ZERO: AtomicI32 = AtomicI32::new(0);

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// Calls the game's callback with this message.
fn post(id: u32, name: Option<&CStr>) {
    post_value(id, 0, name);
}

fn post_value(id: u32, value: i32, name: Option<&CStr>) {
    let callback = CALLBACK.load(Ordering::Relaxed);
    if callback == 0 {
        return;
    }
    let callback: Callback = unsafe { std::mem::transmute(callback) };
    let mut message = Message {
        kind: 1,
        id,
        value,
        data: name.map_or(std::ptr::null(), |n| n.as_ptr().cast()),
        _reserved: 0,
        handled: 0,
    };
    unsafe { callback(&mut message) };
}

/// The H shifter's position of a gear: (left, right, up, down); 3rd and 4th in the middle.
fn shifter(gear: u32) -> (bool, bool, bool, bool) {
    (gear <= 2, gear >= 5, gear % 2 == 1, gear % 2 == 0)
}

/// The per-frame update: the stick read, the button changes sent as events, the gear stepped.
fn update() {
    let stick: StickState = wal_payload_common::input(0);
    let mut buttons = 0;
    map().for_each_pressed(&stick, |Native(bit)| buttons |= bit);
    let old = BUTTONS.swap(buttons, Ordering::Relaxed);
    let pressed = buttons & !old;
    let mut gear = GEAR.load(Ordering::Relaxed);
    if pressed & SHIFT_UP != 0 && gear < 6 {
        gear += 1;
    }
    if pressed & SHIFT_DOWN != 0 && gear > 1 {
        gear -= 1;
    }
    if pressed & (SHIFT_UP | SHIFT_DOWN) != 0 {
        GEAR.store(gear, Ordering::Relaxed);
        log!("gvrio: gear {gear}");
    }
    for &(bit, name) in EVENTS {
        if (buttons ^ old) & bit != 0 {
            let down = buttons & bit != 0;
            let event = format!("onGvr{name}Button{}\0", if down { "Down" } else { "Up" });
            log!("gvrio: {}", event.trim_end_matches('\0'));
            post(ID_EVENT, CStr::from_bytes_with_nul(event.as_bytes()).ok());
            if down {
                let event = format!("onGvr{name}Pressed\0");
                post(ID_EVENT, CStr::from_bytes_with_nul(event.as_bytes()).ok());
            }
        }
    }
    select(axis(0));
}

/// Steps the menu selection with the wheel turned past half: once, then every 150 ms while held
/// (after 400 ms).
fn select(wheel: i32) {
    let dir = if wheel < -64 { -1 } else if wheel > 64 { 1 } else { 0 };
    let old = WHEEL_DIR.swap(dir, Ordering::Relaxed);
    let step = if dir == 0 {
        false
    } else if dir != old {
        REPEAT.store(24, Ordering::Relaxed);
        true
    } else if REPEAT.fetch_sub(1, Ordering::Relaxed) <= 1 {
        REPEAT.store(9, Ordering::Relaxed);
        true
    } else {
        false
    };
    if !step {
        return;
    }
    let range = RANGE_X.load(Ordering::Relaxed);
    let old = SELECTION_X.load(Ordering::Relaxed);
    let mut new = old + dir;
    if range > 0 {
        new = new.clamp(0, range - 1);
    }
    if new != old {
        SELECTION_X.store(new, Ordering::Relaxed);
        log!("gvrio: selection {new} of {range}");
        post(ID_EVENT, Some(c"onSelectionChanged"));
    }
}

/// Loaded in the cabinet frontend (not the game).
fn in_frontend() -> bool {
    static FRONTEND: OnceLock<bool> = OnceLock::new();
    *FRONTEND.get_or_init(|| crate::frontend_fit::exe_name() == "amplayer.exe")
}

fn axis(index: i32) -> i32 {
    let stick: StickState = wal_payload_common::input(0);
    match index {
        0 => (stick.axis(Axis::LeftX) as i32 * 127 / 32767).clamp(-127, 127),
        // the frontend's attract starts a game on the gas pedal only (its start event handler
        // does not run there): start presses the gas in the frontend (gas selects in its menus)
        1 if in_frontend() && BUTTONS.load(Ordering::Relaxed) & START != 0 => 255,
        1 => stick.axis_u8(Axis::Accel) as i32,
        2 => stick.axis_u8(Axis::Brake) as i32,
        _ => 0,
    }
}

fn button(name: &[u8]) -> i32 {
    static SEEN: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());
    {
        let mut seen = SEEN.lock().unwrap();
        if !seen.iter().any(|n| n == name) {
            seen.push(name.to_vec());
            log!("gvrio: button {:?} asked", String::from_utf8_lossy(name));
        }
    }
    let (left, right, up, down) = shifter(GEAR.load(Ordering::Relaxed));
    let on = match name {
        b"ShifterLeft" => left,
        b"ShifterRight" => right,
        b"ShifterUp" => up,
        b"ShifterDown" => down,
        // the frontend's IsButtonDown (Music held with the gas enters a locked Career)
        _ => EVENTS.iter().any(|&(bit, n)| n.as_bytes() == name && BUTTONS.load(Ordering::Relaxed) & bit != 0),
    };
    on as i32
}

/// Loaded in Need for Speed Carbon's game: its input manager reads the cabinet only with a
/// DirectInput keyboard and mouse listed (the fake DirectInput of `WAL_DINPUT_DISABLE` has
/// none): an empty keyboard, and the mouse that comes with it.
pub(crate) fn init() {
    if crate::frontend_fit::exe_name().starts_with("carbonrelease") {
        wal_payload_common::dinput::set_keyboard(|_| {});
        log!("gvrio: DirectInput keyboard and mouse for the game's input manager");
    }
}

/// `cGvrIO::cGvrIO()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrio_new(this: *mut c_void) -> *mut c_void {
    this
}

/// `cGvrIO::~cGvrIO()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrio_drop(_this: *mut c_void) {
    CALLBACK.store(0, Ordering::Relaxed);
}

/// `bool cGvrIO::Initialize(GVR_DEVICE_TYPES, HWND, callback)`: the controls are ready.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrio_initialize(_this: *mut c_void, device: u32, _hwnd: usize, callback: usize) -> bool {
    log!("gvrio: initialize device type {device}, callback {callback:#x}");
    map();
    CALLBACK.store(callback, Ordering::Relaxed);
    post(ID_READY, None);
    // the original's message thread: the button events at 60 Hz, also in the frontend (the
    // per-frame update message is the race's only)
    static THREAD: std::sync::Once = std::sync::Once::new();
    THREAD.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(16));
                if CALLBACK.load(Ordering::Relaxed) != 0 {
                    update();
                }
            }
        });
    });
    true
}

/// `bool cGvrIO::MessageRegisterCallback(GVR_DEVICE_TYPES, callback)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrio_register_callback(_this: *mut c_void, _device: u32, callback: usize) -> bool {
    CALLBACK.store(callback, Ordering::Relaxed);
    true
}

/// `void *GvrIOMessageSend(sMessage *)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvrio_send(message: *mut Message) -> *const c_void {
    let Some(message) = (unsafe { message.as_mut() }) else {
        return ZERO.as_ptr().cast();
    };
    let name = || {
        if message.data.is_null() { &[][..] } else { unsafe { CStr::from_ptr(message.data.cast()) }.to_bytes() }
    };
    // WAL_GVR_TRACE=1: each message id once
    static TRACE_IDS: OnceLock<bool> = OnceLock::new();
    if *TRACE_IDS.get_or_init(|| std::env::var("WAL_GVR_TRACE").is_ok()) {
        static SEEN_IDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let mut seen = SEEN_IDS.lock().unwrap();
        if !seen.contains(&message.id) {
            seen.push(message.id);
            log!("gvrio: first message id {:#x} kind {} value {}", message.id, message.kind, message.value);
        }
    }
    let answer = match message.id {
        ID_UPDATE => 0,
        ID_AXIS => {
            let v = axis(message.value);
            // WAL_GVR_TRACE=1: the axes the program reads, once a second each
            static TRACE: OnceLock<bool> = OnceLock::new();
            static COUNT: AtomicU32 = AtomicU32::new(0);
            if *TRACE.get_or_init(|| std::env::var("WAL_GVR_TRACE").is_ok())
                && COUNT.fetch_add(1, Ordering::Relaxed) % 180 < 3
            {
                log!("gvrio: axis {} -> {v}", message.value);
            }
            v
        }
        ID_BUTTON => button(name()),
        ID_DONGLE_CHECK | ID_DONGLE_PRESENT | ID_DONGLE_REGION | ID_DONGLE_GAME | ID_DONGLE_VERSION | ID_DONGLE_ID
        | ID_DISPENSER_STATUS => {
            message.handled = 1;
            return dongle(message.id);
        }
        ID_BOARD_NAME => {
            message.handled = 1;
            return BOARD_NAME.as_ptr().cast();
        }
        ID_BOARD_MAJOR => BOARD_VERSION.0,
        ID_BOARD_MINOR => BOARD_VERSION.1,
        ID_BOARD_PRODUCT => BOARD_PRODUCT,
        ID_BAD_BUTTONS => 0,
        ID_DONGLE_COUNT => 1,
        ID_AXIS_RAW => match message.value {
            0 => axis(0) + 128,
            index => axis(index),
        },
        ID_SELECTION_X => SELECTION_X.load(Ordering::Relaxed),
        ID_SELECTION_Y => SELECTION_Y.load(Ordering::Relaxed),
        ID_SET_SELECTION_X | ID_SET_SELECTION_Y | ID_RANGE_X | ID_RANGE_Y => {
            let target = match message.id {
                ID_SET_SELECTION_X => &SELECTION_X,
                ID_SET_SELECTION_Y => &SELECTION_Y,
                ID_RANGE_X => &RANGE_X,
                _ => &RANGE_Y,
            };
            target.store(message.value, Ordering::Relaxed);
            0
        }
        ID_AXIS_MIN => 0,
        ID_AXIS_MAX => 255,
        // the wheel as 0..=255 too (centered 128)
        ID_AXIS_CALIBRATED => match message.value {
            0 => axis(0) + 128,
            index => axis(index),
        },
        id => {
            static SEEN: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
            let mut seen = SEEN.lock().unwrap();
            if !seen.contains(&id) {
                seen.push(id);
                log!("gvrio: message id {id:#x} kind {} value {} (not handled)", message.kind, message.value);
            }
            ZERO.store(0, Ordering::Relaxed);
            return ZERO.as_ptr().cast();
        }
    };
    message.handled = 1;
    ANSWER.store(answer, Ordering::Relaxed);
    ANSWER.as_ptr().cast()
}

/// The dongle and card dispenser plug-ins' answers.
fn dongle(id: u32) -> *const c_void {
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if id < 0x40 && LOGGED.fetch_or(1 << (id - 0x20), Ordering::Relaxed) & (1 << (id - 0x20)) == 0 {
        log!("gvrio: plug-in message {id:#x} answered");
    }
    match id {
        ID_DONGLE_CHECK => {
            post_value(ID_DONGLE_CHECKED, PRESENT as i32, None);
            (&PRESENT as *const u8).cast()
        }
        ID_DONGLE_PRESENT => (&PRESENT as *const u8).cast(),
        ID_DONGLE_REGION => REGION.as_ptr().cast(),
        ID_DONGLE_GAME => GAME.as_ptr().cast(),
        ID_DONGLE_VERSION => VERSION.as_ptr().cast(),
        ID_DONGLE_ID => (&DONGLE_ID as *const u32).cast(),
        // no card ready
        _ => 1 as *const c_void,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifter_gears() {
        assert_eq!(shifter(1), (true, false, true, false));
        assert_eq!(shifter(2), (true, false, false, true));
        assert_eq!(shifter(3), (false, false, true, false));
        assert_eq!(shifter(4), (false, false, false, true));
        assert_eq!(shifter(5), (false, true, true, false));
        assert_eq!(shifter(6), (false, true, false, true));
    }
}
