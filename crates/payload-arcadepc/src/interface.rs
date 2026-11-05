//! Hog Wild's I/O library (`bin/Interface.dll`, `WAL_ARCADEPC_BOARD=interface`): the payload is
//! it (`payload_install`, x86 build only) and forwards the game's `CInterface` calls to the dump's
//! original library (`Interface.ORI.dll`), whose board is emulated below it.
//!
//! The game (`ft3d_GameEngine.dll` and the executable) imports a C++ class `CInterface`
//! (thiscall methods, MSVC names in `interface.def`); `ft3d_GameEngine.dll` also reads its fields
//! inline (handle center and limits), so the object stays the original library's. That library
//! drives the cabinet's board on the parallel port through the PortTalk driver: its port
//! routines (`Inputb`, `Outputb`) are redirected here and `OpenInterface` answers without the
//! driver. The board, by the control register written before a read of the data port (0x378):
//!
//! * `0x24`: buttons A, `0x2E`: buttons B (a bit at 0 = pressed);
//! * `0x2C`: the next bit (MSB first) of the handlebar's ADC0831, a conversion started by the
//!   chip select (data bit 2) going low;
//! * `0x26`: the protection check, 0 = the cabinet's.
//!
//! Lamps, the handlebar's motor and the seat's vibration are written to the data port: ignored.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The board's inputs: `GetButtonValue`'s `Input_Type` values 0..=10 (the operator's I/O check
/// names them; 9 and 10 are not read) and the handlebar turned fully by a button.
#[derive(Clone, Copy, PartialEq)]
enum Native {
    Coin,
    Start,
    Service,
    Test,
    Accelerator,
    GearHigh,
    GearLow,
    Brake,
    ViewChange,
    SteerLeft,
    SteerRight,
}

/// The (second register, bit) of a board button.
fn button_bit(n: Native) -> Option<(bool, u8)> {
    Some(match n {
        Native::Coin => (false, 0x80),
        Native::Start => (false, 0x04),
        Native::Service => (true, 0x04),
        Native::Test => (true, 0x08),
        Native::Accelerator => (false, 0x40),
        Native::GearHigh => (true, 0x02),
        Native::GearLow => (true, 0x01),
        Native::Brake => (true, 0x10),
        Native::ViewChange => (false, 0x02),
        Native::SteerLeft | Native::SteerRight => return None,
    })
}

const NATIVES: &[(&str, Native)] = &[
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("service", Native::Service),
    ("test", Native::Test),
    ("accelerator", Native::Accelerator),
    ("gear_high", Native::GearHigh),
    ("gear_low", Native::GearLow),
    ("brake", Native::Brake),
    ("view", Native::ViewChange),
    ("steer_left", Native::SteerLeft),
    ("steer_right", Native::SteerRight),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("coin", "coin"),
    ("start", "start"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "accelerator"),
    ("b2", "brake"),
    ("b3", "gear_high"),
    ("b4", "gear_low"),
    ("b5", "view"),
    ("up", "accelerator"),
    ("down", "brake"),
    ("left", "steer_left"),
    ("right", "steer_right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The last control register value written.
static CONTROL: AtomicU8 = AtomicU8::new(0);
/// The ADC's bits read since its chip select went low, and the value converted.
static ADC_BIT: AtomicU8 = AtomicU8::new(0);
static ADC_VALUE: AtomicU8 = AtomicU8::new(0x80);

/// The board's two button registers, active low; the pedal axes past a quarter are the
/// accelerator and the brake.
fn buttons() -> (u8, u8) {
    let stick = input(0);
    let (mut a, mut b) = (0xFFu8, 0xFFu8);
    let mut press = |n: Native| {
        if let Some((second, bit)) = button_bit(n) {
            if second { b &= !bit } else { a &= !bit }
        }
    };
    map().for_each_pressed(&stick, &mut press);
    if stick.axis(Axis::Accel) > 8192 {
        press(Native::Accelerator);
    }
    if stick.axis(Axis::Brake) > 8192 {
        press(Native::Brake);
    }
    (a, b)
}

/// The handlebar's ADC value: 0x80 centered, the left stick (right = higher), the steering
/// buttons fully turned.
fn handlebar() -> u8 {
    let stick = input(0);
    let (mut left, mut right) = (false, false);
    map().for_each_pressed(&stick, |n| match n {
        Native::SteerLeft => left = true,
        Native::SteerRight => right = true,
        _ => {}
    });
    let lx = match (left, right) {
        (true, false) => -32767,
        (false, true) => 32767,
        _ => stick.axis(Axis::LeftX) as i32,
    };
    (0x80 + lx * 0x7F / 32767).clamp(0, 0xFF) as u8
}

type This = *mut u8;

/// The original `Inputb`: a byte of the board.
extern "thiscall" fn inputb(_this: This, port: u32) -> u8 {
    if port as u16 != 0x378 {
        return 0xFF;
    }
    match CONTROL.load(Ordering::Relaxed) {
        0x24 => buttons().0,
        0x2E => buttons().1,
        0x26 => 0,
        0x2C => {
            let n = ADC_BIT.fetch_add(1, Ordering::Relaxed);
            if n == 0 {
                ADC_VALUE.store(handlebar(), Ordering::Relaxed);
            }
            if n < 8 { (ADC_VALUE.load(Ordering::Relaxed) >> (7 - n)) & 1 } else { 0 }
        }
        _ => 0xFF,
    }
}

/// The original `Outputb`: the control register selects what the next read returns; a chip
/// select on the data port starts an ADC conversion.
extern "thiscall" fn outputb(_this: This, port: u32, value: u32) {
    let value = value as u8;
    match port as u16 {
        0x37A => CONTROL.store(value, Ordering::Relaxed),
        0x378 if value & 0x04 != 0 => ADC_BIT.store(0, Ordering::Relaxed),
        _ => {}
    }
}

// The original library's routines patched (RVAs of the dump's Interface.ORI.dll) and the bytes
// expected there.
const OPEN_INTERFACE: usize = 0x1280;
const INPUTB: usize = 0x15F0;
const OUTPUTB: usize = 0x1650;
const OPEN_INTERFACE_CODE: [u8; 4] = [0x56, 0x8B, 0xF1, 0x57];
const INPUTB_CODE: [u8; 4] = [0x51, 0x83, 0x79, 0x0C];
const OUTPUTB_CODE: [u8; 4] = [0x83, 0x79, 0x0C, 0x01];

unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
    }
}

fn jump(from: usize, to: usize) -> [u8; 5] {
    let mut code = [0xE9, 0, 0, 0, 0];
    code[1..].copy_from_slice(&(to.wrapping_sub(from + 5) as u32).to_le_bytes());
    code
}

// The forwarded methods, in `interface.def`'s order.
const METHODS: [&[u8]; 17] = [
    b"??0CInterface@@QAE@XZ\0",
    b"??1CInterface@@QAE@XZ\0",
    b"?InterfaceInitialize@CInterface@@QAEXXZ\0",
    b"?Clear@CInterface@@QAEXXZ\0",
    b"?GetADC0831Value@CInterface@@QAEHXZ\0",
    b"?GetButtonValue@CInterface@@QAEEW4Input_Type@@@Z\0",
    b"?SetButtonLamp@CInterface@@QAEXW4Lamp_Type@@H@Z\0",
    b"?SetHardwareLock@CInterface@@QAEXH@Z\0",
    b"?SecretChcek@CInterface@@QAEHXZ\0",
    b"?HandleCenterCheck@CInterface@@QAEHXZ\0",
    b"?HandleStrength@CInterface@@QAEHH@Z\0",
    b"?SetFeedBackLevel@CInterface@@QAEXUHardware_Parameter_s@@@Z\0",
    b"?HandleSetting@CInterface@@QAEXMH@Z\0",
    b"?Handle_and_Vibration@CInterface@@QAEXHHHH@Z\0",
    b"?SetCollision_and_Position@CInterface@@QAEXW4FIRST_TYPE@@HHHH@Z\0",
    b"?CollisionCheck@CInterface@@QAE_NM@Z\0",
    b"?CollisionCheck@CInterface@@QAEHHW4Collision_Type@@@Z\0",
];

/// The original library's methods, loaded and patched at the first call.
fn original() -> &'static [usize; 17] {
    static ORIGINAL: OnceLock<[usize; 17]> = OnceLock::new();
    ORIGINAL.get_or_init(|| {
        let module = unsafe { LoadLibraryA(b"Interface.ORI.dll\0".as_ptr()) };
        if module.is_null() {
            log!("interface: Interface.ORI.dll not found, the board does nothing");
            return [0; 17];
        }
        let base = module as usize;
        let code = |rva: usize| unsafe { std::slice::from_raw_parts((base + rva) as *const u8, 4) };
        if code(OPEN_INTERFACE) == OPEN_INTERFACE_CODE && code(INPUTB) == INPUTB_CODE && code(OUTPUTB) == OUTPUTB_CODE {
            unsafe {
                // OpenInterface: mov eax, 1; ret (no PortTalk driver)
                write(base + OPEN_INTERFACE, &[0xB8, 0x01, 0x00, 0x00, 0x00, 0xC3]);
                write(base + INPUTB, &jump(base + INPUTB, inputb as *const () as usize));
                write(base + OUTPUTB, &jump(base + OUTPUTB, outputb as *const () as usize));
            }
            log!("interface: Interface.ORI.dll at {base:#x}, board emulated");
        } else {
            log!("interface: unknown Interface.ORI.dll build, its port routines left alone");
        }
        METHODS.map(|name| unsafe { GetProcAddress(module, name.as_ptr()) }.map_or(0, |f| f as usize))
    })
}

macro_rules! forward {
    ($($index:literal $name:ident ($($arg:ident),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "thiscall" fn $name(this: This $(, $arg: u32)*) -> u32 {
            let f = original()[$index];
            if f == 0 {
                return 0;
            }
            let f: extern "thiscall" fn(This $(, forward!(@u32 $arg))*) -> u32 = unsafe { std::mem::transmute(f) };
            f(this $(, $arg)*)
        }
    )*};
    (@u32 $arg:ident) => { u32 };
}

forward! {
    0 iface_new();
    1 iface_delete();
    2 iface_initialize();
    3 iface_clear();
    4 iface_adc0831_value();
    5 iface_button_value(kind);
    6 iface_set_button_lamp(lamp, on);
    7 iface_set_hardware_lock(lock);
    8 iface_secret_check();
    9 iface_handle_center_check();
    10 iface_handle_strength(strength);
    11 iface_set_feedback_level(a, b, c, d);
    12 iface_handle_setting(velocity, collision);
    13 iface_handle_and_vibration(a, b, c, d);
    14 iface_set_collision_and_position(first, a, b, c, d);
    15 iface_collision_check(velocity);
    16 iface_collision_check2(value, kind);
}
