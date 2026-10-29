//! Global VR driving cabinets (Need for Speed GT): `GVRInputRaw.dll`, the input library of the
//! cabinet frontend (`UniverShell2.exe`) and of the game it starts (`NFSHP2.exe`).
//!
//! The original library tries its drivers in turn (DirectInput joystick, Immersion USB wheel
//! with the USBIO board's pedals and buttons, mice, USBIO trackball, keyboard), with the
//! cabinet's calibration and fault checks. This one is the driving cabinet fed by the virtual
//! sticks, installed under the same name next to both executables (profile `payload_install`).
//!
//! The caller owns the state (`GVRInputRawInit(hwnd, state, game)`), which `GVRInputRawUpdate`
//! refreshes and both programs read directly:
//!
//! | offset | content |
//! |---|---|
//! | 0x00 | i16 wheel, -128 (left) ..= 127 (the game reads its low byte) |
//! | 0x02 | u16 gas, 0 ..= 255 |
//! | 0x08 | previous wheel |
//! | 0x0C / 0x10 | buttons / previous buttons |
//! | 0x18 / 0x20 | QueryPerformanceCounter of this / the previous update |
//! | 0x30 | driver: 6 the wheel (NFSHP2 uses the library only then, else its own controllers) |
//! | 0x34 | u16 brake, 0 ..= 255 |
//! | 0x42 / 0x44 / 0x46 | raw wheel / gas / brake |
//! | 0x48 / 0x4A | lowest / highest wheel seen (calibration check) |
//! | 0x88 / 0x94 | force feedback spring / damper: u8 on, i32 strength 0..=100, i32 center |
//! | 0xA0 / 0xAC | force feedback sines (crashes, terrain): u8 on, i32 strength 0..=100, i32 period |
//!
//! The game writes the force feedback there (the original library plays them as DirectInput
//! effects on the Immersion wheel): the spring (its strength follows the speed) goes to the
//! launcher as `FFB_SPRING`, the strongest sine as `FFB_VIBRATION`. The damper follows the
//! spring and the center stays 0: not sent.
//!
//! Buttons (bits of the frontend's own debug keys, in brackets):
//!
//! | bit | button |
//! |---|---|
//! | 0x10 | start [S] (in race: car reset) |
//! | 0x20 | operator [O] (test menu) |
//! | 0x100 | music [R] |
//! | 0x200 | look back [L] |
//! | 0x400 | horn [H] |
//! | 0x10000 | view [V] |
//! | 0x20000 | handbrake |
//! | 0x40000 / 0x80000 | shift down / up |
//! | 0x100000 | coin [X] |
//!
//! Loaded as NFSHP2's `GVRInputRaw.dll`, the payload also writes the game's install keys the cabinet
//! has in its registry (the dump's `_Registry`): without them NFSHP2.exe stops on "Corrupted
//! files or registry; please reinstall.".

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState, output};
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Performance::QueryPerformanceCounter;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExA,
    RegSetValueExA,
};

/// The driver of the Immersion wheel.
const DRIVER_WHEEL: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u32);

const NATIVES: &[(&str, Native)] = &[
    ("start", Native(0x10)),
    ("operator", Native(0x20)),
    ("music", Native(0x100)),
    ("lookback", Native(0x200)),
    ("horn", Native(0x400)),
    ("view", Native(0x10000)),
    ("handbrake", Native(0x20000)),
    ("shiftdown", Native(0x40000)),
    ("shiftup", Native(0x80000)),
    ("coin", Native(0x100000)),
];

/// Virtual stick -> cabinet, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin"),
    ("test", "operator"),
    ("b1", "view"),
    ("b2", "music"),
    ("b3", "horn"),
    ("b4", "lookback"),
    ("b5", "shiftdown"),
    ("b6", "shiftup"),
    ("b7", "handbrake"),
];

enum Value {
    Str(&'static str),
    Dword(u32),
}

/// The cabinet's registry (the dump's `_Registry/update/*.reg`), HKEY_LOCAL_MACHINE.
const REGISTRY: &[(&str, &[(&str, Value)])] = &[
    (
        "SOFTWARE\\EA Games\\Need For Speed Hot Pursuit 2",
        &[
            ("Installed From", Value::Str("C:\\GVR\\Games\\HP2\\NFSHP2\\")),
            ("CacheSize", Value::Dword(0x85635)),
            ("SwapSize", Value::Dword(0)),
            ("DisplayName", Value::Str("Need For Speed Hot Pursuit 2")),
            ("Language", Value::Str("English US")),
            ("CD Drive", Value::Str("D:\\")),
            ("Install Dir", Value::Str("C:\\GVR\\Games\\HP2\\NFSHP2\\")),
            ("Product GUID", Value::Str("{76F4DD9B-C246-4BE0-00B6-3DE9ABF72299}")),
            ("Region", Value::Str("en-us")),
        ],
    ),
    (
        "SOFTWARE\\EA Games\\Need For Speed Hot Pursuit 2\\1.0",
        &[
            ("Language", Value::Dword(1)),
            ("DisplayName", Value::Str("Need For Speed Hot Pursuit 2")),
            ("LanguageName", Value::Str("English US")),
        ],
    ),
    ("SOFTWARE\\Electronic Arts\\EA Games\\Need For Speed Hot Pursuit 2\\ergc", &[("", Value::Str("3RE7AHLP4XQMZBZR"))]),
];

fn file_name(module: HINSTANCE) -> String {
    let mut name = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    String::from_utf16_lossy(&name[..n]).to_ascii_lowercase()
}

/// Writes NFSHP2's registry when the payload is installed as its `GVRInputRaw.dll`; gives
/// Need for Speed Underground's race a keyboard.
pub(crate) fn init(module: HINSTANCE) {
    // UndergroundGVR.exe reads GVRInputRaw.dll from its keyboard's update (the keyboard keys and
    // the cabinet's wheel together): without a keyboard device (the fake DirectInput of
    // WAL_DINPUT_DISABLE has none) GVRInputRawUpdate is never called. An empty one.
    if file_name(module).ends_with("\\gvrinputraw.dll") && crate::frontend_fit::exe_name() == "undergroundgvr.exe" {
        wal_payload_common::dinput::set_keyboard(|_| {});
        log!("gvrinput: empty DirectInput keyboard for the race");
    }
    if !file_name(module).ends_with("\\gvrinputraw.dll") || !file_name(std::ptr::null_mut()).ends_with("\\nfshp2.exe") {
        return;
    }
    for (key, values) in REGISTRY {
        let ckey = format!("{key}\0");
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExA(
                HKEY_LOCAL_MACHINE,
                ckey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            log!("gvrinput: cannot create HKLM\\{key}: error {status}");
            continue;
        }
        for (name, value) in *values {
            let cname = format!("{name}\0");
            let (kind, data) = match value {
                Value::Str(s) => (REG_SZ, format!("{s}\0").into_bytes()),
                Value::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            };
            unsafe { RegSetValueExA(hkey, cname.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
        }
        unsafe { RegCloseKey(hkey) };
    }
    log!("gvrinput: cabinet registry written");
}

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static LAST: AtomicU32 = AtomicU32::new(u32::MAX);
static SPRING: AtomicI32 = AtomicI32::new(-1);
static VIBRATION: AtomicI32 = AtomicI32::new(-1);

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

unsafe fn put16(state: *mut u8, offset: usize, v: i16) {
    unsafe { (state.add(offset) as *mut i16).write_unaligned(v) }
}

unsafe fn put32(state: *mut u8, offset: usize, v: u32) {
    unsafe { (state.add(offset) as *mut u32).write_unaligned(v) }
}

unsafe fn get32(state: *const u8, offset: usize) -> u32 {
    unsafe { (state.add(offset) as *const u32).read_unaligned() }
}

/// Strength of the effect at `offset` (0..=FFB_MAX), 0 when off.
unsafe fn effect(state: *const u8, offset: usize) -> i32 {
    if unsafe { *state.add(offset) } == 0 {
        return 0;
    }
    (unsafe { get32(state, offset + 4) } as i32).clamp(0, 100) * output::FFB_MAX / 100
}

/// Sends the game's force feedback on change.
fn ffb(state: *const u8) {
    let spring = unsafe { effect(state, 0x88) };
    let vibration = unsafe { effect(state, 0xA0).max(effect(state, 0xAC)) };
    for (id, value, last) in [(output::FFB_SPRING, spring, &SPRING), (output::FFB_VIBRATION, vibration, &VIBRATION)] {
        let old = last.swap(value, Ordering::Relaxed);
        if old != value {
            wal_payload_common::send_output(0, id, value);
            if (old <= 0) != (value <= 0) {
                log!("gvrinput: ffb {} {value}", if id == output::FFB_SPRING { "spring" } else { "vibration" });
            }
        }
    }
}

/// Virtual stick -> wheel, -128..=127.
fn wheel(v: i16) -> i16 {
    v >> 8
}

/// How the program reads the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Need for Speed GT (frontend and NFSHP2.exe): the table above.
    Gt = 0,
    /// Need for Speed Underground's race (UndergroundGVR.exe): the wheel, gas and brake as Need
    /// for Speed GT (it adds 0x80 to the wheel's low byte), and the gears of its H shifter
    /// ([`shifter`]).
    UgRace = 1,
    /// Need for Speed Underground's frontend (UniverShell2.exe through NFSControl.dll): the wheel
    /// selects by its position (i16), which must not change but on deliberate input (each new
    /// selection is an event into its .NET engine: a wheel moving every frame left it busy, then
    /// hung); no pedal fields, the gas is the accept button (0x100) and the brake the back one
    /// (0x400), and 0x42..0x48 stay 0 (it reads them, garbage there spun it).
    UgShell = 2,
    /// Beach Head 2002 and its shell: the older library's state (`turret.rs`).
    Turret = 3,
}

static LAYOUT: AtomicU32 = AtomicU32::new(Layout::Gt as u32);
/// The Underground race's gear, 1..=6 (manual transmission: the frontend's `-trans 1`).
static GEAR: AtomicU32 = AtomicU32::new(1);
/// The shift buttons held at the last update (edge detection).
static SHIFT_HELD: AtomicU32 = AtomicU32::new(0);

/// The Underground cabinet's 6-speed H shifter: the bits of each gear, 1st to 6th (none: 3rd).
const SHIFTER: [u32; 6] = [0x1, 0x80001, 0, 0x80000, 0x2, 0x80002];
const SHIFT_DOWN: u32 = 0x40000;
const SHIFT_UP: u32 = 0x80000;

/// The Underground race's buttons: the shift down / up buttons step the gear, sent as the H
/// shifter's position (its bits share 0x80000 with shift up).
fn shifter(buttons: u32) -> u32 {
    let shift = buttons & (SHIFT_DOWN | SHIFT_UP);
    let pressed = shift & !SHIFT_HELD.swap(shift, Ordering::Relaxed);
    let mut gear = GEAR.load(Ordering::Relaxed);
    if pressed & SHIFT_UP != 0 && gear < 6 {
        gear += 1;
    }
    if pressed & SHIFT_DOWN != 0 && gear > 1 {
        gear -= 1;
    }
    if pressed != 0 {
        GEAR.store(gear, Ordering::Relaxed);
        log!("gvrinput: gear {gear}");
    }
    (buttons & !(SHIFT_DOWN | SHIFT_UP)) | SHIFTER[gear as usize - 1]
}
/// The Underground frontend's wheel, -120..=120 in steps of 8.
static SHELL_WHEEL: AtomicI32 = AtomicI32::new(0);

fn layout() -> Layout {
    match LAYOUT.load(Ordering::Relaxed) {
        1 => Layout::UgRace,
        2 => Layout::UgShell,
        3 => Layout::Turret,
        _ => Layout::Gt,
    }
}

/// The Underground frontend's wheel for this stick value: quantized, moved only by a full step.
fn shell_wheel(steer: i16) -> i16 {
    const STEP: i32 = 8;
    let target = ((steer as i32 / STEP) * STEP).clamp(-120, 120);
    let current = SHELL_WHEEL.load(Ordering::Relaxed);
    if (target - current).abs() >= STEP {
        SHELL_WHEEL.store(target, Ordering::Relaxed);
        return target as i16;
    }
    current as i16
}

fn fill(state: *mut u8) {
    let stick: StickState = wal_payload_common::input(0);
    let mut buttons = 0;
    map().for_each_pressed(&stick, |Native(bit)| buttons |= bit);
    let steer = wheel(stick.axis(Axis::LeftX));
    let gas = stick.axis_u8(Axis::Accel) as i16;
    let brake = stick.axis_u8(Axis::Brake) as i16;
    unsafe {
        put16(state, 0x08, (state as *const i16).read_unaligned());
        match layout() {
            Layout::Gt => {
                put16(state, 0x00, steer);
                put16(state, 0x42, steer);
                put16(state, 0x02, gas);
                put16(state, 0x44, gas);
                put16(state, 0x34, brake);
                put16(state, 0x46, brake);
                put16(state, 0x48, -128);
                put16(state, 0x4A, 127);
            }
            Layout::UgRace => {
                put16(state, 0x00, steer);
                put16(state, 0x02, gas);
                put16(state, 0x34, brake);
                buttons = shifter(buttons);
            }
            Layout::UgShell => {
                put16(state, 0x00, shell_wheel(steer));
                put16(state, 0x02, 0);
                put16(state, 0x34, 0);
                state.add(0x42).write_bytes(0, 6);
                if gas > 0x80 {
                    buttons |= 0x100;
                }
                if brake > 0x80 {
                    buttons |= 0x400;
                }
            }
            // its own state (turret.rs), never filled here
            Layout::Turret => return,
        }
        put32(state, 0x10, get32(state, 0x0C));
        put32(state, 0x0C, buttons);
        put32(state, 0x30, DRIVER_WHEEL);
    }
    if LAST.swap(buttons, Ordering::Relaxed) != buttons {
        log!("gvrinput: buttons {buttons:#x}");
    }
    // WAL_GVR_TRACE=1: what the program gets, every 60 updates
    static TRACE: OnceLock<bool> = OnceLock::new();
    static COUNT: AtomicU32 = AtomicU32::new(0);
    if *TRACE.get_or_init(|| std::env::var("WAL_GVR_TRACE").is_ok()) && COUNT.fetch_add(1, Ordering::Relaxed) % 60 == 0 {
        let word = |o: usize| unsafe { (state.add(o) as *const i16).read_unaligned() };
        log!("gvrinput: update wheel {} gas {} brake {} buttons {:#x}", word(0), word(2), word(0x34), unsafe { get32(state, 0x0C) });
    }
}

/// `int GVRInputRawInit(HWND, state *, const char *game)`: 0 when a driver is ready.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_init(_hwnd: usize, state: *mut u8, game: *const u8) -> i32 {
    let name = if game.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(game.cast()) }.to_string_lossy().into_owned()
    };
    let layout = if crate::turret::is_turret(&name) {
        Layout::Turret
    } else if !name.eq_ignore_ascii_case("NFSUNDERGROUND") {
        Layout::Gt
    } else if crate::frontend_fit::exe_name() == "undergroundgvr.exe" {
        Layout::UgRace
    } else {
        Layout::UgShell
    };
    LAYOUT.store(layout as u32, Ordering::Relaxed);
    log!("gvrinput: init for {name:?}, state at {state:p}, {layout:?} layout");
    map();
    if state.is_null() {
        return 2;
    }
    if layout == Layout::Turret {
        crate::turret::set_caller(&name);
        crate::turret::init(state);
    } else {
        fill(state);
    }
    0
}

/// `int GVRInputRawUpdate(state *)`: 0 when updated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_update(state: *mut u8) -> i32 {
    if state.is_null() {
        return 3;
    }
    unsafe {
        put32(state, 0x20, get32(state, 0x18));
        put32(state, 0x24, get32(state, 0x1C));
        QueryPerformanceCounter(state.add(0x18) as *mut i64);
    }
    if layout() == Layout::Turret {
        crate::turret::fill(state);
        return 0;
    }
    fill(state);
    ffb(state);
    crate::frontend_fit::update();
    0
}

/// The calls answering nothing to do or no fault: cleanup, sleep, wake, calibration and FFB
/// resets, fault counters.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_none() -> i32 {
    0
}

/// `int GVRInputRawGetCardStatus()`: 1, what the original answers without a card dispenser (0
/// is a card ready).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_card_status() -> i32 {
    1
}

/// `ReadDrivingValues` / `WriteDrivingValues`: the cabinet's calibration file, nothing to keep.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_driving_values() -> i32 {
    1
}

/// `CUSBIO *GVRInputRawGetCUSBIO()`: no USBIO board.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gvr_cusbio() -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_range() {
        assert_eq!(wheel(i16::MIN), -128);
        assert_eq!(wheel(0), 0);
        assert_eq!(wheel(i16::MAX), 127);
    }

    #[test]
    fn shell_wheel_steps() {
        assert_eq!(shell_wheel(5), 0);
        assert_eq!(shell_wheel(40), 40);
        assert_eq!(shell_wheel(44), 40);
        assert_eq!(shell_wheel(127), 120);
    }
}
