//! `WAL_DINPUT_DISABLE=1`: the game gets a DirectInput that works but sees no input, so only
//! its I/O board drives it. Some arcade builds still read the keyboard and joysticks through
//! DirectInput (debug or operator keys): Raiden IV (Type X) enters its test menu on the PC
//! keyboard's `2`, which the launcher also maps to P2 start (the keyboard is not grabbed, the
//! game window sees it).
//!
//! `DirectInputCreateA/W/Ex` and `DirectInput8Create` return a static fake interface (the
//! IDirectInput8 vtable, a superset of the older ones): `EnumDevices` lists nothing (the system
//! keyboard and mouse once a payload set up the keyboard: [`set_keyboard`]),
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
//! motor that is not there. `WAL_DINPUT_WHEEL_Y_AXIS` / `_Z_` / `_RX_` / `_RY_` / `_RZ_AXIS`
//! map more axes (Rx, Ry, Rz are listed only when mapped: GRID's "SEGA IO Controller" has its
//! gas on Rx); `WAL_DINPUT_WHEEL_BUTTONS=<virtual button>,...` gives the buttons in order
//! (`none` for an unused one, `b1+start` pressed by either; default: b1-b8, start, coin,
//! service, test).
//! `WAL_DINPUT_WHEEL_FFB=0`: no force feedback (`CreateEffect` unsupported).
//! `WAL_DINPUT_WHEEL_FFB_OUT=1`: the game's constant force effect (its magnitude, gain and
//! direction, started / stopped) sent to the launcher's wheel (`output::FFB_CONSTANT`; VCThunder).
//! The wheel's buffered data (`GetDeviceData`) reports its objects' changes;
//! `WAL_DINPUT_WHEEL_HEARTBEAT_MS=<ms>` re-reports every axis that often (Visual Pinball takes
//! an axis silent for 3 s as its nudge board unplugged).
//! `WAL_DINPUT_WHEEL_<X|Y|Z|RX|RY|RZ>_REST=<0..1>`: where the axis rests in its raw range (a
//! spring plunger near its end), each half of the stick spanning one side. The wheel keeps the
//! calibration a game sets (`DIPROP_CALIBRATION`, in the registry, for the other programs) and
//! applies it, raw values in `DIPROP_CALIBRATIONMODE` raw; its names (`DIPROP_PRODUCTNAME` /
//! `DIPROP_INSTANCENAME`) are its product name.
//! `WAL_DINPUT_WHEEL_VIDPID=<vid>:<pid>` (hex): the USB ids in its product GUID (Blazing Angels
//! picks its controls layout by the vendor id: the cabinet board's, 1ab7).
//! `WAL_DINPUT_WHEEL_BUTTON_BASE=0`: its buttons named from `Button 0` (Windows' names), else
//! from `Button 1`. `WAL_DINPUT_WHEEL_JOYSTICK=1`: a joystick, not a driving device.
//! `WAL_DINPUT_WHEEL_SYSTEM=1`: the system keyboard and mouse of
//! [`set_keyboard`] listed and created next to it (Ballistics binds its cabinet's GameFrame by
//! its product name and buttons' names, and stops without a keyboard device).
//!
//! `WAL_DINPUT_PADS=<n>`: the same fake DirectInput, with `n` joysticks (players 1 to `n`, at
//! most 4): X and Y from the player's d-pad, else its left stick, and the buttons
//! `WAL_DINPUT_PADS_BUTTONS=<virtual button>,...` in order (as `WAL_DINPUT_WHEEL_BUTTONS`;
//! default b1-b8, start, coin). Justice League: Heroes United reads its cabinet's two control
//! panels as DirectInput joysticks.
//!
//! `WAL_DINPUT_BOARD=<product name or 1>`: the same fake DirectInput, with one cabinet button
//! box (26 buttons, the Injustice cabinet's layout): the players' fight buttons, coins, start,
//! the card dispensers' buy buttons and the card readers' scan buttons, from the virtual
//! sticks.
//!
//! [`set_keyboard`]: a payload gives the fake DirectInput a system keyboard whose key states it
//! fills from the virtual sticks (Contra: Evolution plays on its DirectInput keyboard keys and
//! needs the keyboard device to work: `GetDeviceInfo` answered). `EnumDevicesBySemantics` lists
//! it then, its action map accepted (Need for Speed Underground: DirectInput 8 action mapping).
//!
//! Games creating DirectInput as a COM object (`CoCreateInstance(CLSID_DirectInput)`, then
//! `Initialize`: the Virtools player's input manager, After Dark) get the same fake one.
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

// An enumeration callback called with the caller's stack pointer kept: the callbacks are stdcall,
// but some games declare theirs cdecl (Blazing Angels' calibration page), which leaves its two
// arguments on the stack; real DirectInput copes with it.
#[cfg(target_arch = "x86")]
core::arch::global_asm!(
    ".globl {stub}",
    "{stub}:",
    "push ebp",
    "mov ebp, esp",
    "push dword ptr [ebp + 16]",
    "push dword ptr [ebp + 12]",
    "call dword ptr [ebp + 8]",
    "mov esp, ebp",
    "pop ebp",
    "ret",
    stub = sym wal_dinput_enum_callback,
);

#[cfg(target_arch = "x86")]
unsafe extern "C" {
    fn wal_dinput_enum_callback(cb: usize, item: *const u8, context: P) -> i32;
}

/// Calls an enumeration callback `BOOL cb(item, context)`, whatever its calling convention.
unsafe fn enum_callback(cb: unsafe extern "system" fn(*const u8, P) -> i32, item: *const u8, context: P) -> i32 {
    #[cfg(target_arch = "x86")]
    return unsafe { wal_dinput_enum_callback(cb as usize, item, context) };
    #[cfg(not(target_arch = "x86"))]
    return unsafe { cb(item, context) };
}
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
/// share the first slots), but the joystick configuration ones (IDirectInputJoyConfig and
/// IDirectInputJoyConfig8: Hello! Pop'n Music asks for its joysticks' config, calling past
/// the object's slots otherwise).
unsafe extern "system" fn query_interface(this: P, iid: P, out: *mut P) -> i32 {
    const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
    const JOY_CONFIG: [[u8; 16]; 2] = [
        // eb0d7dfa-1990-4f27-b4d6-edf2eec4a44c
        [0xfa, 0x7d, 0x0d, 0xeb, 0x90, 0x19, 0x27, 0x4f, 0xb4, 0xd6, 0xed, 0xf2, 0xee, 0xc4, 0xa4, 0x4c],
        // 1de12ab1-c9f5-11cf-bfc7-444553540000
        [0xb1, 0x2a, 0xe1, 0x1d, 0xf5, 0xc9, 0xcf, 0x11, 0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
    ];
    if !iid.is_null() && JOY_CONFIG.contains(unsafe { &*(iid as *const [u8; 16]) }) {
        if !out.is_null() {
            unsafe { *out = std::ptr::null_mut() };
        }
        return E_NOINTERFACE;
    }
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

/// The payload's source of the system keyboard's key states ([`set_keyboard`]).
static KEYBOARD: std::sync::OnceLock<fn(&mut [u8; 256])> = std::sync::OnceLock::new();

/// The fake DirectInput's system keyboard: `keys` fills its 256 key states (0x80 a key down,
/// by DIK code) at each `GetDeviceState`. Call before the game creates the device.
pub fn set_keyboard(keys: fn(&mut [u8; 256])) {
    let _ = KEYBOARD.set(keys);
}

/// The keyboard's `GetDeviceState(cbData, lpvData)`: the payload's key states (the other
/// devices': nothing pressed).
unsafe extern "system" fn keyboard_state(this: P, size: u32, data: *mut u8) -> i32 {
    let mut keys = [0u8; 256];
    if std::ptr::eq(this.cast_const().cast(), &KEYBOARD_DEVICE)
        && let Some(fill) = KEYBOARD.get()
    {
        fill(&mut keys);
    }
    if !data.is_null() {
        unsafe {
            std::ptr::write_bytes(data, 0, size as usize);
            std::ptr::copy_nonoverlapping(keys.as_ptr(), data, (size as usize).min(256));
        }
    }
    DI_OK
}

/// `GetDeviceInfo(DIDEVICEINSTANCEA/W)`: a DirectInput 8 keyboard or mouse (dwDevType at +0x24,
/// after dwSize and the two GUIDs).
unsafe extern "system" fn keyboard_info(this: P, info: *mut u32) -> i32 {
    const DI8DEVTYPE_MOUSE: u32 = 0x12;
    const DI8DEVTYPE_KEYBOARD: u32 = 0x13;
    let keyboard = std::ptr::eq(this.cast_const().cast(), &KEYBOARD_DEVICE);
    if !info.is_null() {
        unsafe {
            let size = (*info).clamp(4, 0x244) as usize;
            std::ptr::write_bytes(info.cast::<u8>().add(4), 0, size - 4);
            if size >= 0x28 {
                *info.cast::<u8>().add(0x24).cast::<u32>() = if keyboard { DI8DEVTYPE_KEYBOARD } else { DI8DEVTYPE_MOUSE };
            }
        }
    }
    DI_OK
}

/// The system keyboard of [`set_keyboard`] and the system mouse next to it (the game sets up
/// both): their state and info answered.
static KEYBOARD_VTABLE: VTable<32> = {
    let mut v = DEVICE_VTABLE.0;
    v[9] = keyboard_state as *const ();
    v[15] = keyboard_info as *const ();
    v[29] = ok3 as *const (); // BuildActionMap
    v[30] = ok3 as *const (); // SetActionMap
    VTable(v)
};
static KEYBOARD_DEVICE: Object = Object { vtable: KEYBOARD_VTABLE.0.as_ptr() };
static MOUSE_DEVICE: Object = Object { vtable: KEYBOARD_VTABLE.0.as_ptr() };

/// `GUID_SysKeyboard` {6F1D2B61-D5A0-11CF-BFC7-444553540000}, `GUID_SysMouse` {6F1D2B60-...}.
const GUID_SYS_KEYBOARD: [u8; 16] =
    [0x61, 0x2b, 0x1d, 0x6f, 0xa0, 0xd5, 0xcf, 0x11, 0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00];
const GUID_SYS_MOUSE: [u8; 16] =
    [0x60, 0x2b, 0x1d, 0x6f, 0xa0, 0xd5, 0xcf, 0x11, 0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00];

/// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`
unsafe extern "system" fn create_device(_this: P, guid: *const [u8; 16], out: *mut P, _outer: P) -> i32 {
    let system = if KEYBOARD.get().is_some() && !guid.is_null() { Some(unsafe { *guid }) } else { None };
    if !out.is_null() {
        let device = match system {
            Some(GUID_SYS_KEYBOARD) => &KEYBOARD_DEVICE,
            Some(GUID_SYS_MOUSE) => &MOUSE_DEVICE,
            _ => &DEVICE,
        };
        unsafe { *out = device as *const Object as P };
    }
    DI_OK
}

/// `EnumDevicesBySemantics(user, lpdiActionFormat, callback, ref, flags)`: the system keyboard of
/// [`set_keyboard`] (its action map built and set, never any data), else nothing. Need for Speed
/// Underground's race sets up its controls through DirectInput 8 action mapping, and reads its
/// cabinet's wheel only from its keyboard's update.
/// `EnumDevices(dwDevType, lpCallback, pvRef, dwFlags)`: the system keyboard of
/// [`set_keyboard`] and the mouse next to it, when the type filter takes them (all devices,
/// keyboards, pointers or their exact type); else none. Need for Speed Carbon's input manager
/// reads the cabinet only with a keyboard and a mouse listed.
unsafe extern "system" fn enum_devices(_this: P, filter: u32, cb: P, ctx: P, _flags: u32) -> i32 {
    if KEYBOARD.get().is_none() || cb.is_null() {
        return DI_OK;
    }
    type Callback = unsafe extern "system" fn(*const u8, P) -> i32;
    let cb: Callback = unsafe { std::mem::transmute(cb) };
    // (guid, dwDevType: DI8DEVTYPE_KEYBOARD / PCENH, DI8DEVTYPE_MOUSE / TRADITIONAL, class, name)
    let devices: [(&[u8; 16], u32, u32, &[u8]); 2] =
        [(&GUID_SYS_KEYBOARD, 0x0413, 3, b"Keyboard"), (&GUID_SYS_MOUSE, 0x0212, 2, b"Mouse")];
    for (guid, devtype, class, name) in devices {
        let f = filter & 0xff;
        if f != 0 && f != class && f != devtype & 0xff {
            continue;
        }
        // DIDEVICEINSTANCEA: dwSize, guidInstance, guidProduct, dwDevType, tszInstanceName[260],
        // tszProductName[260], guidFFDriver, wUsagePage, wUsage
        let mut instance = [0u8; 580];
        instance[0..4].copy_from_slice(&580u32.to_le_bytes());
        instance[4..20].copy_from_slice(guid);
        instance[20..36].copy_from_slice(guid);
        instance[36..40].copy_from_slice(&devtype.to_le_bytes());
        instance[40..40 + name.len()].copy_from_slice(name);
        instance[300..300 + name.len()].copy_from_slice(name);
        let more = unsafe { cb(instance.as_ptr(), ctx) };
        log!("dinput: EnumDevices(type {filter:#x}): {}", String::from_utf8_lossy(name));
        if more == 0 {
            break;
        }
    }
    DI_OK
}

unsafe extern "system" fn enum_by_semantics(_this: P, _user: P, _format: P, cb: P, ctx: P, _flags: u32) -> i32 {
    if KEYBOARD.get().is_none() || cb.is_null() {
        log!("dinput: EnumDevicesBySemantics: none");
        return DI_OK;
    }
    // DIDEVICEINSTANCEA: dwSize, guidInstance, guidProduct, dwDevType, tszInstanceName[260],
    // tszProductName[260], guidFFDriver, wUsagePage, wUsage
    let mut instance = [0u8; 580];
    instance[0..4].copy_from_slice(&580u32.to_le_bytes());
    instance[4..20].copy_from_slice(&GUID_SYS_KEYBOARD);
    instance[20..36].copy_from_slice(&GUID_SYS_KEYBOARD);
    // DI8DEVTYPE_KEYBOARD, DI8DEVTYPEKEYBOARD_PCENH
    instance[36..40].copy_from_slice(&0x0413u32.to_le_bytes());
    instance[40..48].copy_from_slice(b"Keyboard");
    instance[300..308].copy_from_slice(b"Keyboard");
    type Callback = unsafe extern "system" fn(*const u8, P, u32, u32, P) -> i32;
    let cb: Callback = unsafe { std::mem::transmute(cb) };
    // DIEDBS_MAPPEDPRI1, no device left
    unsafe { cb(instance.as_ptr(), &KEYBOARD_DEVICE as *const Object as P, 1, 0, ctx) };
    log!("dinput: EnumDevicesBySemantics: the keyboard");
    DI_OK
}

/// IDirectInput8 (IDirectInputA/2/7 share the first 9 slots; IDirectInput7's CreateDeviceEx
/// sits where EnumDevicesBySemantics is: [`seven`]).
static DINPUT_VTABLE: VTable<11> = VTable([
    query_interface as *const (),
    ref_count as *const (), // AddRef
    ref_count as *const (), // Release
    create_device as *const (),
    enum_devices as *const (),
    unsupported1 as *const (), // GetDeviceStatus
    ok2 as *const (), // RunControlPanel
    ok2 as *const (), // Initialize
    unsupported3 as *const (), // FindDevice
    enum_by_semantics as *const (), // EnumDevicesBySemantics
    unsupported4 as *const (), // ConfigureDevices
]);
static DINPUT: Object = Object { vtable: DINPUT_VTABLE.0.as_ptr() };

/// `IDirectInput7::CreateDeviceEx(rguid, riid, ppv, outer)` (the slot of IDirectInput8's
/// EnumDevicesBySemantics): the interface's own `CreateDevice` (slot 3), the device interface
/// asked for being the fake device's (a superset of the older ones).
unsafe extern "system" fn create_device_ex(this: P, guid: *const u8, _iid: *const u8, out: *mut P, outer: P) -> i32 {
    type CreateDevice = unsafe extern "system" fn(P, *const u8, *mut P, P) -> i32;
    let create: CreateDevice = unsafe { std::mem::transmute(*(*(this as *const Object)).vtable.add(3)) };
    unsafe { create(this, guid, out, outer) }
}

/// The IDirectInput7 of a fake IDirectInput8 (`dinput`): its vtable with CreateDeviceEx in
/// slot 9 (Visual Pinball creates its devices that way), made once per interface.
fn seven(dinput: &'static Object) -> &'static Object {
    static MADE: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    let key = dinput as *const Object as usize;
    if let Some(&(_, object)) = made.iter().find(|(k, _)| *k == key) {
        return unsafe { &*(object as *const Object) };
    }
    let mut slots = [std::ptr::null::<()>(); 11];
    for (i, slot) in slots.iter_mut().enumerate() {
        *slot = unsafe { *dinput.vtable.add(i) };
    }
    slots[9] = create_device_ex as *const ();
    let vtable: &'static VTable<11> = Box::leak(Box::new(VTable(slots)));
    let object: &'static Object = Box::leak(Box::new(Object { vtable: vtable.0.as_ptr() }));
    made.push((key, object as *const Object as usize));
    object
}

/// `wide`: the game asked for a Unicode interface (its EnumDevices / EnumObjects structures).
fn fake(out: *mut P, wide: bool) -> i32 {
    fake_version(out, wide, false)
}

/// `di7`: the game asked for IDirectInput7 (CreateDeviceEx in slot 9).
fn fake_version(out: *mut P, wide: bool, di7: bool) -> i32 {
    if out.is_null() {
        return DIERR_DEVICENOTREG;
    }
    let pads = std::env::var_os("WAL_DINPUT_PADS").is_some();
    let dinput = match (
        std::env::var("WAL_DINPUT_BOARD").is_ok(),
        std::env::var_os("WAL_DINPUT_WHEEL").is_some(),
        wide,
    ) {
        _ if pads && !wide => &pads::DINPUT_A,
        _ if pads => &pads::DINPUT_W,
        (true, _, false) => &board::DINPUT_A,
        (true, _, true) => &board::DINPUT_W,
        (false, true, false) => &wheel::DINPUT_A,
        (false, true, true) => &wheel::DINPUT_W,
        (false, false, _) => &DINPUT,
    };
    let dinput = if di7 { seven(dinput) } else { dinput };
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
    // IID_IDirectInput7A / W
    let di7 = !iid.is_null() && matches!(unsafe { *iid }, 0x9A4C_B684 | 0x9A4C_B685);
    fake_version(out, wide, di7)
}

static ORIG_GET_PROC_ADDRESS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ORIG_CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `CLSID_DirectInput` / `CLSID_DirectInput8` (`25E609E0` / `25E609E1-B259-11CF-BFC7-444553540000`).
fn is_dinput_clsid(clsid: *const u8) -> bool {
    const TAIL: [u8; 12] = [0x59, 0xB2, 0xCF, 0x11, 0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00];
    if clsid.is_null() {
        return false;
    }
    let c = unsafe { std::slice::from_raw_parts(clsid, 16) };
    matches!(c[..4], [0xE0 | 0xE1, 0x09, 0xE6, 0x25]) && c[4..] == TAIL
}

/// `CoCreateInstance(clsid, outer, context, riid, out)`: DirectInput is the fake one.
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: P, context: u32, iid: *const u32, out: *mut P) -> i32 {
    if is_dinput_clsid(clsid) {
        return unsafe { create_iid(std::ptr::null_mut(), 0, iid, out, outer) };
    }
    let orig: unsafe extern "system" fn(*const u8, P, u32, *const u32, *mut P) -> i32 =
        unsafe { std::mem::transmute(ORIG_CO_CREATE_INSTANCE.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { orig(clsid, outer, context, iid, out) }
}

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

/// The fake DirectInput for one more module, loaded after the payload started and not from the
/// game's directory at load time (VCThunder's `VCThunder.dll`, loaded by its stub executable,
/// resolves `DirectInput8Create` at run time): its imports and `GetProcAddress`. After [`init`].
pub fn hook_module(module: usize) {
    if module == 0 {
        return;
    }
    unsafe {
        iat::hook_module(module, "dinput.dll", "DirectInputCreateA", create as *const () as usize);
        iat::hook_module(module, "dinput.dll", "DirectInputCreateW", create_w as *const () as usize);
        iat::hook_module(module, "dinput.dll", "DirectInputCreateEx", create_iid as *const () as usize);
        iat::hook_module(module, "dinput8.dll", "DirectInput8Create", create_iid as *const () as usize);
        let ours = get_proc_address as *const () as usize;
        if let Some(o) = iat::hook_module(module, "kernel32.dll", "GetProcAddress", ours).filter(|&o| o != ours) {
            let _ = ORIG_GET_PROC_ADDRESS.compare_exchange(0, o, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

pub fn init() {
    if std::env::var("WAL_KEYBOARD_STATE_DISABLE").is_ok_and(|v| v.trim() == "1") {
        unsafe { iat::hook("user32.dll", "GetKeyboardState", get_keyboard_state as *const () as usize) };
        log!("dinput: GetKeyboardState reports no key");
    }
    let board = std::env::var("WAL_DINPUT_BOARD").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let wheel = std::env::var("WAL_DINPUT_WHEEL").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let pads = std::env::var("WAL_DINPUT_PADS").ok().and_then(|n| n.trim().parse::<usize>().ok()).filter(|&n| n > 0);
    if pads.is_none() && board.is_none() && wheel.is_none() && !std::env::var("WAL_DINPUT_DISABLE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe {
        // the executable and the DLLs of its folder (Justice League: Heroes United's engine,
        // ElementalEngine.dll, imports DirectInput)
        for module in crate::workarea::game_modules(iat::game_module()) {
            iat::hook_module(module, "dinput.dll", "DirectInputCreateA", create as *const () as usize);
            iat::hook_module(module, "dinput.dll", "DirectInputCreateW", create_w as *const () as usize);
            iat::hook_module(module, "dinput.dll", "DirectInputCreateEx", create_iid as *const () as usize);
            iat::hook_module(module, "dinput8.dll", "DirectInput8Create", create_iid as *const () as usize);
            if let Some(o) = iat::hook_module(module, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) {
                if o != co_create_instance as *const () as usize {
                    ORIG_CO_CREATE_INSTANCE.store(o, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        // games loading DirectInput at run time (Ford Racing: LoadLibrary("dinput8.dll"))
        // init runs once per hooked module set (Mario Kart: twice): a second hook returns our
        // own function, which must not become the original (endless recursion).
        let ours = get_proc_address as *const () as usize;
        if let Some(o) = iat::hook("kernel32.dll", "GetProcAddress", ours).filter(|&o| o != ours) {
            ORIG_GET_PROC_ADDRESS.store(o, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if let Some(count) = pads {
        pads::init(count);
        return;
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
    /// The axes: X, Y, Z always listed, Rx, Ry, Rz when mapped.
    const AXIS_COUNT: usize = 6;
    /// Virtual input axes for X, Y, Z, Rx, Ry, Rz. 0xff means a parked, unused axis.
    static INPUT_AXES: [AtomicU8; AXIS_COUNT] = [
        AtomicU8::new(Axis::LeftX as u8),
        AtomicU8::new(0xff),
        AtomicU8::new(0xff),
        AtomicU8::new(0xff),
        AtomicU8::new(0xff),
        AtomicU8::new(0xff),
    ];
    /// (min, max) of the axes, set by the game.
    static RANGES: [(AtomicI32, AtomicI32); AXIS_COUNT] = [const { (AtomicI32::new(0), AtomicI32::new(65535)) }; AXIS_COUNT];
    /// `WAL_DINPUT_WHEEL_<axis>_REST=<0..1>`: where an axis rests in its raw range (default the
    /// middle): the stick's half on each side of it (a spring plunger rests near its end).
    static REST: [AtomicI32; AXIS_COUNT] = [const { AtomicI32::new(-1) }; AXIS_COUNT];
    /// DIPROP_CALIBRATIONMODE: raw values (the device's 0-65535), else cooked (calibrated, in
    /// the game's range).
    static RAW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    /// DIPROP_CALIBRATION of each axis (raw min, center, max), kept in the registry as Windows
    /// keeps a joystick's: the program calibrating it (UltraPin's frontend) is not the one
    /// reading it (its tables' player).
    static CALIBRATION: std::sync::Mutex<[Option<[i32; 3]>; AXIS_COUNT]> = std::sync::Mutex::new([None; AXIS_COUNT]);
    /// The axes listed: X, Y, Z, plus the mapped rotation axes (Rx, Ry, Rz).
    static LISTED_AXES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(3);
    /// `WAL_DINPUT_WHEEL_BUTTONS`: the virtual stick button of each DirectInput button (0: none).
    static BUTTONS: OnceLock<Vec<u32>> = OnceLock::new();

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
        let a = d1.to_le_bytes();
        let b = d2.to_le_bytes();
        let c = d3.to_le_bytes();
        [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
    }
    pub(super) const INSTANCE: [u8; 16] = guid(0x5741_4C57, 0x4845, 0x454C, *b"WALWHEEL");
    /// The product GUID's first field: product id << 16 | vendor id (`WAL_DINPUT_WHEEL_VIDPID`).
    static VIDPID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x5741_4C57);
    /// GUID_XAxis, GUID_YAxis, GUID_ZAxis, GUID_RxAxis, GUID_RyAxis, GUID_RzAxis
    const AXES: [[u8; 16]; AXIS_COUNT] = [
        guid(0xA36D_02E0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02E1, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02E2, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02F4, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02F5, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
        guid(0xA36D_02E3, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]),
    ];
    const AXIS_NAMES: [&str; AXIS_COUNT] = ["X Axis", "Y Axis", "Z Axis", "X Rotation", "Y Rotation", "Z Rotation"];
    /// DI8DEVTYPE_DRIVING, DI8DEVTYPEDRIVING_DUALPEDALS, DIDEVTYPE_HID
    const DEV_TYPE: u32 = 0x16 | 3 << 8 | 0x1_0000;
    /// `WAL_DINPUT_WHEEL_JOYSTICK=1`: DI8DEVTYPE_JOYSTICK, DI8DEVTYPEJOYSTICK_STANDARD, HID (a
    /// game taking only joysticks and gamepads: Ballistics' cabinet GameFrame).
    fn dev_type() -> u32 {
        static JOYSTICK: OnceLock<bool> = OnceLock::new();
        if *JOYSTICK.get_or_init(|| std::env::var("WAL_DINPUT_WHEEL_JOYSTICK").is_ok_and(|v| v.trim() == "1")) {
            0x14 | 2 << 8 | 0x1_0000
        } else {
            DEV_TYPE
        }
    }
    /// DIDFT_ABSAXIS
    const ABS_AXIS: u32 = 2;
    const DIPROP_RANGE: usize = 4;
    const DIPROP_CALIBRATIONMODE: usize = 10;
    const DIPROP_CALIBRATION: usize = 11;
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
        let keys = ["WAL_DINPUT_WHEEL_Y_AXIS", "WAL_DINPUT_WHEEL_Z_AXIS", "WAL_DINPUT_WHEEL_RX_AXIS", "WAL_DINPUT_WHEEL_RY_AXIS", "WAL_DINPUT_WHEEL_RZ_AXIS"];
        for (i, key) in keys.into_iter().enumerate() {
            if let Ok(value) = std::env::var(key) {
                let value = value.trim();
                let (value, invert) = value.strip_prefix('-').map_or((value, false), |axis| (axis, true));
                match Axis::from_name(value) {
                    Some(axis) => {
                        INPUT_AXES[i + 1].store(axis as u8 | if invert { 0x80 } else { 0 }, Ordering::Relaxed);
                        LISTED_AXES.fetch_max(i + 2, Ordering::Relaxed);
                    }
                    None => log!("dinput: unknown wheel axis {key}={value:?}"),
                }
            }
        }
        if let Ok(spec) = std::env::var("WAL_DINPUT_WHEEL_BUTTONS") {
            let buttons = spec
                .split(',')
                .map(|b| {
                    let b = b.trim();
                    if b.eq_ignore_ascii_case("none") { 0 } else {
                        // `b1+start`: pressed with either
                        b.split('+').fold(0, |mask, b| {
                            mask | wal_protocol::button::from_name(b.trim()).unwrap_or_else(|| {
                                log!("dinput: unknown wheel button {b:?}");
                                0
                            })
                        })
                    }
                })
                .collect();
            let _ = BUTTONS.set(buttons);
        }
        if let Ok(ids) = std::env::var("WAL_DINPUT_WHEEL_VIDPID") {
            let ids = ids.split_once(':').and_then(|(v, p)| Some((u16::from_str_radix(v.trim(), 16).ok()?, u16::from_str_radix(p.trim(), 16).ok()?)));
            match ids {
                Some((vid, pid)) => VIDPID.store((pid as u32) << 16 | vid as u32, Ordering::Relaxed),
                None => log!("dinput: bad WAL_DINPUT_WHEEL_VIDPID"),
            }
        }
        log!("dinput: wheel {name:?}");
        let _ = NAME.set(name);
        for (i, axis) in ["X", "Y", "Z", "RX", "RY", "RZ"].into_iter().enumerate() {
            if let Some(rest) = std::env::var(format!("WAL_DINPUT_WHEEL_{axis}_REST")).ok().and_then(|v| v.trim().parse::<f32>().ok()) {
                REST[i].store((rest.clamp(0.0, 1.0) * 65535.0) as i32, Ordering::Relaxed);
            }
        }
        let mut calibration = CALIBRATION.lock().unwrap_or_else(|e| e.into_inner());
        for (i, slot) in calibration.iter_mut().enumerate() {
            *slot = calibration_key::load(i);
        }
        if calibration.iter().any(Option::is_some) {
            log!("dinput: wheel calibration {:?}", *calibration);
        }
    }

    fn name() -> &'static str {
        NAME.get().map_or("", |n| n.as_str())
    }

    /// `WAL_DINPUT_WHEEL_BUTTON_BASE=0`: the buttons named from `Button 0` (Windows' names of a
    /// HID device's buttons; Ballistics binds its cabinet's buttons by name), else `Button 1`.
    fn button_base() -> usize {
        static BASE: OnceLock<usize> = OnceLock::new();
        *BASE.get_or_init(|| std::env::var("WAL_DINPUT_WHEEL_BUTTON_BASE").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1))
    }

    /// `WAL_DINPUT_WHEEL_SYSTEM=1`: the system keyboard and mouse of [`super::set_keyboard`]
    /// listed and created next to the wheel (Ballistics stops without a keyboard device).
    fn system_devices() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("WAL_DINPUT_WHEEL_SYSTEM").is_ok_and(|v| v.trim() == "1"))
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
            let product = guid(VIDPID.load(Ordering::Relaxed), 0, 0, *b"\0\0PIDVID");
            std::ptr::copy_nonoverlapping(product.as_ptr(), out.add(20), 16);
            (out.add(36) as *mut u32).write_unaligned(dev_type());
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
        matches!(filter & 0xFF, 0 | 4) || filter & 0xFF == dev_type() & 0xFF
    }

    unsafe fn enum_devices(filter: u32, cb: EnumCallback, context: P, wide: bool) -> i32 {
        if system_devices() && !wide {
            unsafe { super::enum_devices(std::ptr::null_mut(), filter, cb as P, context, 0) };
        }
        if listed(filter) {
            let mut instance = [0u8; 0x44C];
            let r = unsafe {
                device_instance(instance.as_mut_ptr(), wide);
                super::enum_callback(cb, instance.as_ptr(), context)
            };
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
                log!("dinput: EnumDevices(type {filter:#x}): the wheel, to the game's callback +{:#x} -> {r}", (cb as usize).wrapping_sub(exe));
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
        if !is_wheel && system_devices() && !wide {
            return unsafe { super::create_device(std::ptr::null_mut(), guid.cast(), out, std::ptr::null_mut()) };
        }
        log!("dinput: CreateDevice({})", if is_wheel { "the wheel" } else { "another device" });
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

    /// The wheel's force feedback (`WAL_DINPUT_WHEEL_FFB=0`: none, for games whose cabinet
    /// motor is driven elsewhere, Showdown's parallel port board).
    fn ffb() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| !std::env::var("WAL_DINPUT_WHEEL_FFB").is_ok_and(|v| v.trim() == "0"))
    }

    /// `GetCapabilities(lpDIDevCaps)`: DIDEVCAPS, attached, FFB capable, 3 axes.
    unsafe extern "system" fn get_capabilities(_this: P, caps: *mut u32) -> i32 {
        if !caps.is_null() {
            unsafe {
                let size = (*caps).clamp(4, 44) as usize;
                std::ptr::write_bytes(caps.add(1) as *mut u8, 0, size - 4);
                // dwFlags, dwDevType, dwAxes, dwButtons, dwPOVs, then the FFB timing fields.
                let axes = LISTED_AXES.load(Ordering::Relaxed) as u32;
                let flags = if ffb() { CAPS_FLAGS } else { CAPS_FLAGS & !0x100 };
                for (i, v) in [flags, dev_type(), axes, 32, 0, 10_000, 1, 1, 1, 0x0001_0000].into_iter().enumerate() {
                    if 4 * (i + 2) <= size {
                        *caps.add(i + 1) = v;
                    }
                }
            }
        }
        DI_OK
    }

    /// `EnumObjects(lpCallback, pvRef, dwFlags)`: the absolute axes (when asked), the buttons.
    unsafe fn enum_objects(cb: EnumCallback, context: P, flags: u32, wide: bool) -> i32 {
        // DIDFT_ALL, or a type mask with DIDFT_AXIS / DIDFT_ABSAXIS
        if flags != 0 && flags & 7 == 0 {
            return DI_OK;
        }
        let chars = if wide { 2 } else { 1 };
        let size = 32 + 260 * chars + 24;
        let mut object = [0u8; 0x240];
        for i in 0..LISTED_AXES.load(Ordering::Relaxed) {
            if flags != 0 && flags & 3 == 0 { break; }
            object.fill(0);
            let o = object.as_mut_ptr();
            unsafe {
                (o as *mut u32).write_unaligned(size as u32);
                std::ptr::copy_nonoverlapping(AXES[i].as_ptr(), o.add(4), 16);
                (o.add(20) as *mut u32).write_unaligned(4 * i as u32); // dwOfs in DIJOYSTATE
                (o.add(24) as *mut u32).write_unaligned(ABS_AXIS | (i as u32) << 8); // dwType
                put_str(o.add(32), 260, AXIS_NAMES[i], wide);
                if super::enum_callback(cb, o, context) == 0 {
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
                    let label = format!("Button {}", i + button_base());
                    put_str(o.add(32), 260, &label, wide);
                    if super::enum_callback(cb, o, context) == 0 { break; }
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
        (axis < LISTED_AXES.load(Ordering::Relaxed)).then_some(axis)
    }

    /// `GetProperty(rguidProp, pdiph)`: the axis ranges, the names.
    unsafe extern "system" fn get_property(_this: P, prop: usize, header: *mut u32) -> i32 {
        {
            static LOGGED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
            let mut logged = LOGGED.lock().unwrap();
            if !logged.contains(&prop) {
                logged.push(prop);
                log!("dinput: wheel GetProperty({prop})");
            }
        }
        if prop == DIPROP_CALIBRATIONMODE && !header.is_null() {
            unsafe { *header.add(4) = RAW.load(Ordering::Relaxed) as u32 };
            return DI_OK;
        }
        if prop == DIPROP_CALIBRATION && !header.is_null() {
            let Some(axis) = (unsafe { header_axis(header) }) else { return DIERR_NOTFOUND };
            let values = CALIBRATION.lock().unwrap_or_else(|e| e.into_inner())[axis].unwrap_or([0, 32768, 65535]);
            for (i, v) in values.into_iter().enumerate() {
                unsafe { *header.add(4 + i) = v as u32 };
            }
            return DI_OK;
        }
        // DIPROP_INSTANCENAME / DIPROP_PRODUCTNAME (DIPROPSTRING: the header, WCHAR[260]): the
        // wheel's name (Visual Pinball turns its accelerometer and reads its plunger only for the
        // cabinet controller's)
        if matches!(prop, 13 | 14) && !header.is_null() {
            let text = header.wrapping_add(4) as *mut u16;
            for (i, c) in name().encode_utf16().take(259).chain(std::iter::once(0)).enumerate() {
                unsafe { text.add(i).write_unaligned(c) };
            }
            return DI_OK;
        }
        // DIPROP_PHYSICALRANGE / DIPROP_LOGICALRANGE: the range too
        if !matches!(prop, DIPROP_RANGE | 18 | 19) || header.is_null() {
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
        if prop != DIPROP_RANGE && !header.is_null() {
            log!("dinput: wheel SetProperty({prop}, value {:#x})", unsafe { *header.add(4) });
        }
        // DIPROP_CALIBRATIONMODE (DIPROPDWORD): 1 raw, 0 cooked
        if prop == DIPROP_CALIBRATIONMODE && !header.is_null() {
            RAW.store(unsafe { *header.add(4) } == 1, Ordering::Relaxed);
            return DI_OK;
        }
        // DIPROP_CALIBRATION (DIPROPCAL): lMin, lCenter, lMax of an axis
        if prop == DIPROP_CALIBRATION && !header.is_null() {
            let Some(axis) = (unsafe { header_axis(header as *mut u32) }) else { return DIERR_NOTFOUND };
            let values = unsafe { [*header.add(4) as i32, *header.add(5) as i32, *header.add(6) as i32] };
            log!("dinput: wheel axis {axis} calibration {values:?}");
            CALIBRATION.lock().unwrap_or_else(|e| e.into_inner())[axis] = Some(values);
            calibration_key::save(axis, values);
            return DI_OK;
        }
        if prop != DIPROP_RANGE || header.is_null() {
            return DI_OK;
        }
        let (min, max) = unsafe { (*header.add(4) as i32, *header.add(5) as i32) };
        let axes = match unsafe { *header.add(3) } {
            0 => 0..AXIS_COUNT,
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

    /// A stick value moved to the axis' rest (`REST`): each half of the stick spans the raw range
    /// on its side of it.
    fn rested(axis: usize, v: u16) -> u16 {
        let rest = REST[axis].load(Ordering::Relaxed);
        if rest < 0 {
            return v;
        }
        let v = v as i32;
        (if v >= 32768 { rest + (v - 32768) * (65535 - rest) / 32767 } else { v * rest / 32768 }) as u16
    }

    /// A raw value in the game's terms: itself in raw mode, else mapped to the axis' range
    /// through its calibration (its center to the range's middle), else linearly.
    fn scaled(axis: usize, v: u16) -> i32 {
        if RAW.load(Ordering::Relaxed) {
            return v as i32;
        }
        let (min, max) = (RANGES[axis].0.load(Ordering::Relaxed) as i64, RANGES[axis].1.load(Ordering::Relaxed) as i64);
        let calibration = CALIBRATION.lock().unwrap_or_else(|e| e.into_inner())[axis];
        if let Some([lo, center, hi]) = calibration.map(|c| c.map(i64::from)).filter(|[lo, c, hi]| lo < c && c < hi) {
            let (v, mid) = (v as i64, (min + max) / 2);
            let cooked = if v <= center {
                mid - (mid - min) * (center - v.max(lo)) / (center - lo)
            } else {
                mid + (max - mid) * (v.min(hi) - center) / (hi - center)
            };
            return cooked as i32;
        }
        (min + (max - min) * v as i64 / 65535) as i32
    }

    /// The wheel's calibration in the registry (`HKCU\\Software\\WindowsArcadeLoader\\DirectInput\\
    /// <wheel name>`, a binary `CalibrationN` per axis: min, center, max).
    mod calibration_key {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE, RegCloseKey,
            RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        };

        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        fn path() -> Vec<u16> {
            wide(&format!("Software\\WindowsArcadeLoader\\DirectInput\\{}", super::name()))
        }

        pub(super) fn load(axis: usize) -> Option<[i32; 3]> {
            let mut key: HKEY = std::ptr::null_mut();
            if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path().as_ptr(), 0, KEY_QUERY_VALUE, &mut key) } != 0 {
                return None;
            }
            let mut data = [0i32; 3];
            let mut size = 12u32;
            let name = wide(&format!("Calibration{axis}"));
            let found = unsafe { RegQueryValueExW(key, name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), data.as_mut_ptr().cast(), &mut size) } == 0
                && size == 12;
            unsafe { RegCloseKey(key) };
            found.then_some(data)
        }

        pub(super) fn save(axis: usize, values: [i32; 3]) {
            let mut key: HKEY = std::ptr::null_mut();
            let created = unsafe {
                RegCreateKeyExW(HKEY_CURRENT_USER, path().as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
            };
            if created != 0 {
                return;
            }
            let name = wide(&format!("Calibration{axis}"));
            unsafe {
                RegSetValueExW(key, name.as_ptr(), 0, REG_BINARY, values.as_ptr().cast(), 12);
                RegCloseKey(key);
            }
        }
    }

    /// An object of the game's data format: what goes at which offset.
    #[derive(Clone, Copy, Debug)]
    enum Field {
        Axis(usize),
        Button(usize),
        Pov,
    }
    /// The game's `SetDataFormat` (none: DIJOYSTATE's layout).
    static FORMAT: std::sync::Mutex<Option<Vec<(Field, usize)>>> = std::sync::Mutex::new(None);

    /// `SetDataFormat(lpdf)`: the game's own layout (GRID lists the objects it enumerated, in
    /// its own order): each object gets the axis (by GUID, else by instance or the next one),
    /// button or POV it names, at its offset.
    unsafe extern "system" fn set_data_format(_this: P, format: *const u32) -> i32 {
        if format.is_null() {
            return DI_OK;
        }
        let (obj_size, count, objects) = unsafe { (*format.add(1) as usize, *format.add(4) as usize, *format.add(5) as *const u8) };
        let mut fields = Vec::new();
        let (mut next_axis, mut next_button) = (0usize, 0usize);
        let listed = LISTED_AXES.load(Ordering::Relaxed);
        let mut used_axes = [false; AXIS_COUNT];
        for i in 0..count {
            let o = unsafe { objects.add(i * obj_size.max(16)) };
            let (guid, offset, ty) = unsafe {
                ((o as *const usize).read_unaligned() as *const u8, (o.add(4) as *const u32).read_unaligned() as usize, (o.add(8) as *const u32).read_unaligned())
            };
            let instance = (ty >> 8) & 0xFFFF;
            let field = if ty & 3 != 0 {
                // an axis: by GUID, else its instance, else the next one
                let by_guid = (!guid.is_null()).then(|| {
                    let g = unsafe { std::slice::from_raw_parts(guid, 16) };
                    AXES.iter().position(|a| a.as_slice() == g)
                });
                let axis = match by_guid {
                    Some(found) => found,
                    None if instance != 0xFFFF => Some(instance as usize),
                    None => {
                        while next_axis < listed && used_axes[next_axis] {
                            next_axis += 1;
                        }
                        Some(next_axis)
                    }
                };
                match axis {
                    Some(a) if a < listed && !used_axes[a] => {
                        used_axes[a] = true;
                        Some(Field::Axis(a))
                    }
                    _ => None, // a slider, or an axis the wheel does not have (optional)
                }
            } else if ty & 0x0C != 0 {
                let b = if instance != 0xFFFF { instance as usize } else { next_button };
                next_button = b + 1;
                (b < 32).then_some(Field::Button(b))
            } else if ty & 0x10 != 0 {
                Some(Field::Pov)
            } else {
                None
            };
            if let Some(f) = field {
                fields.push((f, offset));
            }
        }
        log!("dinput: wheel data format: {fields:?}");
        *FORMAT.lock().unwrap() = Some(fields);
        DI_OK
    }

    /// `GetDeviceState(cbData, lpvData)`: DIJOYSTATE(2), X the wheel, Y and Z released
    /// (their minimum).
    unsafe extern "system" fn get_device_state(_this: P, size: u32, data: *mut u8) -> i32 {
        if data.is_null() {
            return DI_OK;
        }
        let stick = crate::input(0);
        let axes = std::array::from_fn::<_, AXIS_COUNT, _>(|i| {
            let mapped = INPUT_AXES[i].load(Ordering::Relaxed);
            let value = if mapped & 0x7f == 0x7f { 0 } else {
                let value = stick.axis_u16(Axis::ALL[(mapped & 0x7f) as usize]);
                rested(i, if mapped & 0x80 != 0 { !value } else { value })
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
                log!("dinput: wheel state X={} Y={} Z={} Rx={} (steer/brake/accel from stick lx={} accel={} brake={})",
                    axes[0], axes[1], axes[2], axes[3],
                    stick.axis(Axis::LeftX), stick.axis(Axis::Accel), stick.axis(Axis::Brake));
            }
        }
        let pressed = |b: usize| -> bool {
            match BUTTONS.get() {
                Some(buttons) => buttons.get(b).is_some_and(|&m| m != 0 && stick.pressed(m)),
                None => match b {
                    0..=7 => stick.buttons >> (b + 4) & 1 != 0,
                    8..=11 => stick.buttons >> (b + 4) & 1 != 0,
                    _ => false,
                },
            }
        };
        if let Some(fields) = FORMAT.lock().unwrap().as_ref() {
            unsafe {
                std::ptr::write_bytes(data, 0, size as usize);
                for &(field, offset) in fields {
                    match field {
                        Field::Axis(a) if offset + 4 <= size as usize => (data.add(offset) as *mut i32).write_unaligned(axes[a]),
                        Field::Button(b) if offset < size as usize => *data.add(offset) = if pressed(b) { 0x80 } else { 0 },
                        Field::Pov if offset + 4 <= size as usize => (data.add(offset) as *mut u32).write_unaligned(u32::MAX),
                        _ => {}
                    }
                }
            }
            return DI_OK;
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
            if let Some(buttons) = BUTTONS.get() {
                for (i, &mask) in buttons.iter().enumerate() {
                    let value = if mask != 0 && stick.pressed(mask) { 0x80 } else { 0 };
                    if 0x30 + i < size as usize && i < 32 { *data.add(0x30 + i) = value; }
                }
            } else {
                for i in 0..8 {
                    let value = ((stick.buttons >> (i + 4)) & 1) as u8 * 0x80;
                    if 0x30 + i < size as usize { *data.add(0x30 + i) = value; }
                }
                for i in 0..4 {
                    let value = ((stick.buttons >> (i + 12)) & 1) as u8 * 0x80;
                    if 0x38 + i < size as usize { *data.add(0x38 + i) = value; }
                }
            }
        }
        DI_OK
    }

    /// The values `GetDeviceData` last reported (the state laid out as `GetDeviceState`
    /// writes it), and when every axis was last reported.
    static REPORTED: std::sync::Mutex<(Vec<u8>, u32)> = std::sync::Mutex::new((Vec::new(), 0));
    static SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

    /// `WAL_DINPUT_WHEEL_HEARTBEAT_MS`: every axis also reported that often, changed or not
    /// (a real accelerometer's noise: Visual Pinball takes 3 s without an axis event as its
    /// nudge board unplugged and ends the table).
    fn heartbeat_ms() -> u32 {
        static MS: OnceLock<u32> = OnceLock::new();
        *MS.get_or_init(|| std::env::var("WAL_DINPUT_WHEEL_HEARTBEAT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
    }

    /// `GetDeviceData(cbObjectData, rgdod, pdwInOut, dwFlags)`: the buffered events, one
    /// DIDEVICEOBJECTDATA (offset, value, time stamp, sequence, DirectInput 8's app data) per
    /// object of the data format whose value changed since the previous call (DIGDD_PEEK: kept).
    unsafe extern "system" fn get_device_data(this: P, obj_size: u32, data: *mut u8, count: *mut u32, flags: u32) -> i32 {
        const DIGDD_PEEK: u32 = 1;
        if count.is_null() {
            return DI_OK;
        }
        // the objects (DIJOYSTATE's layout without a format) and the state they read
        let fields: Vec<(Field, usize)> = FORMAT.lock().unwrap().clone().unwrap_or_else(|| {
            let axes = (0..LISTED_AXES.load(Ordering::Relaxed)).map(|a| (Field::Axis(a), 4 * a));
            axes.chain((0..32).map(|b| (Field::Button(b), 0x30 + b))).collect()
        });
        let len = fields.iter().map(|&(_, o)| o + 4).max().unwrap_or(0).max(0x50);
        let mut state = vec![0u8; len];
        unsafe { get_device_state(this, len as u32, state.as_mut_ptr()) };
        let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
        let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
        if reported.0.len() != len {
            // the axes reported at once, the buttons only when they change (an event per
            // released button would be a release never pressed: Visual Pinball toggles its
            // joystick ball control on button 7's)
            *reported = (vec![0xAA; len], now);
            for &(field, offset) in &fields {
                if let Field::Button(_) = field {
                    reported.0[offset] = state[offset];
                }
            }
        }
        let heartbeat = heartbeat_ms() != 0 && now.wrapping_sub(reported.1) >= heartbeat_ms();
        let mut events = Vec::new();
        for &(field, offset) in &fields {
            let (value, last, axis) = match field {
                Field::Axis(_) => (u32::from_le_bytes(state[offset..offset + 4].try_into().unwrap()), u32::from_le_bytes(reported.0[offset..offset + 4].try_into().unwrap()), true),
                Field::Button(_) => (state[offset] as u32, reported.0[offset] as u32, false),
                Field::Pov => continue,
            };
            if value != last || (axis && heartbeat) {
                events.push((offset, value, axis));
            }
        }
        let capacity = unsafe { *count } as usize;
        let n = events.len().min(capacity);
        let size = (obj_size as usize).max(16);
        for (i, &(offset, value, _)) in events.iter().take(n).enumerate() {
            if !data.is_null() {
                let sequence = if flags & DIGDD_PEEK == 0 { SEQUENCE.fetch_add(1, Ordering::Relaxed) } else { SEQUENCE.load(Ordering::Relaxed) };
                unsafe {
                    let e = data.add(i * size);
                    std::ptr::write_bytes(e, 0, size);
                    for (k, v) in [offset as u32, value, now, sequence].into_iter().enumerate() {
                        (e.add(4 * k) as *mut u32).write_unaligned(v);
                    }
                }
            }
            if flags & DIGDD_PEEK == 0 {
                let width = if events[i].2 { 4 } else { 1 };
                reported.0[offset..offset + width].copy_from_slice(&state[offset..offset + width]);
            }
        }
        if heartbeat && flags & DIGDD_PEEK == 0 && n == events.len() {
            reported.1 = now;
        }
        unsafe { *count = n as u32 };
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
        effect_set_parameters as *const (),
        effect_start as *const (),
        effect_stop as *const (),
        effect_status as *const (),
        ok0 as *const (), // Download
        ok0 as *const (), // Unload
        ok1 as *const (), // Escape
    ]);

    /// `WAL_DINPUT_WHEEL_FFB_OUT=1`: the game's constant force sent to the launcher's wheel
    /// (`output::FFB_CONSTANT`, positive to the right): its magnitude (`DICONSTANTFORCE`) times
    /// its gain and its direction's sign on the one axis, 0 when stopped.
    fn ffb_out() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("WAL_DINPUT_WHEEL_FFB_OUT").is_ok_and(|v| v.trim() == "1"))
    }
    /// The constant force's magnitude, gain (0..10000) and direction sign; it plays.
    static FORCE: AtomicI32 = AtomicI32::new(0);
    static GAIN: AtomicI32 = AtomicI32::new(10000);
    static SIGN: AtomicI32 = AtomicI32::new(1);
    static PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static SENT: AtomicI32 = AtomicI32::new(i32::MIN);

    /// `DIEFFECT`
    #[repr(C)]
    struct DiEffect {
        size: u32,
        flags: u32,
        duration: u32,
        sample_period: u32,
        gain: u32,
        trigger_button: u32,
        trigger_repeat_interval: u32,
        axes: u32,
        axis_ids: *const u32,
        direction: *const i32,
        envelope: P,
        type_specific_size: u32,
        type_specific: *const i32,
    }
    const DIEP_GAIN: u32 = 0x04;
    const DIEP_DIRECTION: u32 = 0x40;
    const DIEP_TYPESPECIFICPARAMS: u32 = 0x100;
    const DIEP_START: u32 = 0x2000_0000;

    fn send_force() {
        let value = if PLAYING.load(Ordering::Relaxed) {
            let v = FORCE.load(Ordering::Relaxed) as i64 * GAIN.load(Ordering::Relaxed) as i64 / 10000 * SIGN.load(Ordering::Relaxed) as i64;
            v.clamp(-(wal_protocol::output::FFB_MAX as i64), wal_protocol::output::FFB_MAX as i64) as i32
        } else {
            0
        };
        if SENT.swap(value, Ordering::Relaxed) != value {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if value != 0 && !LOGGED.swap(true, Ordering::Relaxed) {
                log!("dinput: the game's constant force to the launcher (first: {value})");
            }
            crate::send_output(0, wal_protocol::output::FFB_CONSTANT, value);
        }
    }

    /// The parameters of a constant force effect (`flags`: those set).
    unsafe fn take_parameters(effect: *const DiEffect, flags: u32) {
        if effect.is_null() || !ffb_out() || EFFECT_GUID.lock().unwrap()[..4] != 0x1354_1C20u32.to_le_bytes() {
            return;
        }
        let e = unsafe { &*effect };
        if flags & DIEP_GAIN != 0 {
            GAIN.store(e.gain.min(10000) as i32, Ordering::Relaxed);
        }
        if flags & DIEP_DIRECTION != 0 && !e.direction.is_null() && e.axes >= 1 {
            let d = unsafe { e.direction.read_unaligned() };
            SIGN.store(if d < 0 { -1 } else { 1 }, Ordering::Relaxed);
        }
        if flags & DIEP_TYPESPECIFICPARAMS != 0 && !e.type_specific.is_null() && e.type_specific_size >= 4 {
            FORCE.store(unsafe { e.type_specific.read_unaligned() }, Ordering::Relaxed);
        }
        if flags & DIEP_START != 0 {
            PLAYING.store(true, Ordering::Relaxed);
        }
        send_force();
    }

    /// `SetParameters(lpeff, dwFlags)`
    unsafe extern "system" fn effect_set_parameters(_this: P, effect: *const DiEffect, flags: u32) -> i32 {
        unsafe { take_parameters(effect, flags) };
        DI_OK
    }

    /// `Start(dwIterations, dwFlags)`
    unsafe extern "system" fn effect_start(_this: P, _iterations: u32, _flags: u32) -> i32 {
        PLAYING.store(true, Ordering::Relaxed);
        if ffb_out() {
            send_force();
        }
        DI_OK
    }

    /// `Stop()`
    unsafe extern "system" fn effect_stop(_this: P) -> i32 {
        PLAYING.store(false, Ordering::Relaxed);
        if ffb_out() {
            send_force();
        }
        DI_OK
    }

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

    /// `CreateEffect(rguidSrc, lpdiEffect, lplpDirectInputEffect, pUnkOuter)` (stdcall: the
    /// 5 arguments popped, Blazing Angels' caller depends on it)
    unsafe extern "system" fn create_effect(_this: P, guid: *const u8, _effect: P, out: *mut P, _outer: P) -> i32 {
        if out.is_null() {
            return super::DIERR_DEVICENOTREG;
        }
        if !ffb() {
            return super::DIERR_UNSUPPORTED;
        }
        if !guid.is_null() {
            unsafe { EFFECT_GUID.lock().unwrap().copy_from_slice(std::slice::from_raw_parts(guid, 16)); }
        }
        if EFFECT_CREATED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log!("dinput: another fake FFB effect created");
        } else {
            log!("dinput: fake FFB effect created");
        }
        // the creation's parameters (all of them: those the structure holds)
        unsafe { take_parameters(_effect as *const DiEffect, DIEP_GAIN | DIEP_DIRECTION | DIEP_TYPESPECIFICPARAMS) };
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
        v[10] = get_device_data as *const ();
        v[11] = set_data_format as *const ();
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

    /// `WAL_DINPUT_WHEEL_DELAY_MS=<n>`: the wheel listed only `n` milliseconds after the game's
    /// first enumeration (a device plugged in later, for games polling for devices).
    fn plugged() -> bool {
        static FIRST: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let first = *FIRST.get_or_init(std::time::Instant::now);
        let delay = std::env::var("WAL_DINPUT_WHEEL_DELAY_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        first.elapsed() >= std::time::Duration::from_millis(delay)
    }

    /// Whether an `EnumDevices` type filter takes the board: all devices (and the HID
    /// super-type), the game controller class (DI8DEVCLASS_GAMECTRL), HID gamepad and
    /// joystick types, all input devices.
    fn listed(filter: u32) -> bool {
        matches!(filter & 0xFF, 0 | 0x1 | 0x4 | 0x5 | 0x80)
    }

    unsafe fn enum_devices(filter: u32, cb: EnumCallback, context: P, wide: bool) -> i32 {
        log!("dinput: EnumDevices(type {filter:#x}){}", if listed(filter) { ": the wheel" } else { "" });
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

/// The fake joysticks of `WAL_DINPUT_PADS`.
mod pads {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use wal_protocol::{Axis, button};

    use super::{DEVICE_VTABLE, DI_OK, DIERR_DEVICENOTREG, Object, P, VTable};
    use crate::log;

    const MAX: usize = wal_protocol::MAX_PLAYERS;
    /// How many joysticks are listed (`WAL_DINPUT_PADS`).
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    /// `WAL_DINPUT_PADS_BUTTONS`: the virtual stick buttons of each DirectInput button.
    static BUTTONS: OnceLock<Vec<u32>> = OnceLock::new();
    const DEFAULT_BUTTONS: [u32; 10] = [
        button::B1,
        button::B2,
        button::B3,
        button::B4,
        button::B5,
        button::B6,
        button::B7,
        button::B8,
        button::START,
        button::COIN,
    ];

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
        let a = d1.to_le_bytes();
        let b = d2.to_le_bytes();
        let c = d3.to_le_bytes();
        [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]]
    }
    /// Instance GUID of joystick `i`: its last byte is the player.
    const fn instance(i: usize) -> [u8; 16] {
        guid(0x5741_4C57, 0x4150, 0x5344, [b'W', b'A', b'L', b'P', b'A', b'D', b'0', b'0' + i as u8])
    }
    const X_AXIS: [u8; 16] = guid(0xA36D_02E0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
    const Y_AXIS: [u8; 16] = guid(0xA36D_02E1, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
    const BUTTON_GUID: [u8; 16] = guid(0xA36D_02F0, 0xC9F3, 0x11CF, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
    /// DI8DEVTYPE_JOYSTICK, DI8DEVTYPEJOYSTICK_STANDARD, HID.
    const DEV_TYPE: u32 = 0x14 | 2 << 8 | 0x1_0000;
    const ABS_AXIS: u32 = 2;
    const DIPROP_RANGE: usize = 4;
    const DIPH_BYOFFSET: u32 = 1;
    const DIPH_BYID: u32 = 2;
    const DIERR_NOTFOUND: i32 = 0x8007_0002u32 as i32;

    pub(super) fn init(count: usize) {
        COUNT.store(count.clamp(1, MAX), Ordering::Relaxed);
        if let Ok(spec) = std::env::var("WAL_DINPUT_PADS_BUTTONS") {
            let buttons = spec
                .split(',')
                .map(|b| {
                    let b = b.trim();
                    if b.eq_ignore_ascii_case("none") {
                        0
                    } else {
                        // `b1+start`: pressed with either
                        b.split('+').fold(0, |mask, b| {
                            mask | button::from_name(b.trim()).unwrap_or_else(|| {
                                log!("dinput: unknown pad button {b:?}");
                                0
                            })
                        })
                    }
                })
                .collect();
            let _ = BUTTONS.set(buttons);
        }
        log!("dinput: {} joysticks", COUNT.load(Ordering::Relaxed));
    }

    fn buttons() -> &'static [u32] {
        BUTTONS.get().map_or(&DEFAULT_BUTTONS, |b| b.as_slice())
    }

    fn name(i: usize) -> String {
        format!("WAL Joystick {}", i + 1)
    }

    /// The player of a device interface (its address in the A or W array).
    fn player(this: P) -> usize {
        let base = |objects: &'static [Object; MAX]| objects.as_ptr() as usize;
        let size = std::mem::size_of::<Object>();
        let a = this as usize;
        for objects in [&PADS_A, &PADS_W] {
            let b = base(objects);
            if a >= b && a < b + MAX * size {
                return (a - b) / size;
            }
        }
        0
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

    /// Fills a DIDEVICEINSTANCEA / W (`wide`) of joystick `i`, its dwSize included.
    unsafe fn device_instance(out: *mut u8, i: usize, wide: bool) {
        let chars = if wide { 2 } else { 1 };
        let size = 4 + 36 + 2 * 260 * chars + 20;
        let id = instance(i);
        let product = guid(0x5741_4C57, 0, 0, *b"\0\0PIDVID");
        unsafe {
            std::ptr::write_bytes(out, 0, size);
            (out as *mut u32).write_unaligned(size as u32);
            std::ptr::copy_nonoverlapping(id.as_ptr(), out.add(4), 16);
            std::ptr::copy_nonoverlapping(product.as_ptr(), out.add(20), 16);
            (out.add(36) as *mut u32).write_unaligned(DEV_TYPE);
            put_str(out.add(40), 260, &name(i), wide);
            put_str(out.add(40 + 260 * chars), 260, &name(i), wide);
            // HID usage page 1 (generic desktop), usage 4 (joystick)
            let usage = out.add(40 + 2 * 260 * chars + 16);
            (usage as *mut u16).write_unaligned(1);
            (usage.add(2) as *mut u16).write_unaligned(4);
        }
    }

    type EnumCallback = unsafe extern "system" fn(*const u8, P) -> i32;

    /// Whether an `EnumDevices` type filter takes the joysticks: all devices, game controllers
    /// (DI8DEVCLASS_GAMECTRL), joysticks (DIDEVTYPE_JOYSTICK, DI8DEVTYPE_JOYSTICK).
    fn listed(filter: u32) -> bool {
        matches!(filter & 0xFF, 0 | 4 | 0x14)
    }

    unsafe fn enum_devices(filter: u32, cb: EnumCallback, context: P, wide: bool) -> i32 {
        if !listed(filter) {
            return DI_OK;
        }
        let count = COUNT.load(Ordering::Relaxed);
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log!("dinput: EnumDevices(type {filter:#x}): {count} joysticks");
        }
        let mut item = [0u8; 0x44C];
        for i in 0..count {
            unsafe {
                device_instance(item.as_mut_ptr(), i, wide);
                if super::enum_callback(cb, item.as_ptr(), context) == 0 {
                    break; // DIENUM_STOP
                }
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

    /// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`: a joystick by its instance GUID.
    unsafe fn create_device(guid: *const u8, out: *mut P, wide: bool) -> i32 {
        if out.is_null() {
            return DIERR_DEVICENOTREG;
        }
        let g = if guid.is_null() { None } else { Some(unsafe { std::slice::from_raw_parts(guid, 16) }) };
        let pad = (0..COUNT.load(Ordering::Relaxed)).find(|&i| g == Some(instance(i).as_slice()));
        log!("dinput: CreateDevice({})", pad.map_or("another device".to_string(), |i| name(i)));
        let device: &Object = match (pad, wide) {
            (Some(i), false) => &PADS_A[i],
            (Some(i), true) => &PADS_W[i],
            (None, _) => &super::DEVICE,
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

    /// The fake IDirectInput8A / W with the joysticks.
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

    /// `GetCapabilities(lpDIDevCaps)`: DIDEVCAPS, attached, 2 axes, the buttons.
    unsafe extern "system" fn get_capabilities(_this: P, caps: *mut u32) -> i32 {
        if !caps.is_null() {
            unsafe {
                let size = (*caps).clamp(4, 44) as usize;
                std::ptr::write_bytes(caps.add(1) as *mut u8, 0, size - 4);
                // dwFlags (DIDC_ATTACHED), dwDevType, dwAxes, dwButtons, dwPOVs
                for (i, v) in [1, DEV_TYPE, 2, buttons().len() as u32, 0].into_iter().enumerate() {
                    if 4 * (i + 2) <= size {
                        *caps.add(i + 1) = v;
                    }
                }
            }
        }
        DI_OK
    }

    /// `EnumObjects(lpCallback, pvRef, dwFlags)`: X and Y, the buttons.
    unsafe fn enum_objects(cb: EnumCallback, context: P, flags: u32, wide: bool) -> i32 {
        let chars = if wide { 2 } else { 1 };
        let size = 32 + 260 * chars + 24;
        let mut object = [0u8; 0x240];
        let axes = [(X_AXIS, "X Axis"), (Y_AXIS, "Y Axis")];
        if flags == 0 || flags & 3 != 0 {
            for (i, (g, label)) in axes.iter().enumerate() {
                object.fill(0);
                let o = object.as_mut_ptr();
                unsafe {
                    (o as *mut u32).write_unaligned(size as u32);
                    std::ptr::copy_nonoverlapping(g.as_ptr(), o.add(4), 16);
                    (o.add(20) as *mut u32).write_unaligned(4 * i as u32);
                    (o.add(24) as *mut u32).write_unaligned(ABS_AXIS | (i as u32) << 8);
                    put_str(o.add(32), 260, label, wide);
                    if super::enum_callback(cb, o, context) == 0 {
                        return DI_OK;
                    }
                }
            }
        }
        if flags == 0 || flags & 0x0C != 0 {
            for i in 0..buttons().len() {
                object.fill(0);
                let o = object.as_mut_ptr();
                unsafe {
                    (o as *mut u32).write_unaligned(size as u32);
                    std::ptr::copy_nonoverlapping(BUTTON_GUID.as_ptr(), o.add(4), 16);
                    (o.add(20) as *mut u32).write_unaligned(0x30 + i as u32);
                    (o.add(24) as *mut u32).write_unaligned(4 | (i as u32) << 8);
                    put_str(o.add(32), 260, &format!("Button {}", i + 1), wide);
                    if super::enum_callback(cb, o, context) == 0 {
                        return DI_OK;
                    }
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

    /// Each joystick's X and Y ranges (`DIPROP_RANGE`, default 0..65535).
    static RANGES: [[(AtomicI32, AtomicI32); 2]; MAX] =
        [const { [const { (AtomicI32::new(0), AtomicI32::new(65535)) }; 2] }; MAX];

    /// The axis a DIPROPHEADER names (by its DIJOYSTATE offset or its object ID).
    unsafe fn header_axis(header: *const u32) -> Option<usize> {
        let (obj, how) = unsafe { (*header.add(2), *header.add(3)) };
        let axis = match how {
            DIPH_BYOFFSET => obj / 4,
            DIPH_BYID if obj & ABS_AXIS != 0 => obj >> 8 & 0xFFFF,
            _ => return None,
        } as usize;
        (axis < 2).then_some(axis)
    }

    /// `GetProperty(rguidProp, pdiph)`: the axis ranges.
    unsafe extern "system" fn get_property(this: P, prop: usize, header: *mut u32) -> i32 {
        // DIPROP_PHYSICALRANGE / DIPROP_LOGICALRANGE: the range too
        if !matches!(prop, DIPROP_RANGE | 18 | 19) || header.is_null() {
            return DI_OK;
        }
        let Some(axis) = (unsafe { header_axis(header) }) else { return DIERR_NOTFOUND };
        let range = &RANGES[player(this)][axis];
        unsafe {
            *header.add(4) = range.0.load(Ordering::Relaxed) as u32;
            *header.add(5) = range.1.load(Ordering::Relaxed) as u32;
        }
        DI_OK
    }

    /// `SetProperty(rguidProp, pdiph)`: the axis ranges (DIPH_DEVICE sets both).
    unsafe extern "system" fn set_property(this: P, prop: usize, header: *const u32) -> i32 {
        if prop != DIPROP_RANGE || header.is_null() {
            return DI_OK;
        }
        let (min, max) = unsafe { (*header.add(4) as i32, *header.add(5) as i32) };
        let axes = match unsafe { *header.add(3) } {
            0 => 0..2,
            _ => match unsafe { header_axis(header) } {
                Some(a) => a..a + 1,
                None => return DIERR_NOTFOUND,
            },
        };
        let pad = player(this);
        for a in axes {
            log!("dinput: {} axis {a} range {min}..{max}", name(pad));
            RANGES[pad][a].0.store(min, Ordering::Relaxed);
            RANGES[pad][a].1.store(max, Ordering::Relaxed);
        }
        DI_OK
    }

    /// An object of the game's data format: what goes at which offset.
    #[derive(Clone, Copy, Debug)]
    enum Field {
        Axis(usize),
        Button(usize),
        Pov,
    }
    /// Each joystick's `SetDataFormat` (none: DIJOYSTATE's layout).
    static FORMATS: [Mutex<Option<Vec<(Field, usize)>>>; MAX] = [const { Mutex::new(None) }; MAX];

    /// `SetDataFormat(lpdf)`: each object gets the axis (by GUID, else by instance or the next
    /// one), button or POV it names, at its offset.
    unsafe extern "system" fn set_data_format(this: P, format: *const u32) -> i32 {
        if format.is_null() {
            return DI_OK;
        }
        let (obj_size, count, objects) =
            unsafe { (*format.add(1) as usize, *format.add(4) as usize, *format.add(5) as *const u8) };
        let mut fields = Vec::new();
        let (mut next_axis, mut next_button) = (0usize, 0usize);
        for i in 0..count {
            let o = unsafe { objects.add(i * obj_size.max(16)) };
            let (guid, offset, ty) = unsafe {
                (
                    (o as *const usize).read_unaligned() as *const u8,
                    (o.add(4) as *const u32).read_unaligned() as usize,
                    (o.add(8) as *const u32).read_unaligned(),
                )
            };
            let instance = (ty >> 8) & 0xFFFF;
            let field = if ty & 3 != 0 {
                let axis = if guid.is_null() {
                    Some(if instance != 0xFFFF { instance as usize } else { next_axis })
                } else {
                    let g = unsafe { std::slice::from_raw_parts(guid, 16) };
                    [X_AXIS, Y_AXIS].iter().position(|a| a.as_slice() == g)
                };
                match axis {
                    Some(a) if a < 2 => {
                        next_axis = a + 1;
                        Some(Field::Axis(a))
                    }
                    _ => None, // Z, rotations, sliders: none
                }
            } else if ty & 0x0C != 0 {
                let b = if instance != 0xFFFF { instance as usize } else { next_button };
                next_button = b + 1;
                Some(Field::Button(b))
            } else if ty & 0x10 != 0 {
                Some(Field::Pov)
            } else {
                None
            };
            if let Some(f) = field {
                fields.push((f, offset));
            }
        }
        let pad = player(this);
        log!("dinput: {} data format: {} objects", name(pad), fields.len());
        *FORMATS[pad].lock().unwrap() = Some(fields);
        DI_OK
    }

    /// `GetDeviceState(cbData, lpvData)`: the player's stick (X / Y from the d-pad, else the
    /// left stick) and buttons.
    unsafe extern "system" fn get_device_state(this: P, size: u32, data: *mut u8) -> i32 {
        if data.is_null() {
            return DI_OK;
        }
        let pad = player(this);
        let stick = crate::input(pad);
        let axis = |a: usize| {
            let (minus, plus, analog) = if a == 0 {
                (button::LEFT, button::RIGHT, Axis::LeftX)
            } else {
                (button::UP, button::DOWN, Axis::LeftY)
            };
            let v = if stick.pressed(minus) {
                0
            } else if stick.pressed(plus) {
                65535
            } else {
                stick.axis_u16(analog)
            };
            let (min, max) = (RANGES[pad][a].0.load(Ordering::Relaxed) as i64, RANGES[pad][a].1.load(Ordering::Relaxed) as i64);
            (min + (max - min) * v as i64 / 65535) as i32
        };
        let pressed = |b: usize| buttons().get(b).is_some_and(|&m| m != 0 && stick.pressed(m));
        {
            // the presses each joystick reports, once per change
            static LAST: [std::sync::atomic::AtomicU32; MAX] = [const { std::sync::atomic::AtomicU32::new(0) }; MAX];
            if LAST[pad].swap(stick.buttons, Ordering::Relaxed) != stick.buttons && stick.buttons != 0 {
                log!("dinput: {} reads buttons {:#x}", name(pad), stick.buttons);
            }
        }
        let n = size as usize;
        unsafe {
            std::ptr::write_bytes(data, 0, n);
            match FORMATS[pad].lock().unwrap().as_ref() {
                Some(fields) => {
                    for &(field, offset) in fields {
                        match field {
                            Field::Axis(a) if offset + 4 <= n => (data.add(offset) as *mut i32).write_unaligned(axis(a)),
                            Field::Button(b) if offset < n => *data.add(offset) = if pressed(b) { 0x80 } else { 0 },
                            Field::Pov if offset + 4 <= n => (data.add(offset) as *mut u32).write_unaligned(u32::MAX),
                            _ => {}
                        }
                    }
                }
                None => {
                    for a in 0..2 {
                        if 4 * (a + 1) <= n {
                            (data.add(4 * a) as *mut i32).write_unaligned(axis(a));
                        }
                    }
                    // POV hats (DIJOYSTATE + 0x20) centered
                    for i in 0..4 {
                        if 0x20 + 4 * (i + 1) <= n {
                            (data.add(0x20 + 4 * i) as *mut u32).write_unaligned(u32::MAX);
                        }
                    }
                    for b in 0..buttons().len().min(32) {
                        if 0x30 + b < n && pressed(b) {
                            *data.add(0x30 + b) = 0x80;
                        }
                    }
                }
            }
        }
        DI_OK
    }

    /// `GetDeviceInfo(pdidi)`: DIDEVICEINSTANCEA or W, by its dwSize.
    unsafe extern "system" fn get_device_info(this: P, info: *mut u8) -> i32 {
        if info.is_null() {
            return super::DIERR_UNSUPPORTED;
        }
        let wide = unsafe { (info as *const u32).read_unaligned() } == 0x44C;
        unsafe { device_instance(info, player(this), wide) };
        DI_OK
    }

    static PADS_A: [Object; MAX] = [const { Object { vtable: PAD_A_VTABLE.0.as_ptr() } }; MAX];
    static PADS_W: [Object; MAX] = [const { Object { vtable: PAD_W_VTABLE.0.as_ptr() } }; MAX];
    static PAD_A_VTABLE: VTable<32> = pad_vtable(enum_objects_a as *const ());
    static PAD_W_VTABLE: VTable<32> = pad_vtable(enum_objects_w as *const ());

    const fn pad_vtable(enum_objects: *const ()) -> VTable<32> {
        let mut v = DEVICE_VTABLE.0;
        v[3] = get_capabilities as *const ();
        v[4] = enum_objects;
        v[5] = get_property as *const ();
        v[6] = set_property as *const ();
        v[9] = get_device_state as *const ();
        v[11] = set_data_format as *const ();
        v[15] = get_device_info as *const ();
        VTable(v)
    }
}
