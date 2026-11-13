//! The cabinet's DNS as Sega's network checks use it (segatools' `platform/dns.c`, public
//! domain): the store's router (`tenporouter.loc`, `bbrouter.loc`) and the ALL.Net hosts are the
//! virtual router ([`set_router`]; ALL.Net: the payload's server behind it, [`crate::allnet`]);
//! the mobile and DSL routers must not resolve (the check then takes the store router's line).
//! Answered at once, the machine's DNS not asked (its timeouts held the checks up, Wine's
//! `DnsQuery_A` never answered `.loc` names).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

static ROUTER: OnceLock<[u8; 4]> = OnceLock::new();

/// The virtual router's address (network order bytes): the address every name resolves to (a
/// loopback address is refused as ALL.Net's).
pub fn set_router(router: [u8; 4]) {
    let _ = ROUTER.set(router);
}

/// The virtual router's address ([`set_router`]).
pub fn router() -> [u8; 4] {
    ROUTER.get().copied().unwrap_or([127, 0, 0, 1])
}

pub fn dotted(a: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

/// A structure's field offset in the 32-bit and the 64-bit layouts.
const fn w(x86: usize, x64: usize) -> usize {
    if cfg!(target_pointer_width = "64") { x64 } else { x86 }
}

unsafe fn put<T>(base: *mut u8, offset: usize, value: T) {
    unsafe { (base.add(offset) as *mut T).write_unaligned(value) };
}

const DNS_ERROR_RCODE_NAME_ERROR: u32 = 9003;

/// The answer record (`DNS_RECORDA`, type A), never freed (`DnsFree` skips it).
struct Record([u8; 64]);
static RECORD: std::sync::Mutex<Record> = std::sync::Mutex::new(Record([0; 64]));
static HOST: &[u8] = b"localhost\0";

fn resolves(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && name != "mobirouter.loc" && name != "dslrouter.loc"
}

fn record(addr: [u8; 4]) -> usize {
    let mut r = RECORD.lock().unwrap();
    let p = r.0.as_mut_ptr();
    unsafe {
        put(p, w(4, 8), HOST.as_ptr() as usize);
        put(p, w(8, 16), 1u16); // DNS_TYPE_A
        put(p, w(10, 18), 4u16);
        put(p, w(12, 20), 1u32 | (3 << 3)); // answer section, ANSI names
        put(p, w(16, 24), 300u32);
        put(p, w(24, 32), addr);
    }
    p as usize
}

unsafe extern "system" fn dns_query_a(name: *const u8, kind: u16, _options: u32, _extra: usize, results: *mut usize, _reserved: usize) -> u32 {
    let name = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned() };
    let found = kind == 1 && resolves(&name);
    log!("dns: DnsQuery_A {name} type {kind}: {}", if found { dotted(router()) } else { "not found".into() });
    if !results.is_null() {
        unsafe { *results = if found { record(router()) } else { 0 } };
    }
    if found { 0 } else { DNS_ERROR_RCODE_NAME_ERROR }
}

unsafe extern "system" fn dns_query_ex(request: *const u8, result: *mut u8, _cancel: usize) -> u32 {
    let mut found = false;
    if !request.is_null() {
        let name = unsafe { (request.add(w(4, 8)) as *const *const u16).read_unaligned() };
        let len = if name.is_null() { 0 } else { (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count() };
        let name = if name.is_null() { String::new() } else { String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }) };
        found = resolves(&name);
        log!("dns: DnsQueryEx {name}: {found}");
    }
    let status = if found { 0 } else { DNS_ERROR_RCODE_NAME_ERROR };
    if !result.is_null() {
        // DNS_QUERY_RESULT: version, status, options, records
        unsafe {
            put(result, 4, status);
            put(result, 16, if found { record(router()) } else { 0 });
        }
    }
    status
}

static GETADDRINFO: AtomicUsize = AtomicUsize::new(0);

/// `getaddrinfo`: a name of the network (not an address, not `localhost`) resolved as
/// `DnsQuery` resolves it, the real function given its address.
unsafe extern "system" fn getaddrinfo(node: *const u8, service: *const u8, hints: *const std::ffi::c_void, result: *mut usize) -> i32 {
    type F = unsafe extern "system" fn(*const u8, *const u8, *const std::ffi::c_void, *mut usize) -> i32;
    let real: F = unsafe { std::mem::transmute(GETADDRINFO.load(Ordering::Relaxed)) };
    let name = if node.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(node.cast()) }.to_string_lossy().into_owned() };
    let numeric = name.parse::<std::net::Ipv4Addr>().is_ok() || name.contains(':');
    if name.is_empty() || numeric || name.eq_ignore_ascii_case("localhost") || !resolves(&name) {
        return unsafe { real(node, service, hints, result) };
    }
    let addr = std::ffi::CString::new(dotted(router())).unwrap();
    log!("dns: getaddrinfo {name}: {}", addr.to_string_lossy());
    unsafe { real(addr.as_ptr().cast(), service, hints, result) }
}

static GETHOSTBYNAME: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn WSASetLastError(error: i32);
}

/// `gethostbyname` (ws2_32 ordinal 52: Wonderland Wars' amLib checks the line type with it): a
/// name of the network resolved as `DnsQuery` resolves it, at once (the mobile and DSL routers
/// not found, not asked of the machine's DNS: its timeout held the network test up to 80 s).
unsafe extern "system" fn gethostbyname(name: *const u8) -> usize {
    type F = unsafe extern "system" fn(*const u8) -> usize;
    let real: F = unsafe { std::mem::transmute(GETHOSTBYNAME.load(Ordering::Relaxed)) };
    let host = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned() };
    // the machine's own name (no domain) asked of the machine
    let numeric = host.parse::<std::net::Ipv4Addr>().is_ok();
    if !host.contains('.') || numeric {
        return unsafe { real(name) };
    }
    if !resolves(&host) {
        log!("dns: gethostbyname {host}: not found");
        unsafe { WSASetLastError(11001) }; // WSAHOST_NOT_FOUND
        return 0;
    }
    let addr = std::ffi::CString::new(dotted(router())).unwrap();
    log!("dns: gethostbyname {host}: {}", addr.to_string_lossy());
    unsafe { real(addr.as_ptr().cast()) }
}

static DNS_FREE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn dns_free(data: usize, kind: u32) {
    if data == 0 || data == RECORD.lock().unwrap().0.as_ptr() as usize {
        return;
    }
    let orig: unsafe extern "system" fn(usize, u32) = unsafe { std::mem::transmute(DNS_FREE.load(Ordering::Relaxed)) };
    unsafe { orig(data, kind) }
}

/// `getaddrinfo` only (an ALLS game's process: ALL.Net's hosts, its connections redirected).
pub fn hook_getaddrinfo() {
    if let Some(orig) = unsafe { iat::hook("ws2_32.dll", "getaddrinfo", getaddrinfo as *const () as usize) } {
        GETADDRINFO.store(orig, Ordering::Relaxed);
    }
}

/// The game's lookups: `DnsQuery_A`, `DnsQueryEx`, `getaddrinfo`, `gethostbyname`, `DnsFree`.
pub fn init() {
    hook_getaddrinfo();
    unsafe {
        iat::hook("dnsapi.dll", "DnsQuery_A", dns_query_a as *const () as usize);
        iat::hook("dnsapi.dll", "DnsQueryEx", dns_query_ex as *const () as usize);
        let f = gethostbyname as *const () as usize;
        if let Some(orig) = iat::hook_ordinal(iat::game_module(), "WS2_32.dll", 52, f).or_else(|| iat::hook("ws2_32.dll", "gethostbyname", f)) {
            GETHOSTBYNAME.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("dnsapi.dll", "DnsFree", dns_free as *const () as usize) {
            DNS_FREE.store(orig, Ordering::Relaxed);
        }
    }
}
