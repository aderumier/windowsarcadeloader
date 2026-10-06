//! The TsuNet cabinet COM object: register the dump's real `tsunet.dll` with Wine.
//!
//! After the TsuInput init, the game does
//! `CoCreateInstance(CLSID {9F17431A-FFA3-42A5-A8EB-F9033D3403AA}, ..., IID_IUnknown)` and QIs the
//! result for `{360FE1FA-D991-4032-A65D-01500E07F05E}` (cabinet network/communication, on real
//! cabinets the in-proc server `Tsunami/tsunet.dll`, "TsuNet Class"). The game *tolerates* the
//! creation failure (it logs it) but then calls `vtable[0x30]` on the NULL interface and aborts
//! in a hidden MSVCRT "Runtime Error!" dialog — the white screen.
//!
//! The dump ships the real server, so instead of emulating it we register it: the profile copies
//! `tsunet.dll` next to the exe (`files:`) and this writes the CLSID's `InprocServer32` key at
//! load time; Wine's COM then loads it and provides the real vtable.
//!
//! The DLL's init walks the `GetAdaptersInfo` result looking for an Ethernet adapter, following
//! `Next` pointers with no bounds check. Wine's `GetAdaptersInfo` reports a required size
//! smaller than the data it writes (on a host with several interfaces), so the walk runs past
//! the buffer into adjacent heap and spins forever, stalling the game before its loop. The
//! network is unused in single-cabinet mode: `patch_dll` makes it take the first adapter and
//! stop (see the byte patterns).

use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameA;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegSetValueExW,
};

use crate::log;

const CLSID: &str = "{9F17431A-FFA3-42A5-A8EB-F9033D3403AA}";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set(key: HKEY, name: *const u16, value: &str) -> i32 {
    let value = wide(value);
    unsafe { RegSetValueExW(key, name, 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32) as i32 }
}

/// Stops the adapter walk of the run-dir copy (file offsets; ImageBase 0x10000000, .text file
/// offset == RVA): `0x5d3b` takes any adapter (was: Ethernet only), `0x5d5c` stops after the
/// first one (was: walk the whole list).
fn patch_dll(path: &str) {
    const PATCHES: [(usize, &[u8], &[u8]); 2] =
        [(0x5d3b, b"\x83\xbe\x90\x01\x00\x00\x06", b"\xeb\x07\x90\x90\x90\x90\x90"), (0x5d5c, b"\x75\xdd", b"\xeb\x02")];
    let Ok(mut data) = std::fs::read(path) else {
        log!("tsunet: cannot read {path}");
        return;
    };
    for (off, from, to) in PATCHES {
        let cur = &data[off..off + from.len()];
        if cur == to {
            continue; // already patched
        }
        if cur != from {
            log!("tsunet: unexpected bytes at +{off:x}, not patched");
            return;
        }
        data[off..off + to.len()].copy_from_slice(to);
    }
    if let Err(e) = std::fs::write(path, &data) {
        log!("tsunet: cannot write {path}: {e}");
    } else {
        log!("tsunet: patched the adapter walk ({path})");
    }
}

/// Registers `clsid` as an in-proc server provided by `dll` (full path), what a COM server's
/// `DllRegisterServer` writes.
pub fn register_inproc(clsid: &str, class_name: &str, dll: &str) {
    let base = wide(&format!("Software\\Classes\\CLSID\\{clsid}"));
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, base.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("tsunet: cannot create the {clsid} key: error {r}");
        return;
    }
    let r = set(key, std::ptr::null(), class_name);
    if r != 0 {
        log!("tsunet: setting the {clsid} class name failed: error {r}");
    }
    unsafe { RegCloseKey(key) };

    let inproc = wide(&format!("Software\\Classes\\CLSID\\{clsid}\\InprocServer32"));
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, inproc.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("tsunet: cannot create {clsid} InprocServer32: error {r}");
        return;
    }
    let r1 = set(key, std::ptr::null(), dll);
    let model = wide("ThreadingModel");
    let r2 = set(key, model.as_ptr(), "Both");
    unsafe { RegCloseKey(key) };
    log!("tsunet: registered CLSID {clsid} -> {dll} ({}{})", if r1 == 0 { "ok" } else { "path error" }, if r2 == 0 { "" } else { ", threading error" });
}

/// `tsunet.dll` sits next to the game executable (the profile's `files:` copies it there).
pub fn init() {
    let mut buf = [0u8; 260];
    let n = unsafe { GetModuleFileNameA(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        log!("tsunet: cannot get the game path");
        return;
    }
    let path = String::from_utf8_lossy(&buf[..n]).into_owned();
    let dir = match path.rfind('\\') {
        Some(i) => &path[..i],
        None => return,
    };
    let dll = format!("{dir}\\tsunet.dll");
    patch_dll(&dll);
    register_inproc(CLSID, "TsuNet Class", &dll);
}
