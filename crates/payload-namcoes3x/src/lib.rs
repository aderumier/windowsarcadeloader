//! 32-bit Namco ES3 support, installed as the game's Banapass reader (`bngrw.dll`).
//! Mario Kart GP DX reads its wheel and buttons through DirectInput, so the same module also
//! supplies the launcher's virtual wheel and reports the game's memory based force feedback.

#![allow(non_snake_case)]

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_protocol::{button, output};

mod jvs;
mod ipc;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

static CARD_CONSUMED: AtomicBool = AtomicBool::new(false);
static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

type AttachCallback = unsafe extern "C" fn(i32, i32, *mut isize) -> isize;
type ResetCallback = unsafe extern "C" fn(i32, i32, *mut isize) -> isize;
type TouchCallback = unsafe extern "C" fn(i32, i32, *mut c_void, *mut c_void);

fn game_base() -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MkdxVersion { V11022, V11816 }

impl MkdxVersion {
    fn name(self) -> &'static str {
        match self { Self::V11022 => "v1.10.22", Self::V11816 => "v1.18.16" }
    }
}

/// PE build identity is checked before any version-specific code or data address is used.
pub(crate) fn game_version() -> Option<MkdxVersion> {
    static VERSION: std::sync::OnceLock<Option<MkdxVersion>> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        let base = game_base();
        if safe_read::<u16>(base)? != 0x5a4d { return None; }
        let pe = safe_read::<u32>(base + 0x3c)? as usize;
        if pe > 0x1000 || safe_read::<u32>(base + pe)? != 0x4550 { return None; }
        match (safe_read::<u32>(base + pe + 8)?, safe_read::<u32>(base + pe + 24 + 56)?) {
            (0x55a38b64, 0xaab000) => Some(MkdxVersion::V11022),
            (0x5fb222aa, 0xb37000) => Some(MkdxVersion::V11816),
            _ => None,
        }
    })
}

fn thread_input() {
    wal_payload_common::start("namcoes3-mariokart");
    wal_payload_common::log!("namcoes3: executable build {:?}", game_version());
    unsafe { patch_mkdx_dongle(); }
    unsafe { patch_mkdx_content_router(); }
    wal_payload_common::dinput::init();
    jvs::init(&[game_base()]);
    unsafe {
        if let Some(original) = wal_payload_common::iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) {
            CO_CREATE_INSTANCE.store(original, Ordering::Relaxed);
            wal_payload_common::log!("namcoes3: hooked game IPC COM activation");
        }
    }
    std::thread::spawn(force_feedback_loop);
}

/// MKDX checks for a physical Namco Content Router during boot and
/// raises E05-01 when discovery and link checks fail. UltraLoader's
/// disassembly and registrar trace identify the error branches. Redirect the
/// discovery branches to the game's success path, then skip the two separately
/// traced link-up raises. This only skips the cabinet router presence check;
/// it does not implement online services.
unsafe fn patch_mkdx_content_router() {
    let Some(version) = game_version() else { return; };
    let base = game_base();
    if base == 0 { return; }
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11022 => &[
        // 0x006E0F61: je 0x006E10B7 (null network object -> E05-01)
        //             -> je 0x006E10D7 (router found success path)
        (0x2e0f61, &[0x0f, 0x84, 0x50, 0x01, 0x00, 0x00],
            &[0x0f, 0x84, 0x70, 0x01, 0x00, 0x00], "null-object"),
        // 0x006E0FC3: jge 0x006E10AD (10 retries -> E05-01)
        //             -> jge 0x006E10D7 (router found success path)
        (0x2e0fc3, &[0x0f, 0x8d, 0xe4, 0x00, 0x00, 0x00],
            &[0x0f, 0x8d, 0x0e, 0x01, 0x00, 0x00], "retry-limit"),
        // Two additional router link-up raises discovered by UltraLoader's
        // registrar trace (both push error id 0x10, also rendered E05-01).
        // Keep the comparison and jump target, but make the jump unconditional.
        (0x2e1200, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-a"),
        (0x2e1861, &[0x0f, 0x85, 0x16, 0x01, 0x00, 0x00],
            &[0xe9, 0x17, 0x01, 0x00, 0x00, 0x90], "router-link-b"),
        ],
        MkdxVersion::V11816 => &[
            // 0x71BEAC: null router object -> success at 0x71C02E.
            (0x31beac, &[0x0f, 0x84, 0x5c, 0x01, 0x00, 0x00],
                &[0x0f, 0x84, 0x7c, 0x01, 0x00, 0x00], "null-object"),
            // 0x71BF2A: retry limit -> the same success path.
            (0x31bf2a, &[0x0f, 0x8d, 0xd4, 0x00, 0x00, 0x00],
                &[0x0f, 0x8d, 0xfe, 0x00, 0x00, 0x00], "retry-limit"),
            // The newer build has three link-up raises of error id 0x10
            // (E05-01), rather than the two in v1.10.22. Skip each raise.
            (0x31c160, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-a"),
            (0x31cc50, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-b"),
            (0x31cde2, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-c"),
            // The wired Batocera network can make the router object appear,
            // then fail its identity/certification checks. Those paths raise
            // E05-02 through the same registrar. The router service has eight
            // direct registrar calls; replace each call with `xor eax,eax;`
            // `add esp,4`, the registrar's false return and stdcall argument
            // cleanup. These call sites are all inside the router subsystem,
            // so other cabinet/device errors still reach the normal handler.
            (0x31c018, &[0xe8, 0xa3, 0xd5, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-null"),
            (0x31c16c, &[0xe8, 0x4f, 0xd4, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-link-a"),
            (0x31cb58, &[0xe8, 0x63, 0xca, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-begin"),
            (0x31cc5c, &[0xe8, 0x5f, 0xc9, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-match"),
            (0x31ccfa, &[0xe8, 0xc1, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-missing"),
            (0x31cd80, &[0xe8, 0x3b, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-version"),
            (0x31cda1, &[0xe8, 0x1a, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-state"),
            (0x31cdee, &[0xe8, 0xcd, 0xc7, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-final"),
        ],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: Content Router {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: Content Router {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: skipped {} offline Content Router check ({name})", version.name());
    }
}

/// Version-specific dongle compatibility patches, with PE identity and original bytes
/// checked before modification. v1.18.16's persistence helper takes one stack argument;
/// its replacement must retain `ret 4`, unlike v1.10.22's `ret`.
unsafe fn patch_mkdx_dongle() {
    let base = game_base();
    if base == 0 { return; }
    let Some(version) = game_version() else {
        wal_payload_common::log!("namcoes3: unknown executable; dongle patches skipped");
        return;
    };
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11022 => &[
        (0x1efcc0, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "poll"),
        (0x1ef669,
            &[0xe8, 0x42, 0x07, 0x00, 0x00, 0x8a, 0x97, 0xe0, 0x0c, 0x00, 0x00],
            &[0x90, 0x90, 0x90, 0x90, 0x90, 0xb2, 0x01, 0x90, 0x90, 0x90, 0x90], "init"),
        (0x1ef7d0, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "update"),
        ],
        MkdxVersion::V11816 => &[
            (0x208130, &[0x55, 0x8b, 0xec, 0x6a, 0xff], &[0xb0, 0x01, 0xc2, 0x04, 0x00], "poll"),
            (0x207ae9,
                &[0xe8, 0x32, 0x07, 0x00, 0x00, 0x8a, 0x97, 0xe0, 0x0c, 0x00, 0x00],
                &[0x90, 0x90, 0x90, 0x90, 0x90, 0xb2, 0x01, 0x90, 0x90, 0x90, 0x90], "init"),
            (0x207c50, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "update"),
        ],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: dongle {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: dongle {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: applied {} dongle {name} patch", version.name());
    }
}

type CoCreateInstance = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
const IPC_CLSID: [u8; 16] = [0x2a,0x4b,0xc4,0xcc,0x9a,0x70,0xa8,0x33,0xa1,0xb0,0x5d,0xbe,0x25,0xb1,0x9c,0x2c];

unsafe extern "system" fn co_create_instance(
    clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void,
) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == IPC_CLSID {
        if !out.is_null() { unsafe { *out = (&ipc::OBJECT as *const ipc::Object).cast_mut().cast(); } }
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| wal_payload_common::log!("namcoes3: supplied offline IPC COM adapter"));
        return 0;
    }
    let original = CO_CREATE_INSTANCE.load(Ordering::Relaxed);
    if original == 0 { return 0x80004005u32 as i32; }
    let real: CoCreateInstance = unsafe { std::mem::transmute(original) };
    unsafe { real(clsid, outer, context, iid, out) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        thread_input();
        wal_payload_common::log!("namcoes3: 32-bit Banapass and DirectInput adapter loaded");
    }
    TRUE
}

fn safe_read<T: Copy + Default>(address: usize) -> Option<T> {
    if address < 0x10000 { return None; }
    let mut value = T::default();
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(), address as *const c_void, (&mut value as *mut T).cast(),
            std::mem::size_of::<T>(), std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(value)
}

fn force_feedback_loop() {
    let base = game_base();
    // These force-feedback offsets belong to v1.18.16, not v1.10.22.
    if game_version() != Some(MkdxVersion::V11816) { return; }
    let spring = std::env::var("WAL_NAMCOES3_FFB_SPRING").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(0, 10000);
    let mut last = [i32::MIN; 2];
    loop {
        let force = safe_read::<u32>(base + 0xAAEEA0)
            .and_then(|ptr| safe_read::<u32>(ptr as usize + 8))
            .and_then(|ptr| safe_read::<f32>(ptr as usize + 0xBC8))
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0);
        let values = [(force * output::FFB_MAX as f32) as i32, (force.abs() * output::FFB_MAX as f32) as i32];
        for (i, id) in [output::FFB_CONSTANT, output::FFB_VIBRATION].into_iter().enumerate() {
            if last[i] != values[i] {
                wal_payload_common::send_output(0, id, values[i]);
                last[i] = values[i];
            }
        }
        wal_payload_common::send_output(0, output::FFB_SPRING, spring);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn card_data() -> [u8; 168] {
    let mut data = [0u8; 168];
    data[0..8].copy_from_slice(&[1, 1, 0, 0, 4, 0, 0, 0]);
    data[44..76].copy_from_slice(b"7F5C9744F111111143262C3300040610");
    data[80..100].copy_from_slice(b"30764352518498791337");
    if let Ok(id) = std::env::var("WAL_NAMCOES3_CARD_ID") {
        let id = id.as_bytes();
        data[44..76].fill(0);
        let len = id.len().min(31);
        data[44..44 + len].copy_from_slice(&id[..len]);
    }
    if let Ok(access) = std::env::var("WAL_NAMCOES3_ACCESS_CODE") {
        let access = access.as_bytes();
        data[80..100].fill(0);
        let len = access.len().min(19);
        data[80..80 + len].copy_from_slice(&access[..len]);
    }
    data
}

/// Game asks asynchronously for a card tap. The launcher's `card` control inserts a card;
/// the callback is completed once for each insertion.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn BngRwReqWaitTouch(
    _reader: u32, _timeout: i32, _flags: u32, callback: Option<TouchCallback>, context: *mut c_void,
) -> i32 {
    let Some(callback) = callback else { return -1 };
    let context = context as usize;
    std::thread::spawn(move || loop {
        let inserted = wal_payload_common::input(0).pressed(button::CARD);
        if !inserted {
            CARD_CONSUMED.store(false, Ordering::Release);
        } else if !CARD_CONSUMED.swap(true, Ordering::AcqRel) {
            let mut data = card_data();
            unsafe { callback(0, 0, data.as_mut_ptr().cast(), context as *mut c_void) };
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    });
    -1
}

macro_rules! export {
    ($rust:ident, $name:literal, ($($arg:ident : $ty:ty),*) -> $ret:ty, $body:expr) => {
        #[unsafe(export_name = $name)]
        pub extern "system" fn $rust($($arg: $ty),*) -> $ret { $body }
    };
}

export!(bng_init, "BngRwInit", () -> i32, 0);
export!(bng_fin, "BngRwFin", () -> i32, 0);
export!(bng_version, "BngRwGetVersion", () -> u32, 0x0001_0000);
export!(bng_reset, "BngRwDevReset", (_reader: u32, callback: Option<ResetCallback>, context: *mut isize) -> i32, {
    if let Some(callback) = callback { let context = context as usize; std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); unsafe { callback(0, 0, context as *mut isize); } }); }
    1
});
export!(bng_attach, "BngRwAttach", (_reader: u32, _name: *const c_char, _a: i32, _b: i32, callback: Option<AttachCallback>, context: *mut isize) -> u64, {
    if let Some(callback) = callback { let context = context as usize; std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); unsafe { callback(0, 0, context as *mut isize); } }); }
    1
});
export!(bng_station, "BngRwGetStationID", (_out: *mut u32) -> i32, 1);
export!(bng_fw, "BngRwGetFwVersion", (_out: *mut u32) -> i32, 1);
export!(bng_retry, "BngRwGetTotalRetryCount", (_out: *mut u32) -> i32, 0);
export!(bng_cmd, "BngRwIsCmdExec", (_cmd: u32) -> i32, 0);
export!(bng_mifare, "BngRwExReadMifareAllBlock", (_a: u32, _b: *mut c_void) -> i32, 1);
export!(bng_action, "BngRwReqAction", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_auth, "BngRwReqAiccAuth", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32) -> i32, 1);
export!(bng_beep, "BngRwReqBeep", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_cancel, "BngRwReqCancel", (_a: u32) -> i32, 1);
export!(bng_cleanup, "BngRwReqFwCleanup", (_a: u32, _b: u32, _c: u32) -> i32, 1);
export!(bng_fw_update, "BngRwReqFwVersionup", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_latch, "BngRwReqLatchID", (_a: u32, _b: u32, _c: u32) -> i32, 1);
export!(bng_led, "BngRwReqLed", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_mail, "BngRwReqSendMailTo", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32, _i: u32, _j: u32) -> i32, 1);
export!(bng_url, "BngRwReqSendUrlTo", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32) -> i32, 1);
export!(bng_power, "BngRwSetLedPower", (_a: u32, _b: u32, _c: u32) -> i32, 1);
