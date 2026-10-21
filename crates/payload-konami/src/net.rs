//! The cabinet network setup: gs-system gives the cabinet its fixed address (192.168.1.x, from
//! its settings) through iphlpapi's undocumented `SetAdapterIpAddress`, looked up with
//! `GetProcAddress`. Wine only has a stub that aborts the process: answered as done, the
//! host's network left as it is.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static ORIG_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

type GetProcAddress = unsafe extern "system" fn(usize, *const u8) -> usize;

/// `SetAdapterIpAddress(adapter, dhcp, address, mask, gateway)`: NO_ERROR.
unsafe extern "system" fn set_adapter_ip_address(_adapter: *const u8, dhcp: u32, address: u32, _mask: u32, _gateway: u32) -> u32 {
    let a = address.to_le_bytes();
    log!("net: cabinet address {}.{}.{}.{} (dhcp {dhcp}) not applied", a[0], a[1], a[2], a[3]);
    0
}

unsafe extern "system" fn get_proc_address(module: usize, name: *const u8) -> usize {
    // names, not ordinals (a pointer above the 64 KB ordinal range)
    if name as usize > 0xffff
        && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"SetAdapterIpAddress"
    {
        return set_adapter_ip_address as *const () as usize;
    }
    let original: GetProcAddress = unsafe { std::mem::transmute(ORIG_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    unsafe { original(module, name) }
}

/// Hooks gs-system's own `GetProcAddress` import (the module that looks the function up).
pub fn init() {
    let name: Vec<u16> = "gs-system.dll".encode_utf16().chain(std::iter::once(0)).collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) } as usize;
    if module == 0 {
        log!("net: gs-system.dll not loaded, cabinet address setup not answered");
        return;
    }
    let ours = get_proc_address as *const () as usize;
    if let Some(o) = unsafe { iat::hook_module(module, "kernel32.dll", "GetProcAddress", ours) }.filter(|&o| o != ours) {
        ORIG_GET_PROC_ADDRESS.store(o, Ordering::Relaxed);
    }
}
