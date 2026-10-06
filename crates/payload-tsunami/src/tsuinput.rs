//! The TsuInput cabinet COM object, emulated from the virtual arcade stick.
//!
//! The game does `CoCreateInstance(CLSID {74B3FA08-…}, IID_IUnknown)`, then QIs for the
//! cabinet interface `{5AE0E518-373D-4323-98DE-EAF0046041E5}` and caches the object in a
//! global. The cabinet normally provides it as an in-proc server (TsuInputLib.dll) that is
//! not in the dump, so we replace it here by hooking `ole32!CoCreateInstance`.
//!
//! The arg counts come from the call sites in ReVolt.exe (each slot keeps the exact stdcall
//! arg count); the slot names from TsuInputLib.dll, whose 27 flat `TsuInput*` exports are thin
//! wrappers calling one slot each of this same interface:
//!
//! * [7] Attach                   * [8] Detach                  * [9] LastError
//! * [10] NumCoins(out)           * [11] CoinsPerPlay(out)      * [12] SufficientCredit(out)
//! * [13] DeductPlay()            * [14] GetCoinMode(out)       * [15] SetCoinMode(dw)
//! * [16] GetCoinType(out)        * [17] GetMotionEnabled(out)  * [18] GetVolumeMode(out)
//! * [19] SetVolumeMode(dw)       * [20] GetVolume(out f32)     * [21] GetJoyInfo(o1..o5)
//! * [22] SetJoyRange(axis,min,max)                             * [23] SimulateCoin()
//! * [24] SimulateMotionSwitch(f) * [25] SimulateOperatorSwitch()
//! * [28] PulseWatchdog()         * [37] EndPlay()              * [38] StartPlay()
//! * [39] SetStatParam(dw)        * [40] SetWatchdogTimerTimeout(ms)
//!
//! The wheel, the pedals and the cabinet buttons are all read through GetJoyInfo (the game's
//! "Driving Pod" controller; DirectInput only provides the keyboard, see `dinput.rs`).
//! A race starts with a credit (SufficientCredit) and the gas pedal pressed past 3/4.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::log;
use wal_protocol::{Axis, button};
use windows_sys::core::GUID;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

type P = *mut c_void;
type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;
const E_NOTIMPL: HRESULT = 0x8000_4001_u32 as i32;

/// {74B3FA08-50CC-4275-A5CF-8BBF6DDB75C2}
const CLSID_TSUINPUT: GUID = GUID::from_u128(0x74b3fa08_50cc_4275_a5cf_8bbf6ddb75c2);
/// {00000000-0000-0000-C000-000000000046}
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
/// {5AE0E518-373D-4323-98DE-EAF0046041E5}
const IID_TSUINPUT: GUID = GUID::from_u128(0x5ae0e518_373d_4323_98de_eaf0046041e5);
/// {00000016-0000-0000-C000-000000000046}
const IID_ICLASSFACTORY: GUID = GUID::from_u128(0x00000016_0000_0000_c000_000000000046);

static ORIG_CO_CREATE: AtomicUsize = AtomicUsize::new(0);
static REFCOUNT: AtomicU32 = AtomicU32::new(1);
static CREDITS: AtomicU32 = AtomicU32::new(0);
static COIN_DOWN: AtomicU32 = AtomicU32::new(0);
const MAX_CREDITS: u32 = 9;
/// SetJoyRange per axis (0 wheel, 1 brake, 2 gas): (min, max). The game picks a pedal layout
/// with it (separate pedals 0..255, inverted gas -255..0, or one combined axis 0..511).
static JOY_RANGE: [(AtomicI32, AtomicI32); 3] = [const { (AtomicI32::new(0), AtomicI32::new(255)) }; 3];
/// Object cell: a memory location whose first word is the vtable address (COM `this` layout).
static OBJECT_CELL: AtomicUsize = AtomicUsize::new(0);

const N_SLOTS: usize = 49;

/// The COM vtable, filled at init (function-to-address casts are not const-evaluable).
static VTABLE: [AtomicUsize; N_SLOTS] = [const { AtomicUsize::new(0) }; N_SLOTS];

/// Last logged value per method slot, to keep the log small for per-frame polls.
static LAST_LOG: [AtomicU32; N_SLOTS] = [const { AtomicU32::new(u32::MAX) }; N_SLOTS];

fn object_ptr() -> P {
    &OBJECT_CELL as *const AtomicUsize as P
}

/// Log a method call once per distinct argument value (per-frame polls stay quiet).
fn log_call(slot: u32, value: u32, what: &str) {
    let last = LAST_LOG[slot as usize].load(Ordering::Relaxed);
    if last == value && last != u32::MAX {
        return;
    }
    LAST_LOG[slot as usize].store(value, Ordering::Relaxed);
    log!("tsuinput: m{slot} {what} = {value:#x}");
}

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    if a.is_null() {
        return false;
    }
    let a = unsafe { &*a };
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn format_guid(g: *const GUID) -> String {
    let g = unsafe { &*g };
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3,
        g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    )
}

/// Current wheel position, raw 0..=254 centered on 127 (the game reads `(raw - 127) / 127`),
/// from the virtual stick left X axis.
fn wheel_raw() -> u32 {
    let lx = wal_payload_common::input(0).axes[Axis::LeftX as usize] as i32;
    (127 + (lx * 127 / 32767).clamp(-127, 127)) as u32
}

/// Count a credit on each coin button press. Called from the per-frame polls.
fn poll_coin() {
    let down = wal_payload_common::input(0).pressed(button::COIN);
    if down && COIN_DOWN.swap(1, Ordering::Relaxed) == 0 {
        add_credit();
    } else if !down {
        COIN_DOWN.store(0, Ordering::Relaxed);
    }
}

fn add_credit() {
    let c = CREDITS.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if c > MAX_CREDITS {
        CREDITS.store(MAX_CREDITS, Ordering::Relaxed);
    }
    log!("tsuinput: coin -> credits={}", c.min(MAX_CREDITS));
}

/// Cabinet buttons, as the GetJoyInfo bitmask: bit 0 WHEEL (the wheel's button, also the menu
/// select), 1 ABORT, 2 MUSIC, 3 VIEW1, 4 VIEW2, 5 VIEW3 (the game's "Driving Pod" names).
/// START is not one of them: in attract mode a cabinet button with a credit quits the game.
fn cabinet_buttons() -> u32 {
    let input = wal_payload_common::input(0);
    let map = [
        (button::B1, 1 << 0),
        (button::B2, 1 << 1),
        (button::B3, 1 << 2),
        (button::B4, 1 << 3),
        (button::B5, 1 << 4),
        (button::B6, 1 << 5),
    ];
    map.iter().filter(|(b, _)| input.buttons & b != 0).fold(0, |m, (_, bit)| m | bit)
}

// --- IUnknown ---

#[allow(non_snake_case)]
unsafe extern "system" fn QueryInterface(this: P, riid: *const GUID, ppv: *mut P) -> HRESULT {
    let r = if riid.is_null() { String::from("?") } else { format_guid(riid) };
    log!("tsuinput: QueryInterface({r})");
    let ok = guid_eq(riid, &IID_IUNKNOWN) || guid_eq(riid, &IID_TSUINPUT);
    if !ppv.is_null() {
        unsafe { *ppv = if ok { this } else { std::ptr::null_mut() } };
    }
    if ok {
        REFCOUNT.fetch_add(1, Ordering::Relaxed);
        S_OK
    } else {
        E_NOINTERFACE
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn AddRef(_this: P) -> u32 {
    REFCOUNT.fetch_add(1, Ordering::Relaxed)
}

#[allow(non_snake_case)]
unsafe extern "system" fn Release(_this: P) -> u32 {
    let n = REFCOUNT.fetch_sub(1, Ordering::Relaxed);
    if n == 1 {
        log!("tsuinput: released (final)");
    }
    n.saturating_sub(1)
}

// --- Unimplemented slots (the game never calls these; 0 args) ---

#[allow(non_snake_case)]
unsafe extern "system" fn NotImpl(_this: P) -> HRESULT {
    E_NOTIMPL
}

// --- Cabinet methods ---

/// [7] (this, a, b, c): one-time attach right after creation. Args are NULL/0 from the game.
#[allow(non_snake_case)]
unsafe extern "system" fn m7(_this: P, a: u32, b: u32, c: u32) -> HRESULT {
    log_call(7, a | (b << 8) | (c << 16), "Attach");
    S_OK
}

/// [8] (this): detach, called on game shutdown.
#[allow(non_snake_case)]
unsafe extern "system" fn m8(_this: P) -> HRESULT {
    log_call(8, 0, "Detach");
    S_OK
}

/// [10] NumCoins(out): credits inserted. Coins come from the virtual stick's coin button.
#[allow(non_snake_case)]
unsafe extern "system" fn m10(_this: P, out: *mut u32) -> HRESULT {
    poll_coin();
    let v = CREDITS.load(Ordering::Relaxed);
    if !out.is_null() {
        unsafe { *out = v };
    }
    log_call(10, v, "NumCoins");
    S_OK
}

/// [11] CoinsPerPlay(out): one coin per play (0 would mean free play).
#[allow(non_snake_case)]
unsafe extern "system" fn m11(_this: P, out: *mut u32) -> HRESULT {
    poll_coin();
    if !out.is_null() {
        unsafe { *out = 1 };
    }
    log_call(11, 1, "CoinsPerPlay");
    S_OK
}

/// [12] SufficientCredit(out): a play is paid; with it the gas pedal starts a race.
#[allow(non_snake_case)]
unsafe extern "system" fn m12(_this: P, out: *mut u32) -> HRESULT {
    poll_coin();
    let v = (CREDITS.load(Ordering::Relaxed) > 0) as u32;
    if !out.is_null() {
        unsafe { *out = v };
    }
    log_call(12, v, "SufficientCredit");
    S_OK
}

/// [13] DeductPlay(): a race was started, take its credit.
#[allow(non_snake_case)]
unsafe extern "system" fn m13(_this: P) -> HRESULT {
    let left = CREDITS.load(Ordering::Relaxed).saturating_sub(1);
    CREDITS.store(left, Ordering::Relaxed);
    log!("tsuinput: m13 DeductPlay -> credits={left}");
    S_OK
}

/// [14] GetCoinMode(out).
#[allow(non_snake_case)]
unsafe extern "system" fn m14(_this: P, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_call(14, 0, "GetCoinMode");
    S_OK
}

/// [15] SetCoinMode(dw).
#[allow(non_snake_case)]
unsafe extern "system" fn m15(_this: P, flag: u32) -> HRESULT {
    log_call(15, flag, "SetCoinMode");
    S_OK
}

/// [16] GetCoinType(out).
#[allow(non_snake_case)]
unsafe extern "system" fn m16(_this: P, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_call(16, 0, "GetCoinType");
    S_OK
}

/// [17] GetMotionEnabled(out): no motion seat.
#[allow(non_snake_case)]
unsafe extern "system" fn m17(_this: P, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_call(17, 0, "GetMotionEnabled");
    S_OK
}

/// [18] GetVolumeMode(out).
#[allow(non_snake_case)]
unsafe extern "system" fn m18(_this: P, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_call(18, 0, "GetVolumeMode");
    S_OK
}

/// [19] SetVolumeMode(dw).
#[allow(non_snake_case)]
unsafe extern "system" fn m19(_this: P, dw: u32) -> HRESULT {
    log_call(19, dw, "SetVolumeMode");
    S_OK
}

/// [20] (this, out f32): volume, 1.0 = full.
#[allow(non_snake_case)]
unsafe extern "system" fn m20(_this: P, out: *mut f32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 1.0 };
    }
    log_call(20, 1.0f32.to_bits(), "GetVolume");
    S_OK
}

/// [21] GetJoyInfo(wheel, brake, gas, pov, buttons): the per-frame controller refresh
/// (the game's joy struct at 0x9f746c: +4 wheel, +8 brake, +0xc gas, +0x1c buttons).
/// The flat `TsuInputGetJoyInfo` fills a DIJOYSTATE with them (lX, lY, lZ, rgdwPOV[0],
/// rgbButtons), the game's "Driving Pod" axes Wheel / Brake / Gas.
#[allow(non_snake_case)]
unsafe extern "system" fn m21(
    _this: P,
    wheel: *mut i32,
    brake: *mut i32,
    gas: *mut i32,
    pov: *mut i32,
    buttons: *mut u32,
) -> HRESULT {
    poll_coin();
    let input = wal_payload_common::input(0);
    let (w, b, g) = (wheel_raw() as i32, input.axis_u8(Axis::Brake) as i32, input.axis_u8(Axis::Accel) as i32);
    let (brake_v, gas_v) = pedal_values(b, g);
    let btn = cabinet_buttons();
    unsafe {
        if !wheel.is_null() {
            *wheel = w;
        }
        if !brake.is_null() {
            *brake = brake_v;
        }
        if !gas.is_null() {
            *gas = gas_v;
        }
        if !pov.is_null() {
            *pov = -1; // centered
        }
        if !buttons.is_null() {
            *buttons = btn;
        }
    }
    log_call(21, (w as u32) | (b as u32) << 8 | (g as u32) << 16 | btn << 24, "GetJoyInfo wheel|brake<<8|gas<<16|buttons<<24");
    S_OK
}

/// Brake and gas (0..=255 pressed) as the GetJoyInfo axis values of the layout the game set
/// with SetJoyRange (game side: 0x4c8110 brake, 0x4c8180 gas).
fn pedal_values(brake: i32, gas: i32) -> (i32, i32) {
    let range = |axis: usize| (JOY_RANGE[axis].0.load(Ordering::Relaxed), JOY_RANGE[axis].1.load(Ordering::Relaxed));
    if range(1).1 > 255 {
        // one combined axis on the brake slot: 255 at rest, up = brake, down = gas
        ((255 + brake - gas).clamp(0, 510), 0)
    } else if range(2).0 < 0 {
        // brake centered on 127, gas inverted (-255 = floored)
        (127 + brake * 128 / 255, -gas)
    } else {
        (brake, gas)
    }
}

/// [22] SetJoyRange(axis, min, max): axis 0 wheel, 1 brake, 2 gas.
#[allow(non_snake_case)]
unsafe extern "system" fn m22(_this: P, axis: u32, min: i32, max: i32) -> HRESULT {
    if let Some((lo, hi)) = JOY_RANGE.get(axis as usize) {
        lo.store(min, Ordering::Relaxed);
        hi.store(max, Ordering::Relaxed);
    }
    log!("tsuinput: m22 SetJoyRange(axis {axis}, {min}..{max})");
    S_OK
}

/// [23] SimulateCoin(): the game's 'C' key.
#[allow(non_snake_case)]
unsafe extern "system" fn m23(_this: P) -> HRESULT {
    add_credit();
    S_OK
}

/// [24] SimulateMotionSwitch(flag): the game's '9' key.
#[allow(non_snake_case)]
unsafe extern "system" fn m24(_this: P, flag: u32) -> HRESULT {
    log_call(24, flag, "SimulateMotionSwitch");
    S_OK
}

/// [25] SimulateOperatorSwitch(): the game's '7' key.
#[allow(non_snake_case)]
unsafe extern "system" fn m25(_this: P) -> HRESULT {
    log_call(25, 0, "SimulateOperatorSwitch");
    S_OK
}

/// [28] (this): watchdog pulse, every frame.
#[allow(non_snake_case)]
unsafe extern "system" fn m28(_this: P) -> HRESULT {
    static ONCE: AtomicU32 = AtomicU32::new(0);
    if ONCE.swap(1, Ordering::Relaxed) == 0 {
        log!("tsuinput: m28 PulseWatchdog (per-frame)");
    }
    S_OK
}

/// [37] EndPlay().
#[allow(non_snake_case)]
unsafe extern "system" fn m37(_this: P) -> HRESULT {
    log_call(37, 0, "EndPlay");
    S_OK
}

/// [38] StartPlay().
#[allow(non_snake_case)]
unsafe extern "system" fn m38(_this: P) -> HRESULT {
    log_call(38, 0, "StartPlay");
    S_OK
}

/// [39] SetStatParam(dw): the game reports its state to the cabinet (~50 ms).
#[allow(non_snake_case)]
unsafe extern "system" fn m39(_this: P, mask: u32) -> HRESULT {
    log_call(39, mask, "SetStatParam");
    S_OK
}

/// [40] (this, ms): watchdog timeout (the game sets 60000).
#[allow(non_snake_case)]
unsafe extern "system" fn m40(_this: P, ms: u32) -> HRESULT {
    log_call(40, ms, "SetWatchdogTimeout");
    S_OK
}

type CoCreateFn = unsafe extern "system" fn(*const GUID, P, u32, *const GUID, *mut P) -> HRESULT;

unsafe extern "system" fn co_create_instance(clsid: *const GUID, outer: P, ctx: u32, iid: *const GUID, out: *mut P) -> HRESULT {
    if guid_eq(clsid, &CLSID_TSUINPUT) {
        log!("tsuinput: CoCreateInstance(CLSID_TsuInput, ctx={ctx:#x}, riid={}) -> our object", format_guid(iid));
        if !out.is_null() {
            unsafe { *out = object_ptr() };
        }
        return S_OK;
    }
    if guid_eq(clsid, &super::tsumotion::CLSID_TSUMOTION) {
        log!("tsuinput: CoCreateInstance(CLSID_TsuMotion) -> our idle motion seat");
        if !out.is_null() {
            unsafe { *out = super::tsumotion::object_ptr() };
        }
        return S_OK;
    }
    let orig: CoCreateFn = unsafe { std::mem::transmute(ORIG_CO_CREATE.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(clsid, outer, ctx, iid, out) };
    log!("tsuinput: CoCreateInstance forwarded clsid={} -> {hr:#x}", format_guid(clsid));
    hr
}

// --- Class factory (in-proc COM server) ---
//
// The payload also registers the TsuInput object as an in-proc server (this DLL), so the game
// can obtain it through Wine's COM even if the `CoCreateInstance` IAT hook is lost (Wine re-fixes
// the game's IAT when it loads a new module and restores the hooked slot). Wine calls
// `DllGetClassObject` (lib.rs) and QIs/creates through this factory.

const FACTORY_SLOTS: usize = 5; // IUnknown (3) + IClassFactory (2)
static FACTORY_VTABLE: [AtomicUsize; FACTORY_SLOTS] = [const { AtomicUsize::new(0) }; FACTORY_SLOTS];
static FACTORY_CELL: AtomicUsize = AtomicUsize::new(0);
static FACTORY_REFCOUNT: AtomicU32 = AtomicU32::new(1);

fn factory_ptr() -> P {
    &FACTORY_CELL as *const AtomicUsize as P
}

#[allow(non_snake_case)]
unsafe extern "system" fn factory_qi(this: P, riid: *const GUID, out: *mut P) -> HRESULT {
    let ok = guid_eq(riid, &IID_IUNKNOWN) || guid_eq(riid, &IID_ICLASSFACTORY);
    if !out.is_null() {
        unsafe { *out = if ok { this } else { std::ptr::null_mut() } };
    }
    if ok {
        FACTORY_REFCOUNT.fetch_add(1, Ordering::Relaxed);
        S_OK
    } else {
        E_NOINTERFACE
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn factory_addref(_this: P) -> u32 {
    FACTORY_REFCOUNT.fetch_add(1, Ordering::Relaxed) + 1
}

#[allow(non_snake_case)]
unsafe extern "system" fn factory_release(_this: P) -> u32 {
    FACTORY_REFCOUNT.fetch_sub(1, Ordering::Relaxed);
    1 // the factory lives for the life of the process
}

#[allow(non_snake_case)]
unsafe extern "system" fn factory_create_instance(_this: P, outer: P, riid: *const GUID, out: *mut P) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    if !outer.is_null() {
        return E_NOTIMPL; // no aggregation
    }
    if guid_eq(riid, &IID_IUNKNOWN) || guid_eq(riid, &IID_TSUINPUT) {
        unsafe { AddRef(object_ptr()) };
        unsafe { *out = object_ptr() };
        S_OK
    } else {
        E_NOINTERFACE
    }
}

#[allow(non_snake_case)]
unsafe extern "system" fn factory_lock_server(_this: P, _lock: u32) -> HRESULT {
    S_OK
}

/// `DllGetClassObject` for this payload: hands out the factory for the TsuInput CLSID.
pub unsafe extern "system" fn dll_get_class_object(rclsid: *const GUID, riid: *const GUID, out: *mut P) -> HRESULT {
    if !guid_eq(rclsid, &CLSID_TSUINPUT) {
        return E_NOTIMPL;
    }
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    if guid_eq(riid, &IID_IUNKNOWN) || guid_eq(riid, &IID_ICLASSFACTORY) {
        unsafe { *out = factory_ptr() };
        FACTORY_REFCOUNT.fetch_add(1, Ordering::Relaxed);
        S_OK
    } else {
        E_NOINTERFACE
    }
}

pub fn init() {
    // (index, function address); everything not listed is E_NOTIMPL.
    let known: [(u32, usize); 26] = [
        (0, QueryInterface as *const () as usize),
        (1, AddRef as *const () as usize),
        (2, Release as *const () as usize),
        (7, m7 as *const () as usize),
        (8, m8 as *const () as usize),
        (10, m10 as *const () as usize),
        (11, m11 as *const () as usize),
        (12, m12 as *const () as usize),
        (13, m13 as *const () as usize),
        (14, m14 as *const () as usize),
        (15, m15 as *const () as usize),
        (16, m16 as *const () as usize),
        (17, m17 as *const () as usize),
        (18, m18 as *const () as usize),
        (19, m19 as *const () as usize),
        (20, m20 as *const () as usize),
        (21, m21 as *const () as usize),
        (22, m22 as *const () as usize),
        (23, m23 as *const () as usize),
        (24, m24 as *const () as usize),
        (25, m25 as *const () as usize),
        (28, m28 as *const () as usize),
        (37, m37 as *const () as usize),
        (38, m38 as *const () as usize),
        (39, m39 as *const () as usize),
        (40, m40 as *const () as usize),
    ];
    let notimpl = NotImpl as *const () as usize;
    for slot in VTABLE.iter() {
        slot.store(notimpl, Ordering::Relaxed);
    }
    for (i, f) in known {
        VTABLE[i as usize].store(f, Ordering::Relaxed);
    }
    OBJECT_CELL.store(VTABLE.as_ptr() as usize, Ordering::Relaxed);
    if let Some(o) = unsafe { wal_payload_common::iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        ORIG_CO_CREATE.store(o, Ordering::Relaxed);
        log!("tsuinput: CoCreateInstance hooked (TsuInput object provided)");
    } else {
        log!("tsuinput: no ole32!CoCreateInstance in the game IAT to hook");
    }

    // Fill the class factory vtable (IUnknown + CreateInstance + LockServer).
    let factory: [(usize, usize); FACTORY_SLOTS] = [
        (0, factory_qi as *const () as usize),
        (1, factory_addref as *const () as usize),
        (2, factory_release as *const () as usize),
        (3, factory_create_instance as *const () as usize),
        (4, factory_lock_server as *const () as usize),
    ];
    for slot in FACTORY_VTABLE.iter() {
        slot.store(factory[0].1, Ordering::Relaxed);
    }
    for (i, f) in factory {
        FACTORY_VTABLE[i].store(f, Ordering::Relaxed);
    }
    FACTORY_CELL.store(FACTORY_VTABLE.as_ptr() as usize, Ordering::Relaxed);

    // Also expose the object as an in-proc COM server of this payload, so it survives the IAT
    // being re-fixed (Wine restores hooked slots when it loads a new module into the game).
    if let Some(path) = crate::dll_path() {
        super::tsunet::register_inproc("{74B3FA08-50CC-4275-A5CF-8BBF6DDB75C2}", "TsuInput Class", &path);
    }

    // Watchdog: Wine re-fixes the game's IAT on later module loads and can restore the hooked
    // CoCreateInstance slot; re-hook it whenever it drifts.
    if let Some(slot) = unsafe { wal_payload_common::iat::hook_addr("ole32.dll", "CoCreateInstance") } {
        let hook = co_create_instance as usize;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let cur = unsafe { (slot as *const usize).read_volatile() };
                if cur == hook {
                    continue;
                }
                unsafe {
                    let slot = slot as *mut usize;
                    let mut old = 0;
                    VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old);
                    slot.write_volatile(hook);
                    VirtualProtect(slot.cast(), size_of::<usize>(), old, &mut old);
                }
                log!("tsuinput: CoCreateInstance IAT slot was {cur:#x}, re-hooked");
            }
        });
    }
}
