//! `WAL_DINPUT_DISABLE=1`: the game gets a DirectInput that works but sees no input, so only
//! its I/O board drives it. Some arcade builds still read the keyboard and joysticks through
//! DirectInput (debug or operator keys): Raiden IV (Type X) enters its test menu on the PC
//! keyboard's `2`, which the launcher also maps to P2 start (the keyboard is not grabbed, the
//! game window sees it).
//!
//! `DirectInputCreateA/W/Ex` and `DirectInput8Create` return a static fake interface (the
//! IDirectInput8 vtable, a superset of the older ones): `EnumDevices` lists nothing,
//! `CreateDevice` returns a fake device whose methods succeed, with zeroed states and no
//! buffered data. Failing the creation instead would end some games.
//!
//! `WAL_DINPUT_WHEEL=<product name>`: the same fake DirectInput, with one DirectInput 8 game
//! controller (a driving wheel) under that product name: `EnumDevices` lists it, its X, Y and
//! Z axes take the ranges the game sets (`DIPROP_RANGE`, default 0-65535), X follows player
//! 1's `lx` (`WAL_DINPUT_WHEEL_AXIS`, `-` inverts), Y and Z stay released, no buttons. The
//! cabinet wheels (D1GP Arcade, Mario Kart GP DX) are USB "Immersion TouchSense Steering
//! Wheel" devices: the game steers with the X axis, gates its I/O board switches and its
//! force feedback option on that exact product name, and drives the FFB through DirectInput
//! effects (`CreateEffect` / `SetParameters`). The fake wheel therefore also reports
//! `DIDC_FORCEFEEDBACK` and accepts effect creation, so a forced-FFB boot does not wait on a
//! motor that is not there.
//!
//! `WAL_DINPUT_BOARD=<product name or 1>`: the same fake DirectInput, with one cabinet button
//! box (26 buttons, the Injustice cabinet's layout): the players' fight buttons, coins, start,
//! the card dispensers' buy buttons and the card readers' scan buttons, from the virtual
//! sticks.
//!
//! `WAL_KEYBOARD_STATE_DISABLE=1`: the game's `user32!GetKeyboardState` reports no key pressed
//! (Wacky Races reads START on the PC keyboard's Enter and VIEW on Right Shift that way, next
//! to its JVS switches).

use std::ffi::c_void;

use crate::{iat, log};

type P = *mut c_void;

const DI_OK: i32 = 0;
const DIERR_UNSUPPORTED: i32 = 0x80004001u32 as i32;
const DIERR_DEVICENOTREG: i32 = 0x80040154u32 as i32;

#[repr(C)]
struct Object {
    vtable: *const *const (),
}
unsafe impl Sync for Object {}

struct VTable<const N: usize>([*const (); N]);
unsafe impl<const N: usize> Sync for VTable<N> {}

// Methods by argument count after `this` (stdcall: the callee pops them).
unsafe extern "system" fn ok0(_this: P) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok1(_this: P, _a: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok2(_this: P, _a: usize, _b: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok3(_this: P, _a: usize, _b: usize, _c: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok4(_this: P, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok5(_this: P, _a: usize, _b: usize, _c: usize, _d: usize, _e: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn unsupported1(_this: P, _a: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported2(_this: P, _a: usize, _b: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported3(_this: P, _a: usize, _b: usize, _c: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported4(_this: P, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn ref_count(_this: P) -> u32 {
    1
}

/// `QueryInterface`: any interface of the object is the object itself (IDirectInputDevice2/7/8
/// share the first slots).
unsafe extern "system" fn query_interface(this: P, _iid: P, out: *mut P) -> i32 {
    if !out.is_null() {
        unsafe { *out = this };
    }
    DI_OK
}

/// `GetDeviceState(cbData, lpvData)`: nothing pressed, axes 0.
unsafe extern "system" fn get_device_state(_this: P, size: u32, data: *mut u8) -> i32 {
    if !data.is_null() {
        unsafe { std::ptr::write_bytes(data, 0, size as usize) };
    }
    DI_OK
}

/// `GetDeviceData(cbObjectData, rgdod, pdwInOut, dwFlags)`: no buffered events.
unsafe extern "system" fn get_device_data(_this: P, _size: u32, _data: P, count: *mut u32, _flags: u32) -> i32 {
    {
        // Diagnostic: is the game polling buffered events instead of GetDeviceState?
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let last = LAST.load(std::sync::atomic::Ordering::Relaxed);
        if now - last >= 2000 {
            LAST.store(now, std::sync::atomic::Ordering::Relaxed);
            log!("dinput: GetDeviceData called (no buffered events)");
        }
    }
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    DI_OK
}

/// IDirectInputDevice8 (IDirectInputDeviceA/2/7 are its prefixes).
static DEVICE_VTABLE: VTable<32> = VTable([
    query_interface as *const (),
    ref_count as *const (), // AddRef
    ref_count as *const (), // Release
    ok1 as *const (), // GetCapabilities
    ok3 as *const (), // EnumObjects
    ok2 as *const (), // GetProperty
    ok2 as *const (), // SetProperty
    ok0 as *const (), // Acquire
    ok0 as *const (), // Unacquire
    get_device_state as *const (),
    get_device_data as *const (),
    ok1 as *const (), // SetDataFormat
    ok1 as *const (), // SetEventNotification
    ok2 as *const (), // SetCooperativeLevel
    unsupported3 as *const (), // GetObjectInfo
    unsupported1 as *const (), // GetDeviceInfo
    ok2 as *const (), // RunControlPanel
    ok3 as *const (), // Initialize
    unsupported4 as *const (), // CreateEffect
    ok3 as *const (), // EnumEffects
    unsupported2 as *const (), // GetEffectInfo
    unsupported1 as *const (), // GetForceFeedbackState
    ok1 as *const (), // SendForceFeedbackCommand
    ok3 as *const (), // EnumCreatedEffectObjects
    unsupported1 as *const (), // Escape
    ok0 as *const (), // Poll
    ok4 as *const (), // SendDeviceData
    ok4 as *const (), // EnumEffectsInFile
    ok4 as *const (), // WriteEffectToFile
    unsupported3 as *const (), // BuildActionMap
    unsupported3 as *const (), // SetActionMap
    unsupported1 as *const (), // GetImageInfo
]);
static DEVICE: Object = Object { vtable: DEVICE_VTABLE.0.as_ptr() };

/// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`
unsafe extern "system" fn create_device(_this: P, _guid: P, out: *mut P, _outer: P) -> i32 {
    if !out.is_null() {
        unsafe { *out = &DEVICE as *const Object as P };
    }
    DI_OK
}

/// IDirectInput8 (IDirectInputA/2/7 share the first 8 slots; IDirectInput7's CreateDeviceEx
/// sits where FindDevice is, with one more argument: not used by these games).
static DINPUT_VTABLE: VTable<11> = VTable([
    query_interface as *const (),
    ref_count as *const (), // AddRef
    ref_count as *const (), // Release
    create_device as *const (),
    ok4 as *const (), // EnumDevices: none
    unsupported1 as *const (), // GetDeviceStatus
    ok2 as *const (), // RunControlPanel
    ok2 as *const (), // Initialize
    unsupported3 as *const (), // FindDevice
    ok5 as *const (), // EnumDevicesBySemantics
    unsupported4 as *const (), // ConfigureDevices
]);
static DINPUT: Object = Object { vtable: DINPUT_VTABLE.0.as_ptr() };

/// `wide`: the game asked for a Unicode interface (its EnumDevices / EnumObjects structures).
fn fake(out: *mut P, wide: bool) -> i32 {
    if out.is_null() {
        return DIERR_DEVICENOTREG;
    }
    let dinput = match (
        std::env::var("WAL_DINPUT_BOARD").is_ok(),
        std::env::var_os("WAL_DINPUT_WHEEL").is_some(),
        wide,
    ) {
        (true, _, false) => &board::DINPUT_A,
        (true, _, true) => &board::DINPUT_W,
        (false, true, false) => &wheel::DINPUT_A,
        (false, true, true) => &wheel::DINPUT_W,
        (false, false, _) => &DINPUT,
    };
    unsafe { *out = dinput as *const Object as P };
    log!("dinput: game got the fake DirectInput{}", if wide { " (W)" } else { "" });
    DI_OK
}

/// `DirectInputCreateA(hinst, version, out, outer)`
unsafe extern "system" fn create(_instance: P, _version: u32, out: *mut P, _outer: P) -> i32 {
    fake(out, false)
}

/// `DirectInputCreateW(hinst, version, out, outer)`
unsafe extern "system" fn create_w(_instance: P, _version: u32, out: *mut P, _outer: P) -> i32 {
    fake(out, true)
}

/// `DirectInputCreateEx` / `DirectInput8Create(hinst, version, riid, out, outer)`: the Unicode
/// interfaces (IDirectInput8W, IDirectInput7W, IDirectInput2W, IDirectInputW) by their IID's
/// first field.
unsafe extern "system" fn create_iid(_instance: P, _version: u32, iid: *const u32, out: *mut P, _outer: P) -> i32 {
    let wide = !iid.is_null() && matches!(unsafe { *iid }, 0xBF79_8031 | 0x9A4C_B685 | 0x5944_E663 | 0x8952_1361);
    fake(out, wide)
}

static ORIG_GET_PROC_ADDRESS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The DirectInput creation functions resolved at run time are the fake ones.
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    let orig: unsafe extern "system" fn(P, *const u8) -> usize =
        unsafe { std::mem::transmute(ORIG_GET_PROC_ADDRESS.load(std::sync::atomic::Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if f == 0 || (name as usize) >> 16 == 0 {
        return f; // not found, or by ordinal
    }
    let fake = match unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() {
        b"DirectInputCreateA" => create as *const () as usize,
        b"DirectInputCreateW" => create_w as *const () as usize,
        b"DirectInputCreateEx" | b"DirectInput8Create" => create_iid as *const () as usize,
        _ => return f,
    };
    log!("dinput: {} resolved at run time: the fake one", unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy());
    fake
}

unsafe extern "system" fn get_keyboard_state(keys: *mut u8) -> i32 {
    if !keys.is_null() {
        unsafe { std::ptr::write_bytes(keys, 0, 256) };
    }
    1
}

pub fn init() {
    if std::env::var("WAL_KEYBOARD_STATE_DISABLE").is_ok_and(|v| v.trim() == "1") {
        unsafe { iat::hook("user32.dll", "GetKeyboardState", get_keyboard_state as *const () as usize) };
        log!("dinput: GetKeyboardState reports no key");
    }
    let board = std::env::var("WAL_DINPUT_BOARD").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let wheel = std::env::var("WAL_DINPUT_WHEEL").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if board.is_none() && wheel.is_none() && !std::env::var("WAL_DINPUT_DISABLE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe {
        iat::hook("dinput.dll", "DirectInputCreateA", create as *const () as usize);
        iat::hook("dinput.dll", "DirectInputCreateW", create_w as *const () as usize);
        iat::hook("dinput.dll", "DirectInputCreateEx", create_iid as *const () as usize);
        iat::hook("dinput8.dll", "DirectInput8Create", create_iid as *const () as usize);
        // games loading DirectInput at run time (Ford Racing: LoadLibrary("dinput8.dll"))
        if let Some(o) = iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) {
            ORIG_GET_PROC_ADDRESS.store(o, std::sync::atomic::Ordering::Relaxed);
        }
    }
    match (board, wheel) {
        (Some(name), _) => {
            board::init(name);
            log!("dinput: fake DirectInput with a board");
        }
        (None, Some(name)) => {
            wheel::init(name);
            log!("dinput: fake DirectInput with a wheel");
        }
        (None, None) => log!("dinput: disabled (fake DirectInput without devices)"),
    }
}

/// The fake wheel of `WAL_DINPUT_WHEEL`.
mod wheel {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};

    use wal_protocol::Axis;

    use super::{ok0, ok1, ok2, ok3, query_interface, ref_count, DEVICE_VTABLE, DI_OK, Object, P, VTable};
    use crate::log;

    static NAME: OnceLock<String> = OnceLock::new();
    /// The steering axis (`Axis` index), bit 7 set when inverted.
    static STEER: AtomicU8 = AtomicU8::new(Axis::LeftX as u8);
    /// Virtual input axes for X, Y and Z. 0xff means a parked, unused axis.
    static INPUT_AXES: [AtomicU8; 3] = [
        AtomicU8::new(Axis::LeftX as u8),
        AtomicU8::new(0xff),
        AtomicU8::new(0xff),
    ];
    /// (min, max) of the X, Y and Z axes, set by the game.
    static RANGES: [(AtomicI32, AtomicI32); 3] = [
        (AtomicI32::new(0), AtomicI32::new(65535)),
        (AtomicI32::new(0), AtomicI32::new(65535)),
        (AtomicI32::new(0), AtomicI32::new(65535)),
    ];

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
        let a = d1.to_le_bytes();
        let b = d2.to_le_bytes();
        let c = d3.to_le_bytes();
        [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
    }
    pub(super) const INSTANCE: [u8; 16] = guid(0x5741_4C57, 0x4845, 0x454C, *b"WALWHEEL");
    const PRODUCT: [u8; 16] = guid(0x5741_4C57, 0, 0, *b"\0\0PIDVID");
    /// GUID_XAxis, GUID_YAxis, GUID_ZAxis
    const AXES: [[u8; 16]; 3] = [
        guid(0xA36D_02E0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02E1, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02E2, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
    ];
    const AXIS_NAMES: [&str; 3] = ["X Axis", "Y Axis", "Z Axis"];
    /// DI8DEVTYPE_DRIVING, DI8DEVTYPEDRIVING_DUALPEDALS, DIDEVTYPE_HID
    const DEV_TYPE: u32 = 0x16 | 3 << 8 | 0x1_0000;
    /// DIDFT_ABSAXIS
    const ABS_AXIS: u32 = 2;
    const DIPROP_RANGE: usize = 4;
    const DIPH_BYOFFSET: u32 = 1;
    const DIPH_BYID: u32 = 2;
    const DIERR_NOTFOUND: i32 = 0x8007_0002u32 as i32;

    pub(super) fn init(name: String) {
        if let Ok(axis) = std::env::var("WAL_DINPUT_WHEEL_AXIS") {
            let axis = axis.trim();
            let (a, inverted) = axis.strip_prefix('-').map_or((axis, false), |a| (a, true));
            match Axis::from_name(a) {
                Some(a) => STEER.store(a as u8 | if inverted { 0x80 } else { 0 }, Ordering::Relaxed),
                None => log!("dinput: unknown wheel axis {axis:?}"),
            }
        }
        INPUT_AXES[0].store(STEER.load(Ordering::Relaxed), Ordering::Relaxed);
        for (i, key) in ["WAL_DINPUT_WHEEL_Y_AXIS", "WAL_DINPUT_WHEEL_Z_AXIS"].into_iter().enumerate() {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                let (value, invert) = value.strip_prefix('-').map_or((value, false), |axis| (axis, true));
                match Axis::from_name(value) {
                    Some(axis) => INPUT_AXES[i + 1].store(axis as u8 | if invert { 0x80 } else { 0 }, Ordering::Relaxed),
                    None => log!("dinput: unknown wheel axis {key}={value:?}"),
                }
            }
        }
        log!("dinput: wheel {name:?}");
        let _ = NAME.set(name);
    }

    fn name() -> &'static str {
        NAME.get().map_or("", |n| n.as_str())
    }

    /// Copies `s` into a fixed string field of `len` characters, ANSI or UTF-16.
    unsafe fn put_str(dst: *mut u8, len: usize, s: &str, wide: bool) {
        unsafe {
            if wide {
                let dst = dst as *mut u16;
                for (i, c) in s.encode_utf16().take(len - 1).chain(std::iter::once(0)).enumerate() {
                    dst.add(i).write_unaligned(c);
                }
            } else {
                for (i, c) in s.bytes().take(len - 1).chain(std::iter::once(0)).enumerate() {
                    *dst.add(i) = c;
                }
            }
        }
    }

    /// Fills a DIDEVICEINSTANCEA / W (`wide`), its dwSize included.
    unsafe fn device_instance(out: *mut u8, wide: bool) {
        let chars = if wide { 2 } else { 1 };
        let size = 4 + 36 + 2 * 260 * chars + 20;
        unsafe {
            std::ptr::write_bytes(out, 0, size);
            (out as *mut u32).write_unaligned(size as u32);
            std::ptr::copy_nonoverlapping(INSTANCE.as_ptr(), out.add(4), 16);
            std::ptr::copy_nonoverlapping(PRODUCT.as_ptr(), out.add(20), 16);
            (out.add(36) as *mut u32).write_unaligned(DEV_TYPE);
            put_str(out.add(40), 260, name(), wide);
            put_str(out.add(40 + 260 * chars), 260, name(), wide);
            // HID usage page 1 (generic desktop), usage 4 (joystick)
            let usage = out.add(40 + 2 * 260 * chars + 16);
            (usage as *mut u16).write_unaligned(1);
            (usage.add(2) as *mut u16).write_unaligned(4);
        }
    }

    type EnumCallback = unsafe extern "system" fn(*const u8, P) -> i32;

    /// Whether an `EnumDevices` type filter takes the wheel: all devices, game controllers
    /// (DI8DEVCLASS_GAMECTRL), driving devices.
    fn listed(filter: u32) -> bool {
        matches!(filter & 0xFF, 0 | 4 | 0x16)
    }

    unsafe fn enum_devices(filter: u32, cb: EnumCallback, context: P, wide: bool) -> i32 {
        if listed(filter) {
            let mut instance = [0u8; 0x44C];
            unsafe {
                device_instance(instance.as_mut_ptr(), wide);
                cb(instance.as_ptr(), context);
            }
        }
        DI_OK
    }

    /// `IDirectInput8A::EnumDevices(dwDevType, lpCallback, pvRef, dwFlags)`
    unsafe extern "system" fn enum_devices_a(_this: P, filter: u32, cb: EnumCallback, context: P, _flags: u32) -> i32 {
        unsafe { enum_devices(filter, cb, context, false) }
    }
    unsafe extern "system" fn enum_devices_w(_this: P, filter: u32, cb: EnumCallback, context: P, _flags: u32) -> i32 {
        unsafe { enum_devices(filter, cb, context, true) }
    }

    /// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`: the wheel for its instance GUID.
    unsafe fn create_device(guid: *const u8, out: *mut P, wide: bool) -> i32 {
        if out.is_null() {
            return super::DIERR_DEVICENOTREG;
        }
        let is_wheel = !guid.is_null() && unsafe { std::slice::from_raw_parts(guid, 16) } == INSTANCE;
        let device: &Object = match (is_wheel, wide) {
            (true, false) => &WHEEL_A,
            (true, true) => &WHEEL_W,
            (false, _) => &super::DEVICE,
        };
        unsafe { *out = device as *const Object as P };
        DI_OK
    }
    unsafe extern "system" fn create_device_a(_this: P, guid: *const u8, out: *mut P, _outer: P) -> i32 {
        unsafe { create_device(guid, out, false) }
    }
    unsafe extern "system" fn create_device_w(_this: P, guid: *const u8, out: *mut P, _outer: P) -> i32 {
        unsafe { create_device(guid, out, true) }
    }

    /// The fake IDirectInput8A / W with the wheel.
    pub(super) static DINPUT_A: Object = Object { vtable: DINPUT_A_VTABLE.0.as_ptr() };
    pub(super) static DINPUT_W: Object = Object { vtable: DINPUT_W_VTABLE.0.as_ptr() };
    static DINPUT_A_VTABLE: VTable<11> = dinput_vtable(create_device_a as *const (), enum_devices_a as *const ());
    static DINPUT_W_VTABLE: VTable<11> = dinput_vtable(create_device_w as *const (), enum_devices_w as *const ());

    const fn dinput_vtable(create: *const (), enumerate: *const ()) -> VTable<11> {
        let mut v = super::DINPUT_VTABLE.0;
        v[3] = create;
        v[4] = enumerate;
        VTable(v)
    }

    /// DIDC_ATTACHED | DIDC_FORCEFEEDBACK.
    const CAPS_FLAGS: u32 = 1 | 0x100;

    /// `GetCapabilities(lpDIDevCaps)`: DIDEVCAPS, attached, FFB capable, 3 axes.
    unsafe extern "system" fn get_capabilities(_this: P, caps: *mut u32) -> i32 {
        if !caps.is_null() {
            unsafe {
                let size = (*caps).clamp(4, 44) as usize;
                std::ptr::write_bytes(caps.add(1) as *mut u8, 0, size - 4);
                // dwFlags, dwDevType, dwAxes, dwButtons, dwPOVs, then the FFB timing fields.
                for (i, v) in [CAPS_FLAGS, DEV_TYPE, 3, 32, 0, 10_000, 1, 1, 1, 0x0001_0000].into_iter().enumerate() {
                    if 4 * (i + 2) <= size {
                        *caps.add(i + 1) = v;
                    }
                }
            }
        }
        DI_OK
    }

    /// `EnumObjects(lpCallback, pvRef, dwFlags)`: the 3 absolute axes (when asked).
    unsafe fn enum_objects(cb: EnumCallback, context: P, flags: u32, wide: bool) -> i32 {
        // DIDFT_ALL, or a type mask with DIDFT_AXIS / DIDFT_ABSAXIS
        if flags != 0 && flags & 7 == 0 {
            return DI_OK;
        }
        let chars = if wide { 2 } else { 1 };
        let size = 32 + 260 * chars + 24;
        let mut object = [0u8; 0x240];
        for i in 0..3 {
            if flags != 0 && flags & 3 == 0 { break; }
            object.fill(0);
            let o = object.as_mut_ptr();
            unsafe {
                (o as *mut u32).write_unaligned(size as u32);
                std::ptr::copy_nonoverlapping(AXES[i].as_ptr(), o.add(4), 16);
                (o.add(20) as *mut u32).write_unaligned(4 * i as u32); // dwOfs in DIJOYSTATE
                (o.add(24) as *mut u32).write_unaligned(ABS_AXIS | (i as u32) << 8); // dwType
                put_str(o.add(32), 260, AXIS_NAMES[i], wide);
                if cb(o, context) == 0 {
                    break; // DIENUM_STOP
                }
            }
        }
        if flags == 0 || flags & 4 != 0 {
            for i in 0..32 {
                object.fill(0);
                let o = object.as_mut_ptr();
                unsafe {
                    (o as *mut u32).write_unaligned(size as u32);
                    // GUID_Button, with the button index carried by dwType.
                    let button_guid = guid(0xA36D_02F0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
                    std::ptr::copy_nonoverlapping(button_guid.as_ptr(), o.add(4), 16);
                    (o.add(20) as *mut u32).write_unaligned(0x30 + i as u32);
                    (o.add(24) as *mut u32).write_unaligned(4 | (i as u32) << 8);
                    let label = format!("Button {}", i + 1);
                    put_str(o.add(32), 260, &label, wide);
                    if cb(o, context) == 0 { break; }
                }
            }
        }
        DI_OK
    }
    unsafe extern "system" fn enum_objects_a(_this: P, cb: EnumCallback, context: P, flags: u32) -> i32 {
        unsafe { enum_objects(cb, context, flags, false) }
    }
    unsafe extern "system" fn enum_objects_w(_this: P, cb: EnumCallback, context: P, flags: u32) -> i32 {
        unsafe { enum_objects(cb, context, flags, true) }
    }

    /// The axis a DIPROPHEADER names (by its DIJOYSTATE offset or its object ID).
    unsafe fn header_axis(header: *const u32) -> Option<usize> {
        let (obj, how) = unsafe { (*header.add(2), *header.add(3)) };
        let axis = match how {
            DIPH_BYOFFSET => obj / 4,
            DIPH_BYID if obj & ABS_AXIS != 0 => obj >> 8 & 0xFFFF,
            _ => return None,
        } as usize;
        (axis < 3).then_some(axis)
    }

    /// `GetProperty(rguidProp, pdiph)`: the axis ranges.
    unsafe extern "system" fn get_property(_this: P, prop: usize, header: *mut u32) -> i32 {
        if prop != DIPROP_RANGE || header.is_null() {
            return DI_OK;
        }
        let Some(axis) = (unsafe { header_axis(header) }) else { return DIERR_NOTFOUND };
        unsafe {
            *header.add(4) = RANGES[axis].0.load(Ordering::Relaxed) as u32;
            *header.add(5) = RANGES[axis].1.load(Ordering::Relaxed) as u32;
        }
        DI_OK
    }

    /// `SetProperty(rguidProp, pdiph)`: the axis ranges (DIPH_DEVICE sets all of them).
    unsafe extern "system" fn set_property(_this: P, prop: usize, header: *const u32) -> i32 {
        if prop != DIPROP_RANGE || header.is_null() {
            return DI_OK;
        }
        let (min, max) = unsafe { (*header.add(4) as i32, *header.add(5) as i32) };
        let axes = match unsafe { *header.add(3) } {
            0 => 0..3,
            _ => match unsafe { header_axis(header) } {
                Some(a) => a..a + 1,
                None => return DIERR_NOTFOUND,
            },
        };
        for a in axes {
            log!("dinput: wheel axis {a} range {min}..{max}");
            RANGES[a].0.store(min, Ordering::Relaxed);
            RANGES[a].1.store(max, Ordering::Relaxed);
        }
        DI_OK
    }

    fn scaled(axis: usize, v: u16) -> i32 {
        let (min, max) = (RANGES[axis].0.load(Ordering::Relaxed) as i64, RANGES[axis].1.load(Ordering::Relaxed) as i64);
        (min + (max - min) * v as i64 / 65535) as i32
    }

    /// `GetDeviceState(cbData, lpvData)`: DIJOYSTATE(2), X the wheel, Y and Z released
    /// (their minimum).
    unsafe extern "system" fn get_device_state(_this: P, size: u32, data: *mut u8) -> i32 {
        if data.is_null() {
            return DI_OK;
        }
        let stick = crate::input(0);
        let axes = std::array::from_fn::<_, 3, _>(|i| {
            let mapped = INPUT_AXES[i].load(Ordering::Relaxed);
            let value = if mapped & 0x7f == 0x7f { 0 } else {
                let value = stick.axis_u16(Axis::ALL[(mapped & 0x7f) as usize]);
                if mapped & 0x80 != 0 { !value } else { value }
            };
            scaled(i, value)
        });
        {
            // Diagnostic: the values the game reads from the wheel, at most twice a second.
            static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let last = LAST.load(Ordering::Relaxed);
            if now - last >= 500 {
                LAST.store(now, Ordering::Relaxed);
                log!("dinput: wheel state X={} Y={} Z={} (steer/brake/accel from stick lx={} accel={} brake={})",
                    axes[0], axes[1], axes[2],
                    stick.axis(Axis::LeftX), stick.axis(Axis::Accel), stick.axis(Axis::Brake));
            }
        }
        unsafe {
            std::ptr::write_bytes(data, 0, size as usize);
            for (i, a) in axes.iter().enumerate() {
                if 4 * (i + 1) <= size as usize {
                    (data.add(4 * i) as *mut i32).write_unaligned(*a);
                }
            }
            // POV hats (DIJOYSTATE + 0x20) centered
            for i in 0..4 {
                if 0x20 + 4 * (i + 1) <= size as usize {
                    (data.add(0x20 + 4 * i) as *mut u32).write_unaligned(u32::MAX);
                }
            }
            for i in 0..8 {
                let value = ((stick.buttons >> (i + 4)) & 1) as u8 * 0x80;
                if 0x30 + i < size as usize { *data.add(0x30 + i) = value; }
            }
            for i in 0..4 {
                let value = ((stick.buttons >> (i + 12)) & 1) as u8 * 0x80;
                if 0x38 + i < size as usize { *data.add(0x38 + i) = value; }
            }
        }
        DI_OK
    }

    /// `GetDeviceInfo(pdidi)`: DIDEVICEINSTANCEA or W, by its dwSize.
    unsafe extern "system" fn get_device_info(_this: P, info: *mut u8) -> i32 {
        if info.is_null() {
            return super::DIERR_UNSUPPORTED;
        }
        let wide = unsafe { (info as *const u32).read_unaligned() } == 0x44C;
        unsafe { device_instance(info, wide) };
        DI_OK
    }

    // ---- Force feedback -----------------------------------------------------
    //
    // The cabinet wheel's motor is driven by the launcher from the game's memory float; the
    // game side only needs the DirectInput effect interface to succeed. The predefined effect
    // GUIDs of di.h, with their `DIEFT_*` type in the low byte.
    const EFFECTS: &[(u32, u32)] = &[
        (0x1354_1C20, 0x01), // constant force
        (0x1354_1C21, 0x02), // ramp
        (0x1354_1C22, 0x04), // square
        (0x1354_1C23, 0x04), // sine
        (0x1354_1C24, 0x04), // triangle
        (0x1354_1C25, 0x04), // sawtooth up
        (0x1354_1C26, 0x04), // sawtooth down
        (0x1354_1C27, 0x08), // spring
        (0x1354_1C28, 0x08), // damper
        (0x1354_1C29, 0x08), // inertia
        (0x1354_1C2A, 0x08), // friction
        (0x1354_1C2B, 0x10), // custom
    ];

    fn effect_guid(first: u32) -> [u8; 16] {
        let [a, b, c, d] = first.to_le_bytes();
        [a, b, c, d, 0x33, 0x8E, 0x33, 0x11, 0xD0, 0x9A, 0xD0, 0x00, 0xA0, 0xC9, 0xA0, 0x6E]
    }

    /// `sizeof(DIEFFECT)` with `dwStartDelay` (DirectX 6+).
    const DIEFFECT_SIZE: u32 = 0x38;

    /// The GUID of the effect created last, for `GetEffectGuid`.
    static EFFECT_GUID: std::sync::Mutex<[u8; 16]> = std::sync::Mutex::new([0u8; 16]);
    /// An effect was created by the game.
    static EFFECT_CREATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    static EFFECT: Object = Object { vtable: EFFECT_VTABLE.0.as_ptr() };
    static EFFECT_VTABLE: VTable<13> = VTable([
        query_interface as *const (),
        ref_count as *const (), // AddRef
        ref_count as *const (), // Release
        ok3 as *const (), // Initialize
        effect_get_guid as *const (),
        ok2 as *const (), // GetParameters: keep the game's values
        ok2 as *const (), // SetParameters
        ok2 as *const (), // Start
        ok0 as *const (), // Stop
        effect_status as *const (),
        ok0 as *const (), // Download
        ok0 as *const (), // Unload
        ok1 as *const (), // Escape
    ]);

    unsafe extern "system" fn effect_get_guid(_this: P, out: P) -> i32 {
        if !out.is_null() {
            unsafe { std::ptr::copy_nonoverlapping(EFFECT_GUID.lock().unwrap().as_ptr(), out as *mut u8, 16); }
        }
        DI_OK
    }

    /// `GetEffectStatus(lpdwStatus)`: the effect is playing.
    unsafe extern "system" fn effect_status(_this: P, out: P) -> i32 {
        if !out.is_null() {
            unsafe { (out as *mut u32).write_unaligned(1) };
        }
        DI_OK
    }

    /// `CreateEffect(rguidSrc, lpdiEffect, lplpDirectInputEffect)`
    unsafe extern "system" fn create_effect(_this: P, guid: *const u8, _effect: P, out: *mut P) -> i32 {
        if out.is_null() {
            return super::DIERR_DEVICENOTREG;
        }
        if !guid.is_null() {
            unsafe { EFFECT_GUID.lock().unwrap().copy_from_slice(std::slice::from_raw_parts(guid, 16)); }
        }
        if EFFECT_CREATED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log!("dinput: another fake FFB effect created");
        } else {
            log!("dinput: fake FFB effect created");
        }
        unsafe { *out = &EFFECT as *const Object as P };
        DI_OK
    }

    /// Fills a zeroed `DIEFFECT` with a predefined effect's defaults.
    unsafe fn effect_struct(out: *mut u8, flags: u32) {
        unsafe {
            std::ptr::write_bytes(out, 0, DIEFFECT_SIZE as usize);
            (out as *mut u32).write_unaligned(DIEFFECT_SIZE);
            out.add(4).cast::<u32>().write_unaligned(flags); // dwFlags
            out.add(8).cast::<u32>().write_unaligned(1_000_000); // dwDuration, 1 s
            out.add(20).cast::<u32>().write_unaligned(0xFFFF); // dwGain, 100%
            out.add(24).cast::<u32>().write_unaligned(1); // cAxes
            out.add(28).cast::<u32>().write_unaligned(0); // rgdwAxes[0] = X
        }
    }

    /// `EnumEffects(lpCallback, pvRef, dwFlags)`: the predefined effect types.
    unsafe extern "system" fn enum_effects(_this: P, cb: EnumCallback, context: P, flags: u32) -> i32 {
        let mut effect = [0u8; 0x40];
        for &(_, type_flags) in EFFECTS {
            if flags != 0 && flags & 0xFF != type_flags & 0xFF {
                continue;
            }
            let stop = unsafe {
                effect_struct(effect.as_mut_ptr(), type_flags);
                cb(effect.as_ptr(), context) == 0 // DIENUM_STOP
            };
            if stop {
                break;
            }
        }
        DI_OK
    }

    /// `GetEffectInfo(dwEffectType, lpguid, lpeffect)`
    unsafe extern "system" fn get_effect_info(_this: P, r#type: u32, guid: *mut u8, effect: *mut u8) -> i32 {
        let Some(&(first, flags)) = EFFECTS.iter().find(|(_, f)| f & 0xFF == r#type & 0xFF) else {
            return super::DIERR_UNSUPPORTED;
        };
        if !guid.is_null() {
            let g = effect_guid(first);
            unsafe { std::ptr::copy_nonoverlapping(g.as_ptr(), guid, 16) };
        }
        if !effect.is_null() {
            unsafe { effect_struct(effect, flags) };
        }
        DI_OK
    }

    /// `GetForceFeedbackState()`: FFB is active.
    unsafe extern "system" fn ffb_state(_this: P) -> i32 {
        1
    }

    /// `EnumCreatedEffectObjects(lpCallback, pvRef, dwFlags)`
    unsafe extern "system" fn enum_created(_this: P, cb: EnumCallback, context: P, _flags: u32) -> i32 {
        if EFFECT_CREATED.load(std::sync::atomic::Ordering::Relaxed) {
            unsafe { cb(&EFFECT as *const Object as *const u8, context) };
        }
        DI_OK
    }

    static WHEEL_A: Object = Object { vtable: WHEEL_A_VTABLE.0.as_ptr() };
    static WHEEL_W: Object = Object { vtable: WHEEL_W_VTABLE.0.as_ptr() };
    static WHEEL_A_VTABLE: VTable<32> = wheel_vtable(enum_objects_a as *const ());
    static WHEEL_W_VTABLE: VTable<32> = wheel_vtable(enum_objects_w as *const ());

    const fn wheel_vtable(enum_objects: *const ()) -> VTable<32> {
        let mut v = DEVICE_VTABLE.0;
        v[3] = get_capabilities as *const ();
        v[4] = enum_objects;
        v[5] = get_property as *const ();
        v[6] = set_property as *const ();
        v[9] = get_device_state as *const ();
        v[15] = get_device_info as *const ();
        // DirectInput 7/8 force feedback
        v[18] = create_effect as *const ();
        v[19] = enum_effects as *const ();
        v[20] = get_effect_info as *const ();
        v[21] = ffb_state as *const ();
        v[23] = enum_created as *const ();
        VTable(v)
    }
}

/// The fake cabinet button box of `WAL_DINPUT_BOARD` (Injustice: 26 buttons).
mod board {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    use wal_protocol::button;

    use super::{DI_OK, DIERR_DEVICENOTREG, DEVICE_VTABLE, Object, P, VTable};
    use crate::log;

    /// The Injustice cabinet's DirectInput button box: 0 test,
    /// 1 service, 2/3 P1/P2 coin, 4 P1 start, 5-8 P1 b1-b4, 9 P1 b5 (buy card), 10 P1 card
    /// (reader), 11-13 P1 b6-b8 (cards interface), 14 P2 start, 15-18 P2 b1-b4, 19 P2 b5,
    /// 20 P2 card, 21-23 P2 b6-b8, 24/25 volume (unused).
    /// (player, protocol button) of each DirectInput button; a null button stays released.
    const SOURCES: [(usize, u32); 26] = [
        (0, button::TEST),
        (0, button::SERVICE),
        (0, button::COIN),
        (1, button::COIN),
        (0, button::START),
        (0, button::B1),
        (0, button::B2),
        (0, button::B3),
        (0, button::B4),
        (0, button::B5),
        (0, button::CARD),
        (0, button::B6),
        (0, button::B7),
        (0, button::B8),
        (1, button::START),
        (1, button::B1),
        (1, button::B2),
        (1, button::B3),
        (1, button::B4),
        (1, button::B5),
        (1, button::CARD),
        (1, button::B6),
        (1, button::B7),
        (1, button::B8),
        (0, 0),
        (0, 0),
    ];
    static NAME: OnceLock<String> = OnceLock::new();

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
        let a = d1.to_le_bytes();
        let b = d2.to_le_bytes();
        let c = d3.to_le_bytes();
        [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
    }
    pub(super) const INSTANCE: [u8; 16] = guid(0x5741_4C57, 0x424F, 0x4152, *b"WALBOARD");
    /// DIDEVTYPE_HID_JOYSTICK (HID usage page 1, usage 4).
    const DEV_TYPE: u32 = 0x105;
    /// GUID_Button, the buttons' objects' GUID.
    const BUTTON_GUID: [u8; 16] = guid(0xA36D_02F0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
    /// DIDFT_BUTTON.
    const DIDFT_BUTTON: u32 = 4;
    /// DIDC_ATTACHED | DIDC_POLLEDDEVICE.
    const CAPS_FLAGS: u32 = 1 | 0x200;

    pub(super) fn init(name: String) {
        let product = if name == "1" { "WAL Board".to_string() } else { name };
        log!("dinput: board {product:?}");
        let _ = NAME.set(product);
    }

    fn name() -> &'static str {
        NAME.get().map_or("WAL Board", |n| n.as_str())
    }

    /// Copies `s` into a fixed string field of `len` characters, ANSI or UTF-16.
    unsafe fn put_str(dst: *mut u8, len: usize, s: &str, wide: bool) {
        unsafe {
            if wide {
                let dst = dst as *mut u16;
                for (i, c) in s.encode_utf16().take(len - 1).chain(std::iter::once(0)).enumerate() {
                    dst.add(i).write_unaligned(c);
                }
            } else {
                for (i, c) in s.bytes().take(len - 1).chain(std::iter::once(0)).enumerate() {
                    *dst.add(i) = c;
                }
            }
        }
    }

    /// Fills a DIDEVICEINSTANCEA / W (`wide`), its dwSize included.
    unsafe fn device_instance(out: *mut u8, wide: bool) {
        let chars = if wide { 2 } else { 1 };
        let size = 4 + 36 + 260 * chars + 20;
        unsafe {
            std::ptr::write_bytes(out, 0, size);
            (out as *mut u32).write_unaligned(size as u32);
            std::ptr::copy_nonoverlapping(INSTANCE.as_ptr(), out.add(4), 16);
            std::ptr::copy_nonoverlapping(INSTANCE.as_ptr(), out.add(20), 16);
            (out.add(36) as *mut u32).write_unaligned(DEV_TYPE);
            put_str(out.add(40), 260, name(), wide);
            // guidSysID and dwDevNode stay zero
        }
    }

    type EnumCallback = unsafe extern "system" fn(*const u8, P) -> i32;

    /// Whether an `EnumDevices` type filter takes the board: all devices (and the HID
    /// super-type), the game controller class (DI8DEVCLASS_GAMECTRL), HID gamepad and
    /// joystick types, all input devices.
    fn listed(filter: u32) -> bool {
        matches!(filter & 0xFF, 0 | 0x1 | 0x4 | 0x5 | 0x80)
    }

    unsafe fn enum_devices(filter: u32, cb: EnumCallback, context: P, wide: bool) -> i32 {
        if listed(filter) {
            let mut instance = [0u8; 0x24C];
            unsafe {
                device_instance(instance.as_mut_ptr(), wide);
                cb(instance.as_ptr(), context);
            }
        }
        DI_OK
    }

    unsafe extern "system" fn enum_devices_a(_this: P, filter: u32, cb: EnumCallback, context: P, _flags: u32) -> i32 {
        unsafe { enum_devices(filter, cb, context, false) }
    }
    unsafe extern "system" fn enum_devices_w(_this: P, filter: u32, cb: EnumCallback, context: P, _flags: u32) -> i32 {
        unsafe { enum_devices(filter, cb, context, true) }
    }

    /// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`: the board for its instance
    /// GUID, the empty device otherwise.
    unsafe fn create_device(guid: *const u8, out: *mut P, wide: bool) -> i32 {
        if out.is_null() {
            return DIERR_DEVICENOTREG;
        }
        static CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if CREATED.fetch_add(1, Ordering::Relaxed) < 3 {
            let g = if guid.is_null() { "null".to_string() } else {
                let bytes = unsafe { std::slice::from_raw_parts(guid, 16) };
                bytes.iter().map(|b| format!("{b:02x}")).collect()
            };
            log!("dinput: board CreateDevice {g}");
        }
        let is_board = !guid.is_null() && unsafe { std::slice::from_raw_parts(guid, 16) } == INSTANCE;
        let device: &Object = match (is_board, wide) {
            (true, false) => &BOARD_A,
            (true, true) => &BOARD_W,
            (false, _) => &super::DEVICE,
        };
        unsafe { *out = device as *const Object as P };
        DI_OK
    }
    unsafe extern "system" fn create_device_a(_this: P, guid: *const u8, out: *mut P, _outer: P) -> i32 {
        unsafe { create_device(guid, out, false) }
    }
    unsafe extern "system" fn create_device_w(_this: P, guid: *const u8, out: *mut P, _outer: P) -> i32 {
        unsafe { create_device(guid, out, true) }
    }

    /// The fake IDirectInput8A / W with the board.
    pub(super) static DINPUT_A: Object = Object { vtable: DINPUT_A_VTABLE.0.as_ptr() };
    pub(super) static DINPUT_W: Object = Object { vtable: DINPUT_W_VTABLE.0.as_ptr() };
    static DINPUT_A_VTABLE: VTable<11> = dinput_vtable(create_device_a as *const (), enum_devices_a as *const ());
    static DINPUT_W_VTABLE: VTable<11> = dinput_vtable(create_device_w as *const (), enum_devices_w as *const ());

    const fn dinput_vtable(create: *const (), enumerate: *const ()) -> VTable<11> {
        let mut v = super::DINPUT_VTABLE.0;
        v[3] = create;
        v[4] = enumerate;
        VTable(v)
    }

    /// `GetCapabilities(lpDIDevCaps)`: DIDEVCAPS, attached, polled, 26 buttons, no axes.
    unsafe extern "system" fn get_capabilities(_this: P, caps: *mut u32) -> i32 {
        if !caps.is_null() {
            unsafe {
                let size = (*caps).clamp(4, 44) as usize;
                std::ptr::write_bytes(caps.add(1) as *mut u8, 0, size - 4);
                // dwFlags, dwDevType, dwAxes, dwButtons, dwPOVs
                for (i, v) in [CAPS_FLAGS, DEV_TYPE, 0, SOURCES.len() as u32, 0].into_iter().enumerate() {
                    if 4 * (i + 2) <= size {
                        *caps.add(i + 1) = v;
                    }
                }
            }
        }
        DI_OK
    }

    /// The buttons' offsets a `SetDataFormat` installed (button index -> buffer offset): the
    /// game may read the 26 buttons with its own data format (the default DIJOYSTATE layout
    /// is 0x30 + index).
    static FORMAT: Mutex<Vec<(u16, u32)>> = Mutex::new(Vec::new());

    /// `SetDataFormat(lpdformat)`: capture the buttons' offsets.
    unsafe extern "system" fn set_data_format(_this: P, format: *const u8) -> i32 {
        if !format.is_null() {
            let count = unsafe { (format.add(16) as *const u32).read_unaligned() } as usize;
            let entries = unsafe { format.add(48) };
            let mut buttons = Vec::new();
            for i in 0..count.min(64) {
                let e = unsafe { entries.add(i * 24) };
                let object = unsafe { std::slice::from_raw_parts(e, 16) };
                let (ofs, r#type) =
                    unsafe { ((e.add(16) as *const u32).read_unaligned(), (e.add(20) as *const u32).read_unaligned()) };
                if r#type & DIDFT_BUTTON != 0 && object == BUTTON_GUID {
                    buttons.push(((r#type >> 8 & 0xFFFF) as u16, ofs));
                }
            }
            buttons.sort_by_key(|b| b.0);
            *FORMAT.lock().unwrap() = buttons.clone();
            if buttons.is_empty() {
                log!("dinput: board data format without buttons");
            } else {
                let first = format!("{:#x}", buttons[0].1);
                log!("dinput: board data format, {} buttons (first at {first})", buttons.len());
            }
        }
        DI_OK
    }

    /// `EnumObjects(lpCallback, pvRef, dwFlags)`: the 26 buttons (when asked).
    unsafe fn enum_objects(cb: EnumCallback, context: P, flags: u32, wide: bool) -> i32 {
        if flags != 0 && flags & DIDFT_BUTTON == 0 {
            return DI_OK;
        }
        let chars = if wide { 2 } else { 1 };
        let size = 32 + 260 * chars + 24;
        let mut object = [0u8; 0x240];
        for i in 0..SOURCES.len() {
            object.fill(0);
            let o = object.as_mut_ptr();
            unsafe {
                (o as *mut u32).write_unaligned(size as u32);
                std::ptr::copy_nonoverlapping(BUTTON_GUID.as_ptr(), o.add(4), 16);
                (o.add(20) as *mut u32).write_unaligned(0x30 + i as u32);
                (o.add(24) as *mut u32).write_unaligned(DIDFT_BUTTON | (i as u32) << 8);
                let label = format!("Button {}", i + 1);
                put_str(o.add(32), 260, &label, wide);
                if cb(o, context) == 0 {
                    break; // DIENUM_STOP
                }
            }
        }
        DI_OK
    }
    unsafe extern "system" fn enum_objects_a(_this: P, cb: EnumCallback, context: P, flags: u32) -> i32 {
        unsafe { enum_objects(cb, context, flags, false) }
    }
    unsafe extern "system" fn enum_objects_w(_this: P, cb: EnumCallback, context: P, flags: u32) -> i32 {
        unsafe { enum_objects(cb, context, flags, true) }
    }

    /// At most one log line per `min_ms`.
    fn log_throttle(min_ms: u64) -> bool {
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let last = LAST.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= min_ms {
            LAST.store(now, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// `GetDeviceState(cbData, lpvData)`: the cabinet's buttons from the virtual sticks.
    unsafe extern "system" fn get_device_state(_this: P, size: u32, data: *mut u8) -> i32 {
        if data.is_null() {
            return DI_OK;
        }
        let n = size as usize;
        unsafe { std::ptr::write_bytes(data, 0, n) };
        let (stick1, stick2) = (crate::input(0), crate::input(1));
        let format = FORMAT.lock().unwrap();
        let mut pressed = [false; SOURCES.len()];
        for (i, &(player, bit)) in SOURCES.iter().enumerate() {
            if bit == 0 {
                continue;
            }
            let down = if player == 0 { stick1.pressed(bit) } else { stick2.pressed(bit) };
            if !down {
                continue;
            }
            pressed[i] = true;
            let ofs = format.iter().find(|(idx, _)| *idx == i as u16).map_or(0x30 + i as u32, |(_, o)| *o);
            if (ofs as usize) < n {
                unsafe { data.add(ofs as usize).write(0x80) };
            }
        }
        if pressed.iter().any(|&p| p) && log_throttle(1000) {
            let bits: Vec<String> = pressed.iter().enumerate().filter_map(|(i, p)| p.then_some(i.to_string())).collect();
            log!("dinput: board buttons {}", bits.join(","));
        }
        DI_OK
    }

    /// `GetDeviceInfo(pdidi)`: DIDEVICEINSTANCEA or W, by its dwSize.
    unsafe extern "system" fn get_device_info(_this: P, info: *mut u8) -> i32 {
        if info.is_null() {
            return super::DIERR_UNSUPPORTED;
        }
        let dw = unsafe { (info as *const u32).read_unaligned() };
        let wide = dw == 0x24C || dw == 0x44C;
        unsafe { device_instance(info, wide) };
        DI_OK
    }

    static BOARD_A: Object = Object { vtable: BOARD_A_VTABLE.0.as_ptr() };
    static BOARD_W: Object = Object { vtable: BOARD_W_VTABLE.0.as_ptr() };
    static BOARD_A_VTABLE: VTable<32> = board_vtable(enum_objects_a as *const ());
    static BOARD_W_VTABLE: VTable<32> = board_vtable(enum_objects_w as *const ());

    const fn board_vtable(enum_objects: *const ()) -> VTable<32> {
        let mut v = DEVICE_VTABLE.0;
        v[3] = get_capabilities as *const ();
        v[4] = enum_objects;
        v[9] = get_device_state as *const ();
        v[11] = set_data_format as *const ();
        v[15] = get_device_info as *const ();
        VTable(v)
    }
}
