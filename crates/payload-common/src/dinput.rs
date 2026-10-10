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
//! 1's `lx` (`WAL_DINPUT_WHEEL_AXIS`, `-` inverts), Y and Z stay released, no buttons. D1GP
//! Arcade's cabinet wheel is a USB "Immersion TouchSense Steering Wheel": the game steers with
//! its X axis and takes the I/O board's switches only while such a device is present.
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
    let dinput = match (std::env::var_os("WAL_DINPUT_WHEEL").is_some(), wide) {
        (true, false) => &wheel::DINPUT_A,
        (true, true) => &wheel::DINPUT_W,
        (false, _) => &DINPUT,
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
    let wheel = std::env::var("WAL_DINPUT_WHEEL").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if wheel.is_none() && !std::env::var("WAL_DINPUT_DISABLE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe {
        iat::hook("dinput.dll", "DirectInputCreateA", create as *const () as usize);
        iat::hook("dinput.dll", "DirectInputCreateW", create_w as *const () as usize);
        iat::hook("dinput.dll", "DirectInputCreateEx", create_iid as *const () as usize);
        iat::hook("dinput8.dll", "DirectInput8Create", create_iid as *const () as usize);
    }
    match wheel {
        Some(name) => {
            wheel::init(name);
            log!("dinput: fake DirectInput with a wheel");
        }
        None => log!("dinput: disabled (fake DirectInput without devices)"),
    }
}

/// The fake wheel of `WAL_DINPUT_WHEEL`.
mod wheel {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};

    use wal_protocol::Axis;

    use super::{DEVICE_VTABLE, DI_OK, Object, P, VTable};
    use crate::log;

    static NAME: OnceLock<String> = OnceLock::new();
    /// The steering axis (`Axis` index), bit 7 set when inverted.
    static STEER: AtomicU8 = AtomicU8::new(Axis::LeftX as u8);
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

    /// `GetCapabilities(lpDIDevCaps)`: DIDEVCAPS, attached, 3 axes.
    unsafe extern "system" fn get_capabilities(_this: P, caps: *mut u32) -> i32 {
        if !caps.is_null() {
            unsafe {
                let size = (*caps).clamp(4, 44) as usize;
                std::ptr::write_bytes(caps.add(1) as *mut u8, 0, size - 4);
                // dwFlags DIDC_ATTACHED, dwDevType, dwAxes, dwButtons, dwPOVs
                for (i, v) in [1, DEV_TYPE, 3, 0, 0].into_iter().enumerate() {
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
        if flags != 0 && flags & 3 == 0 {
            return DI_OK;
        }
        let chars = if wide { 2 } else { 1 };
        let size = 32 + 260 * chars + 24;
        let mut object = [0u8; 0x240];
        for i in 0..3 {
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
        let s = STEER.load(Ordering::Relaxed);
        let v = crate::input(0).axis_u16(Axis::ALL[(s & 0x7F) as usize]);
        let v = if s & 0x80 != 0 { !v } else { v };
        let axes = [scaled(0, v), scaled(1, 0), scaled(2, 0)];
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
        VTable(v)
    }
}
