//! The cabinet network setup: gs-system gives the cabinet its fixed address (192.168.1.x, from
//! its settings) through iphlpapi's undocumented `SetAdapterIpAddress`, looked up with
//! `GetProcAddress`. Wine only has a stub that aborts the process: answered as done, the
//! host's network left as it is.
//!
//! Winning Eleven 2008 reads the adapters into a fixed buffer: `GetAdaptersInfo` gives it those
//! that fit.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
use windows_sys::Win32::NetworkManagement::IpHelper::{IP_ADAPTER_INFO, IP_ADDR_STRING};
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

static ORIG_GET_ADAPTERS_INFO: AtomicUsize = AtomicUsize::new(0);

type GetAdaptersInfo = unsafe extern "system" fn(*mut IP_ADAPTER_INFO, *mut u32) -> u32;

/// `GetAdaptersInfo` into the caller's buffer, as many adapters as fit, those with an address
/// first. Winning Eleven 2008 passes a fixed 2 KB buffer (3 adapters) and fails its network
/// setup on ERROR_BUFFER_OVERFLOW: a PC has more (bridges, VPNs, containers), a cabinet one.
unsafe extern "system" fn get_adapters_info(out: *mut IP_ADAPTER_INFO, size: *mut u32) -> u32 {
    let original: GetAdaptersInfo = unsafe { std::mem::transmute(ORIG_GET_ADAPTERS_INFO.load(Ordering::Relaxed)) };
    if out.is_null() || size.is_null() {
        return unsafe { original(out, size) };
    }
    let mut all_size = 0u32;
    unsafe { original(std::ptr::null_mut(), &mut all_size) };
    let count = (all_size as usize).div_ceil(size_of::<IP_ADAPTER_INFO>()).max(1);
    let mut all: Vec<IP_ADAPTER_INFO> = vec![unsafe { std::mem::zeroed() }; count];
    all_size = (count * size_of::<IP_ADAPTER_INFO>()) as u32;
    let ret = unsafe { original(all.as_mut_ptr(), &mut all_size) };
    if ret != 0 {
        return ret;
    }
    let mut list = Vec::new();
    let mut at: *const IP_ADAPTER_INFO = all.as_ptr();
    while let Some(a) = unsafe { at.as_ref() } {
        list.push(*a);
        at = a.Next;
    }
    // adapters without an address (0.0.0.0) last
    list.sort_by_key(|a| a.IpAddressList.IpAddress.String[0] == b'0' as _);
    let fit = (unsafe { *size } as usize / size_of::<IP_ADAPTER_INFO>()).min(list.len());
    if fit == 0 {
        unsafe { *size = size_of::<IP_ADAPTER_INFO>() as u32 };
        return ERROR_BUFFER_OVERFLOW;
    }
    for (i, a) in list.iter().take(fit).enumerate() {
        let mut a = *a;
        // the pointers inside an entry are its own lists: rebased to the copy
        let rebase = |p: *mut IP_ADDR_STRING, from: *const IP_ADAPTER_INFO, to: *mut IP_ADAPTER_INFO| -> *mut IP_ADDR_STRING {
            let (f, t) = (from as usize, to as usize);
            if (p as usize) >= f && (p as usize) < f + size_of::<IP_ADAPTER_INFO>() {
                (p as usize - f + t) as *mut IP_ADDR_STRING
            } else {
                p
            }
        };
        let from = list.as_ptr().wrapping_add(i);
        let to = unsafe { out.add(i) };
        a.Next = if i + 1 < fit { unsafe { out.add(i + 1) } } else { std::ptr::null_mut() };
        a.IpAddressList.Next = rebase(a.IpAddressList.Next, from, to);
        a.GatewayList.Next = rebase(a.GatewayList.Next, from, to);
        a.DhcpServer.Next = rebase(a.DhcpServer.Next, from, to);
        a.CurrentIpAddress = rebase(a.CurrentIpAddress, from, to);
        unsafe { to.write(a) };
    }
    if fit < list.len() {
        log!("net: {} of {} network adapters reported (the game's buffer)", fit, list.len());
    }
    0
}

/// Fits `GetAdaptersInfo` into the executable's buffer (Winning Eleven 2008).
pub fn adapters_fit() {
    let ours = get_adapters_info as *const () as usize;
    match unsafe { iat::hook("iphlpapi.dll", "GetAdaptersInfo", ours) } {
        Some(o) => ORIG_GET_ADAPTERS_INFO.store(o, Ordering::Relaxed),
        None => log!("net: GetAdaptersInfo import not found"),
    }
}
