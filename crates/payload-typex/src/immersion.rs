//! Immersion TouchSense force feedback library (`IFC23.dll`) without a TouchSense wheel: the
//! game's imports of it are answered here, every call succeeds, and the effects the game asks
//! for are kept for [`crate::ffb`] (D1GP Arcade drives its cabinet wheel this way).
//!
//! What is kept:
//! * the spring (`CImmSpring`): on or off (`Start` / `Stop`), its center and saturation
//!   (`Initialize`, `ChangeParameters`). The center is in the library's coordinates, 0 to
//!   twice the center given at `Initialize` (the game uses its screen width);
//! * the named effects of the game's project files (`CImmProject::Start` / `Stop` by name,
//!   `Stop(NULL)` stops all).
//!
//! The methods are `thiscall` (`this` in ecx, the callee pops the arguments): each stub takes
//! the arguments of its MSVC signature.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Instant;

use wal_payload_common::{iat, log};

type P = *mut c_void;

const DLL: &str = "IFC23.dll";

pub(crate) static SPRING_ON: AtomicBool = AtomicBool::new(false);
/// Spring center given at `Initialize` (the middle of the range).
pub(crate) static SPRING_MIDDLE: AtomicI32 = AtomicI32::new(0);
pub(crate) static SPRING_CENTER: AtomicI32 = AtomicI32::new(0);
/// Saturation, 0-10000.
pub(crate) static SPRING_SATURATION: AtomicI32 = AtomicI32::new(10000);
/// Named effects playing, with their (last) start.
pub(crate) static EFFECTS: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn trace() -> bool {
    std::env::var("WAL_TYPEX_FFB_TRACE").is_ok_and(|v| v == "1")
}

unsafe fn name(s: *const c_char) -> Option<String> {
    (!s.is_null()).then(|| unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned())
}

// CImmDevice: a static object, its destructor (slot 0) and status (slot 9) are what the game
// calls.
unsafe extern "thiscall" fn device_delete(_this: P, _flags: u32) -> P {
    std::ptr::null_mut()
}
unsafe extern "thiscall" fn device_status(_this: P, _a: P, _b: P) -> i32 {
    1
}
unsafe extern "thiscall" fn device_other(_this: P) -> i32 {
    0
}

#[repr(C)]
struct Device {
    vtable: *const *const (),
}
unsafe impl Sync for Device {}
struct DeviceVTable([*const (); 16]);
unsafe impl Sync for DeviceVTable {}

static DEVICE_VTABLE: DeviceVTable = {
    let mut v = [device_other as *const (); 16];
    v[0] = device_delete as *const ();
    v[9] = device_status as *const ();
    DeviceVTable(v)
};
static DEVICE: Device = Device { vtable: DEVICE_VTABLE.0.as_ptr() };

/// `static CImmDevice* CImmDevice::CreateDevice(HINSTANCE, HWND)` (cdecl)
unsafe extern "C" fn create_device(_instance: P, _window: P) -> P {
    log!("immersion: device created (no TouchSense wheel: effects go to the launcher)");
    &DEVICE as *const Device as P
}

// Constructors return `this`, destructors do nothing.
unsafe extern "thiscall" fn construct(this: P) -> P {
    this
}
unsafe extern "thiscall" fn destruct(_this: P) {}

/// `CImmSpring::Initialize(CImmDevice*, long, ulong, ulong, ulong, POINT, long, long, int, ulong)`
unsafe extern "thiscall" fn spring_initialize(
    _this: P, _device: P, _coefficient: i32, saturation: u32, _deadband: u32, _axes: u32, x: i32, _y: i32,
    _direction: i32, _a: i32, _b: i32, _c: u32,
) -> i32 {
    SPRING_MIDDLE.store(x, Ordering::Relaxed);
    SPRING_CENTER.store(x, Ordering::Relaxed);
    SPRING_SATURATION.store(saturation.min(10000) as i32, Ordering::Relaxed);
    log!("immersion: spring, center {x}, saturation {saturation}");
    1
}

/// `CImmSpring::InitializePolar(CImmDevice*, long, ulong, ulong, POINT, long, int, ulong)`
unsafe extern "thiscall" fn spring_initialize_polar(
    _this: P, _device: P, _a: i32, _b: u32, _c: u32, _x: i32, _y: i32, _d: i32, _e: i32, _f: u32,
) -> i32 {
    1
}

/// `CImmSpring::ChangeParameters(POINT center, long coefficient, ulong saturation, ulong
/// deadband, long, long)`: 0x80000000 keeps a value.
unsafe extern "thiscall" fn spring_change(
    _this: P, x: i32, _y: i32, _coefficient: i32, saturation: u32, _deadband: u32, _a: i32, _b: i32,
) -> i32 {
    if x != i32::MIN {
        SPRING_CENTER.store(x, Ordering::Relaxed);
    }
    if saturation != 0x8000_0000 {
        SPRING_SATURATION.store(saturation.min(10000) as i32, Ordering::Relaxed);
    }
    1
}

/// `CImmCondition::Start(ulong iterations, ulong flags, int)`
unsafe extern "thiscall" fn condition_start(_this: P, _iterations: u32, _flags: u32, _a: i32) -> i32 {
    if !SPRING_ON.swap(true, Ordering::Relaxed) && trace() {
        log!("immersion: spring on");
    }
    1
}

/// `CImmEffect::Stop()`
unsafe extern "thiscall" fn effect_stop(_this: P) -> i32 {
    if SPRING_ON.swap(false, Ordering::Relaxed) && trace() {
        log!("immersion: spring off");
    }
    1
}

/// `CImmProject::OpenFile(const char*, CImmDevice*)`
unsafe extern "thiscall" fn project_open(_this: P, file: *const c_char, _device: P) -> i32 {
    log!("immersion: project {}", unsafe { name(file) }.unwrap_or_default());
    1
}

/// `CImmProject::Start(const char* name, ulong iterations, ulong flags, CImmDevice*)`
unsafe extern "thiscall" fn project_start(_this: P, effect: *const c_char, _iterations: u32, _flags: u32, _device: P) -> i32 {
    if let Some(effect) = unsafe { name(effect) } {
        let mut effects = EFFECTS.lock().unwrap();
        match effects.iter_mut().find(|(e, _)| *e == effect) {
            Some((_, start)) => *start = Instant::now(),
            None => {
                if trace() {
                    log!("immersion: effect {effect} started");
                }
                effects.push((effect, Instant::now()));
            }
        }
    }
    1
}

/// `CImmProject::Stop(const char* name)`: all of them for NULL.
unsafe extern "thiscall" fn project_stop(_this: P, effect: *const c_char) -> i32 {
    let mut effects = EFFECTS.lock().unwrap();
    match unsafe { name(effect) } {
        Some(effect) => {
            if let Some(i) = effects.iter().position(|(e, _)| *e == effect) {
                if trace() {
                    log!("immersion: effect {effect} stopped");
                }
                effects.remove(i);
            }
        }
        None => effects.clear(),
    }
    1
}

// The other methods: success, nothing kept.
unsafe extern "thiscall" fn ok1(_this: P, _a: P) -> i32 {
    1
}
unsafe extern "thiscall" fn ok3(_this: P, _a: P, _b: P, _c: P) -> i32 {
    1
}
unsafe extern "thiscall" fn ok4(_this: P, _a: P, _b: P, _c: P, _d: P) -> i32 {
    1
}
/// `CImmCondition::GetEffectType()`: a condition (spring).
unsafe extern "thiscall" fn effect_type(_this: P) -> u32 {
    4
}
/// `CImmEffect::Unload()` returns a long, `Reload()` nothing: 0 fits both.
unsafe extern "thiscall" fn zero0(_this: P) -> i32 {
    0
}

pub(crate) fn init() {
    let hooks: &[(&str, *const ())] = &[
        ("?CreateDevice@CImmDevice@@SAPAV1@PAUHINSTANCE__@@PAUHWND__@@@Z", create_device as *const ()),
        ("??0CImmProject@@QAE@XZ", construct as *const ()),
        ("??1CImmProject@@QAE@XZ", destruct as *const ()),
        ("?OpenFile@CImmProject@@QAEHPBDPAVCImmDevice@@@Z", project_open as *const ()),
        ("?Start@CImmProject@@QAEHPBDKKPAVCImmDevice@@@Z", project_start as *const ()),
        ("?Stop@CImmProject@@QAEHPBD@Z", project_stop as *const ()),
        ("??0CImmSpring@@QAE@XZ", construct as *const ()),
        ("??1CImmSpring@@UAE@XZ", destruct as *const ()),
        ("?Initialize@CImmSpring@@UAEHPAVCImmDevice@@JKKKUtagPOINT@@JJHK@Z", spring_initialize as *const ()),
        ("?InitializePolar@CImmSpring@@UAEHPAVCImmDevice@@JKKUtagPOINT@@JHK@Z", spring_initialize_polar as *const ()),
        ("?ChangeParameters@CImmSpring@@QAEHUtagPOINT@@JKKJJ@Z", spring_change as *const ()),
        ("?Start@CImmCondition@@UAEHKKH@Z", condition_start as *const ()),
        ("?Stop@CImmEffect@@UAEHXZ", effect_stop as *const ()),
        ("?ChangeParameters@CImmFriction@@QAEHKKJJ@Z", ok4 as *const ()),
        ("?GetIsCompatibleGUID@CImmSpring@@UAEHAAU_GUID@@@Z", ok1 as *const ()),
        ("?GetEffectType@CImmCondition@@UAEKXZ", effect_type as *const ()),
        ("?Initialize@CImmCondition@@UAEHPAVCImmDevice@@ABUFEELIT_EFFECT@@K@Z", ok3 as *const ()),
        ("?InitializeFromProject@CImmEffect@@UAEHAAVCImmProject@@PBDPAVCImmDevice@@K@Z", ok4 as *const ()),
        ("?Unload@CImmEffect@@UAEJXZ", zero0 as *const ()),
        ("?Reload@CImmEffect@@UAEXXZ", zero0 as *const ()),
        ("?load_data@CImmCondition@@UAEHABUFEELIT_EFFECT@@PBDK@Z", ok3 as *const ()),
        ("?buffer_ifr_data@CImmCondition@@UAEHPAD@Z", ok1 as *const ()),
        ("?get_ffe_data@CImmCondition@@MAEHPAX@Z", ok1 as *const ()),
    ];
    let mut n = 0;
    for (function, replacement) in hooks {
        if unsafe { iat::hook(DLL, function, *replacement as usize) }.is_some() {
            n += 1;
        }
    }
    log!("immersion: {n} of {} {DLL} imports answered", hooks.len());
}
