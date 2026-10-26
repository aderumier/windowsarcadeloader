//! Wangan Midnight Maximum Tune 5's network interface (`sys::net::Interface`): the IP Helper
//! calls of the game, logged (`WAL_NAMCOES3_NET_LOG=1`).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

static GET_IF_ENTRY: AtomicUsize = AtomicUsize::new(0);
static GET_IP_ADDR_TABLE: AtomicUsize = AtomicUsize::new(0);
static GET_INTERFACE_INFO: AtomicUsize = AtomicUsize::new(0);
static IP_RENEW_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static WSA_RECV_FROM: AtomicUsize = AtomicUsize::new(0);
static WSA_SEND_TO: AtomicUsize = AtomicUsize::new(0);
static LOGGED: AtomicUsize = AtomicUsize::new(0);

fn logging() -> bool {
    LOGGED.fetch_add(1, Ordering::Relaxed) < 200
}

/// MIB_IFROW: wszName[256], dwIndex 0x200, dwType 0x204, ... dwOperStatus 0x220.
unsafe extern "system" fn get_if_entry(row: *mut u8) -> u32 {
    let orig: unsafe extern "system" fn(*mut u8) -> u32 = unsafe { std::mem::transmute(GET_IF_ENTRY.load(Ordering::Relaxed)) };
    let r = unsafe { orig(row) };
    // logged on change
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let read = |off: usize| unsafe { *(row.add(off) as *const u32) };
    let state = (r as usize) << 32 | (read(0x200) as usize) << 8 | read(0x220) as usize;
    if LAST.swap(state, Ordering::Relaxed) != state {
        log!("net: GetIfEntry index {} -> {r} type {} admin {} oper {}", read(0x200), read(0x204), read(0x21c), read(0x220));
    }
    r
}

/// MIB_IPADDRTABLE: dwNumEntries, then MIB_IPADDRROW (addr, index, mask, bcast, reasm, type).
unsafe extern "system" fn get_ip_addr_table(table: *mut u8, size: *mut u32, order: i32) -> u32 {
    let orig: unsafe extern "system" fn(*mut u8, *mut u32, i32) -> u32 = unsafe { std::mem::transmute(GET_IP_ADDR_TABLE.load(Ordering::Relaxed)) };
    let r = unsafe { orig(table, size, order) };
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let n = if r == 0 && !table.is_null() { (unsafe { *(table as *const u32) }) as usize } else { usize::MAX - 1 };
    if LAST.swap(n, Ordering::Relaxed) != n && r == 0 && !table.is_null() {
        let n = unsafe { *(table as *const u32) };
        for i in 0..n as usize {
            let row = unsafe { table.add(4 + i * 24) as *const u32 };
            let (addr, index) = unsafe { (*row, *row.add(1)) };
            log!("net: GetIpAddrTable {} index {index}", std::net::Ipv4Addr::from(addr.to_be()));
        }
    }
    r
}

unsafe extern "system" fn get_interface_info(info: *mut u8, size: *mut u32) -> u32 {
    let orig: unsafe extern "system" fn(*mut u8, *mut u32) -> u32 = unsafe { std::mem::transmute(GET_INTERFACE_INFO.load(Ordering::Relaxed)) };
    let r = unsafe { orig(info, size) };
    if logging() && r == 0 && !info.is_null() {
        // IP_INTERFACE_INFO: NumAdapters, then IP_ADAPTER_INDEX_MAP (Index, Name[128])
        let n = unsafe { *(info as *const i32) };
        for i in 0..n.max(0) as usize {
            let index = unsafe { *(info.add(4 + i * 260) as *const u32) };
            log!("net: GetInterfaceInfo adapter index {index}");
        }
    } else if logging() {
        log!("net: GetInterfaceInfo -> {r}");
    }
    r
}

unsafe extern "system" fn ip_renew_address(map: *mut u8) -> u32 {
    let orig: unsafe extern "system" fn(*mut u8) -> u32 = unsafe { std::mem::transmute(IP_RENEW_ADDRESS.load(Ordering::Relaxed)) };
    let r = unsafe { orig(map) };
    if logging() {
        log!("net: IpRenewAddress -> {r}");
    }
    r
}

/// WSABUF: len (u32), buf.
#[repr(C)]
struct WsaBuf {
    len: u32,
    buf: *mut u8,
}

unsafe extern "system" fn wsa_recv_from(
    s: usize, bufs: *mut WsaBuf, count: u32, received: *mut u32, flags: *mut u32, from: *mut u8, fromlen: *mut i32, ov: *mut u8, cr: usize,
) -> i32 {
    type F = unsafe extern "system" fn(usize, *mut WsaBuf, u32, *mut u32, *mut u32, *mut u8, *mut i32, *mut u8, usize) -> i32;
    let orig: F = unsafe { std::mem::transmute(WSA_RECV_FROM.load(Ordering::Relaxed)) };
    let r = unsafe { orig(s, bufs, count, received, flags, from, fromlen, ov, cr) };
    if logging() {
        unsafe extern "system" {
            fn WSAGetLastError() -> i32;
        }
        let err = if r != 0 { unsafe { WSAGetLastError() } } else { 0 };
        let n = if r == 0 && !received.is_null() { unsafe { *received } } else { 0 };
        let head: Vec<u8> = if n > 0 { unsafe { std::slice::from_raw_parts((*bufs).buf, n.min(8) as usize) }.to_vec() } else { Vec::new() };
        log!("net: WSARecvFrom socket {s:#x} -> {r} error {err} bytes {n} {head:02X?}");
    }
    r
}

/// The cabinet's store link multicasts to 225.0.0.1:50765 (terminal, other cabinets) and
/// hears its own packets back: sent to the loopback instead, to the cabinet's own socket (the
/// emulated terminal sends there too), nothing on the network.
unsafe extern "system" fn wsa_send_to(
    s: usize, bufs: *mut WsaBuf, count: u32, sent: *mut u32, flags: u32, to: *const u8, tolen: i32, ov: *mut u8, cr: usize,
) -> i32 {
    type F = unsafe extern "system" fn(usize, *mut WsaBuf, u32, *mut u32, u32, *const u8, i32, *mut u8, usize) -> i32;
    let orig: F = unsafe { std::mem::transmute(WSA_SEND_TO.load(Ordering::Relaxed)) };
    // sockaddr_in: family, port (big endian), address
    let mut local = [0u8; 16];
    let mut dest = to;
    if !to.is_null() && tolen >= 8 {
        let addr = unsafe { std::slice::from_raw_parts(to, 8) };
        let link = addr[0..2] == 2u16.to_le_bytes() && addr[2..4] == 50765u16.to_be_bytes() && addr[4..8] == [225, 0, 0, 1];
        if link {
            local[..4].copy_from_slice(&addr[..4]);
            local[4..8].copy_from_slice(&[127, 0, 0, 1]);
            dest = local.as_ptr();
        }
        if logging() {
            log!("net: WSASendTo {}.{}.{}.{}:{} {}", addr[4], addr[5], addr[6], addr[7], u16::from_be_bytes([addr[2], addr[3]]), if link { "-> loopback" } else { "" });
        }
    }
    unsafe { orig(s, bufs, count, sent, flags, dest, tolen, ov, cr) }
}

pub(crate) fn init(base: usize) {
    match unsafe { iat::hook_module(base, "ws2_32.dll", "WSASendTo", wsa_send_to as *const () as usize) } {
        Some(o) => WSA_SEND_TO.store(o, Ordering::Relaxed),
        None => log!("net: WSASendTo not hooked"),
    }
    if std::env::var("WAL_NAMCOES3_NET_LOG").is_ok_and(|v| v == "1") {
        trace_errors(base);
        match unsafe { iat::hook_module(base, "ws2_32.dll", "WSARecvFrom", wsa_recv_from as *const () as usize) } {
            Some(o) => WSA_RECV_FROM.store(o, Ordering::Relaxed),
            None => log!("net: WSARecvFrom not hooked"),
        }
    }
    for (name, f, slot) in [
        ("GetIfEntry", get_if_entry as *const () as usize, &GET_IF_ENTRY),
        ("GetIpAddrTable", get_ip_addr_table as *const () as usize, &GET_IP_ADDR_TABLE),
        ("GetInterfaceInfo", get_interface_info as *const () as usize, &GET_INTERFACE_INFO),
        ("IpRenewAddress", ip_renew_address as *const () as usize, &IP_RENEW_ADDRESS),
    ] {
        match unsafe { iat::hook_module(base, "iphlpapi.dll", name, f) } {
            Some(o) => slot.store(o, Ordering::Relaxed),
            None => log!("net: {name} not hooked"),
        }
    }
}

/// The game's network error_code constructor (0x1eba10: code, the network category), logged
/// with its caller (`WAL_NAMCOES3_NET_LOG=1`): which check raises an error.
unsafe extern "system" fn network_error(out: *mut usize, code: u32) -> *mut usize {
    let base = crate::game_base();
    // the category's static (guard bit, then its vtable) as the game initializes it
    unsafe {
        *((base + 0x19231b0) as *mut u32) |= 1;
        *((base + 0x19231a8) as *mut usize) = base + 0xd2a860;
        *out = code as usize;
        *out.add(1) = base + 0x19231a8;
    }
    if logging() {
        let mut frames = [std::ptr::null_mut(); 3];
        unsafe extern "system" {
            fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut std::ffi::c_void, hash: *mut u32) -> u16;
        }
        unsafe { RtlCaptureStackBackTrace(1, 3, frames.as_mut_ptr(), std::ptr::null_mut()) };
        let at: Vec<String> = frames.iter().map(|f| format!("{:#x}", (*f as usize).wrapping_sub(base))).collect();
        log!("net: error {code} ({code:#x}) from {}", at.join(" < "));
    }
    out
}

pub(crate) fn trace_errors(base: usize) {
    let mut jump = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
    jump[2..10].copy_from_slice(&(network_error as *const () as usize as u64).to_le_bytes());
    crate::patches::patch("network error trace", base, 0x1eba10, b"\x48\x89\x5c\x24\x08\x57\x48\x83\xec\x30\x8b\x05", &jump);
}
