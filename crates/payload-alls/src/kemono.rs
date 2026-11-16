//! Kemono Friends 3 Planet Tours (Nu SX, a 32-bit Mono Unity game, amdaemon 64-bit): the game's
//! devices and the cabinet it looks for.
//!
//! - The cabinet: `AmManager.checkTarget` takes the machine for a cabinet when `USERNAME` is
//!   `AppUser` on Windows 8 / 8.1 (`Environment.OSVersion` 6.2 / 6.3); on another machine it
//!   kills the running amdaemon (`Core.Kill`) and starts a developer's `amdaemon_emu.exe`, and
//!   never gets past the boot's BACKUP check. `USERNAME` is set, Mono's `GetVersionExW`
//!   answers 6.3 (9600).
//! - The LED board (837-15093, COM10) through the game's `SerialPortAPI.dll` (`serport_*`,
//!   its `DllImport`s answered by the payload: payload-common `led.rs` behind them).
//! - The CHC-C300 card printer (`C300usb.dll`, `C300FWDLusb.dll`): payload-common `chcusb.rs`.
//! - The title server (`kemono/server.rs`), in amdaemon's ALL.Net server.
//! - The two cameras (the card slot's, reading the printed cards' QR codes, and the player's):
//!   none needed. The machine's webcams are hidden, two are listed instead, the card camera
//!   sees the empty slot's code (the boot's camera check: else errors 3001 / 3002 / 3005) and
//!   the printed cards (`kemono/camera.rs`).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{chcusb, iat, late_hooks, log, mono, mono_imports, serial};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

type P = *mut c_void;

mod camera;
mod server;

/// `SerPortId.COM10`, the LED board's port.
const LED_PORT_ID: i32 = 9;

static ORIG_GET_VERSION_EX_W: AtomicUsize = AtomicUsize::new(0);
static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);
/// The LED board's fake handle (payload-common `serial`), its replies not read yet.
static LED_HANDLE: AtomicUsize = AtomicUsize::new(0);
static LED_RX: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

#[repr(C)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
}

/// Mono's `GetVersionExW`: Windows 8.1, a cabinet's.
unsafe extern "system" fn get_version_ex_w(info: *mut OsVersionInfoW) -> i32 {
    let orig: unsafe extern "system" fn(*mut OsVersionInfoW) -> i32 = unsafe { std::mem::transmute(ORIG_GET_VERSION_EX_W.load(Ordering::Relaxed)) };
    let r = unsafe { orig(info) };
    if r != 0 && !info.is_null() {
        unsafe {
            (*info).major = 6;
            (*info).minor = 3;
            (*info).build = 9600;
        }
    }
    r
}

/// `serport_Open(id, baud rate, size, parity, stop bits)`: the LED board's port only.
unsafe extern "system" fn serport_open(id: i32, _baud: i32, _csize: i32, _parity: i32, _stop: i32) -> u8 {
    if id != LED_PORT_ID {
        log!("kemono: serial port {} absent", id + 1);
        return 0;
    }
    if LED_HANDLE.load(Ordering::Relaxed) == 0 {
        type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, usize) -> usize;
        let Some(f) = serial::replacement("CreateFileA") else { return 0 };
        let create: CreateFileA = unsafe { std::mem::transmute(f) };
        let h = unsafe { create(c"\\\\.\\COM10".as_ptr().cast(), 0xC000_0000, 0, std::ptr::null_mut(), 3, 0, 0) };
        LED_HANDLE.store(h, Ordering::Relaxed);
    }
    log!("kemono: LED board port COM10 opened");
    1
}

unsafe extern "system" fn serport_close(_id: i32) -> u8 {
    1
}

/// `serport_Read(id, buffer, size)`: the board's replies queued, the count read (never waits).
unsafe extern "system" fn serport_read(id: i32, buf: *mut u8, size: i32) -> i32 {
    if id != LED_PORT_ID || buf.is_null() {
        return 0;
    }
    let mut rx = LED_RX.lock().unwrap();
    let n = rx.len().min(size.max(0) as usize);
    for i in 0..n {
        unsafe { *buf.add(i) = rx.pop_front().unwrap() };
    }
    n as i32
}

/// `serport_Write(id, buffer, size)`: the frames to the LED board, its replies queued.
unsafe extern "system" fn serport_write(id: i32, buf: *const u8, size: i32) -> i32 {
    if id != LED_PORT_ID || buf.is_null() || size <= 0 {
        return size;
    }
    let data = unsafe { std::slice::from_raw_parts(buf, size as usize) };
    if let Some(reply) = serial::transact(LED_HANDLE.load(Ordering::Relaxed) as *mut c_void, data) {
        LED_RX.lock().unwrap().extend(reply);
    }
    size
}

unsafe extern "system" fn serport_get_error(_id: i32) -> i32 {
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    match module {
        "serialportapi.dll" => Some(match name {
            "serport_Open" => serport_open as *const () as usize,
            "serport_Close" => serport_close as *const () as usize,
            "serport_Read" => serport_read as *const () as usize,
            "serport_Write" => serport_write as *const () as usize,
            "serport_GetError" => serport_get_error as *const () as usize,
            _ => return None,
        }),
        "c300usb.dll" | "c300fwdlusb.dll" => {
            let f = chcusb::resolve_c300(name);
            if f.is_none() && (name.starts_with("chcusb_") || name.starts_with("fwdlusb_")) {
                log!("kemono: printer {name} not emulated");
            }
            f
        }
        _ => None,
    }
}

/// UnityPlayer's `GetProcAddress` (it resolves Mono's exports): `mono_add_internal_call`
/// watched, the managed code patched once Mono is up.
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> P {
    let orig: unsafe extern "system" fn(P, *const u8) -> P = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if !f.is_null() && (name as usize) >> 16 != 0 && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
        return add_internal_call as *const () as P;
    }
    f
}

unsafe extern "C" fn add_internal_call(name: *const i8, method: P) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(patch_managed);
    }
    let orig: unsafe extern "C" fn(*const i8, P) = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    // the webcams the game lists: the payload's
    let method = if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() == b"UnityEngine.WebCamTexture::get_devices" {
        camera::webcam_devices as *const () as P
    } else {
        method
    };
    unsafe { orig(name, method) }
}

/// The cameras' methods replaced (once `Assembly-CSharp` is loaded, before the boot scene
/// compiles them).
fn patch_managed() {
    let start = Instant::now();
    let (m, image) = loop {
        if let Some(m) = mono::mono()
            && !m.domain().is_null()
            && m.attach()
        {
            let image = m.image("Assembly-CSharp");
            if !image.is_null() {
                break (m, image);
            }
        }
        if start.elapsed() > Duration::from_secs(120) {
            log!("kemono: Assembly-CSharp not loaded: the managed code unchanged");
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    // the printed cards kept, read in the card slot (`camera.rs`)
    let replaced = [
        ("QrCodeUtil.CalcCRC7", m.replace(m.method(m.class(image, "", "QrCodeUtil"), "CalcCRC7", 3), camera::calc_crc7 as *const () as usize)),
        ("QrCodeDecoder.IsCameraPlaying", m.replace(m.method(m.class(image, "", "QrCodeDecoder"), "get_IsCameraPlaying", 0), camera::is_camera_playing as *const () as usize)),
        ("QrCodeDecoder.SetDeviceNameFromPos", m.replace(m.method(m.class(image, "", "QrCodeDecoder"), "SetDeviceNameFromPos", 1), camera::set_device_name_from_pos as *const () as usize)),
    ];
    for (name, done) in replaced {
        if !done {
            log!("kemono: {name} not replaced: no card read");
        }
    }
}


/// The game's process: a cabinet's user and Windows, its LED board and printer.
pub(crate) fn init() {
    wal_payload_common::webcam::hide_all();
    camera::init();
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    match (unity != 0).then(|| unsafe { iat::hook_module(unity, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) }).flatten() {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("kemono: UnityPlayer.dll's GetProcAddress not hooked: the managed code unchanged"),
    }
    // SAFETY: at the payload's start, before the game's threads read the environment
    unsafe { std::env::set_var("USERNAME", "AppUser") };
    late_hooks::hook("kernel32.dll", "GetVersionExW", get_version_ex_w as *const () as usize, &ORIG_GET_VERSION_EX_W);
    wal_payload_common::led::init("COM10");
    chcusb::setup(
        "Parade_Data\\StreamingAssets\\Printer\\E0223100-014E-C300-MAINAPP.BIN",
        "Parade_Data\\StreamingAssets\\Printer\\D0460700-0101-C300-PARAM.BIN",
    );
    mono_imports::hook(resolve);
    log!("kemono: cabinet user, LED board on COM10 (SerialPortAPI), CHC-C300 printer");
}

/// amdaemon: the title server beside its ALL.Net server.
pub(crate) fn init_daemon() {
    server::init();
}
