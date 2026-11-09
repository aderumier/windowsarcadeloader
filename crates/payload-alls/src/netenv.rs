//! The cabinet's network as amdaemon checks it (segatools' `platform/netenv.c`, public
//! domain): one Ethernet adapter in the keychip's subnet (`WAL_ALLS_SUBNET`, the cabinet at
//! `WAL_ALLS_HOST`, default .205, the router and DNS server at `WAL_ALLS_ROUTER`, default .1),
//! every ping answered. Its iphlpapi imports are answered with that adapter (the game's too
//! with `WAL_ALLS_GAME_ADAPTER=1`); the real network is not used for it.
//!
//! The structures are written by offset (64-bit layouts of iphlpapi's
//! `IP_ADAPTER_ADDRESSES_LH`, `IP_ADAPTER_INFO`, `MIB_IFROW`, `MIB_IPFORWARDROW`,
//! `ICMP_ECHO_REPLY`).

use std::ffi::c_void;
use std::sync::OnceLock;

use wal_payload_common::{iat, log};

const ERROR_BUFFER_OVERFLOW: u32 = 111;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_IO_PENDING: u32 = 997;
const MAC: [u8; 6] = [0x00, 0x1A, 0x4D, 0x5E, 0x67, 0x78];

/// The adapter's addresses (network order bytes).
struct Net {
    prefix: [u8; 4],
    iface: [u8; 4],
    router: [u8; 4],
}

impl Net {
    /// The cabinet at `WAL_ALLS_HOST` (default .205) of the subnet, the router at
    /// `WAL_ALLS_ROUTER` (default .1).
    fn new(prefix: [u8; 4]) -> Net {
        let byte = |name: &str, default: u8| std::env::var(name).ok().and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(default);
        let mut iface = prefix;
        iface[3] = byte("WAL_ALLS_HOST", 205);
        let mut router = prefix;
        router[3] = byte("WAL_ALLS_ROUTER", 1);
        Net { prefix, iface, router }
    }
}

static NET: OnceLock<Net> = OnceLock::new();

fn net() -> &'static Net {
    NET.get().expect("netenv::init")
}

unsafe extern "system" {
    fn SetLastError(code: u32);
    fn SetEvent(event: *mut c_void) -> i32;
}

unsafe fn put<T>(base: *mut u8, offset: usize, value: T) {
    unsafe { (base.add(offset) as *mut T).write_unaligned(value) };
}

unsafe fn put_bytes(base: *mut u8, offset: usize, bytes: &[u8]) {
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(offset), bytes.len()) };
}

unsafe fn put_wide(base: *mut u8, offset: usize, s: &str) {
    for (i, c) in s.encode_utf16().chain(Some(0)).enumerate() {
        unsafe { put(base, offset + i * 2, c) };
    }
}

/// A structure's field offset in the 32-bit and the 64-bit layouts (pointers 4 or 8 bytes:
/// the 32-bit games, CHUNITHM, read the 32-bit structures).
const fn w(x86: usize, x64: usize) -> usize {
    if cfg!(target_pointer_width = "64") { x64 } else { x86 }
}

/// A `sockaddr_in` at `offset`.
unsafe fn sockaddr(base: *mut u8, offset: usize, addr: [u8; 4]) {
    unsafe {
        put(base, offset, 2u16);
        put_bytes(base, offset + 4, &addr);
    }
}

/// `SOCKET_ADDRESS` at `offset` pointing to the `sockaddr_in` at `sa`.
unsafe fn socket_address(base: *mut u8, offset: usize, sa: usize) {
    unsafe {
        put(base, offset, base.add(sa) as usize);
        put(base, offset + w(4, 8), 16i32);
    }
}

// IP_ADAPTER_ADDRESSES_LH (448 bytes in 64-bit, 376 in 32-bit), then its records, then the
// strings
const HEAD: usize = 448;
const UNICAST: usize = HEAD; // 64
const DNS: usize = UNICAST + 64; // 32
const GATEWAY: usize = DNS + 32; // 32
const PREFIX: usize = GATEWAY + 32; // 40
const SOCKADDRS: usize = PREFIX + 40; // 4 x 16
const NAME: usize = SOCKADDRS + 64; // 64
const DNS_SUFFIX: usize = NAME + 64; // 3 wide strings, 64 bytes each
const DESCRIPTION: usize = DNS_SUFFIX + 64;
const FRIENDLY: usize = DESCRIPTION + 64;
const ADDRESSES_SIZE: usize = FRIENDLY + 64;

unsafe extern "system" fn get_adapters_addresses(_family: u32, _flags: u32, reserved: *mut c_void, out: *mut u8, size: *mut u32) -> u32 {
    if !reserved.is_null() || size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let have = unsafe { *size } as usize;
    unsafe { *size = ADDRESSES_SIZE as u32 };
    if out.is_null() || have < ADDRESSES_SIZE {
        return ERROR_BUFFER_OVERFLOW;
    }
    let n = net();
    unsafe {
        std::ptr::write_bytes(out, 0, ADDRESSES_SIZE);
        put(out, 0, w(376, HEAD) as u32);
        put(out, 4, 1u32); // IfIndex
        put(out, w(12, 16), out.add(NAME) as usize);
        put(out, w(16, 24), out.add(UNICAST) as usize);
        put(out, w(28, 48), out.add(DNS) as usize);
        put(out, w(32, 56), out.add(DNS_SUFFIX) as usize);
        put(out, w(36, 64), out.add(DESCRIPTION) as usize);
        put(out, w(40, 72), out.add(FRIENDLY) as usize);
        put_bytes(out, w(44, 80), &MAC);
        put(out, w(52, 88), MAC.len() as u32);
        put(out, w(56, 92), 0x4 | 0x80u32); // DHCP enabled, IPv4 enabled
        put(out, w(60, 96), 1500u32);
        put(out, w(64, 100), 6u32); // IF_TYPE_ETHERNET_CSMACD
        put(out, w(68, 104), 1u32); // IfOperStatusUp
        put(out, w(140, 176), out.add(PREFIX) as usize);
        put(out, w(144, 184), 1_000_000_000u64);
        put(out, w(152, 192), 1_000_000_000u64);
        put(out, w(164, 208), out.add(GATEWAY) as usize);
        put_bytes(out, NAME, b"{00000000-0000-0000-0000-000000000000}\0");
        put_wide(out, DNS_SUFFIX, "local");
        put_wide(out, DESCRIPTION, "Interface Description");
        put_wide(out, FRIENDLY, "Fake Ethernet");
        // unicast address: the cabinet, from DHCP, /24
        put(out, UNICAST, w(48, 64) as u32);
        socket_address(out, UNICAST + w(12, 16), SOCKADDRS);
        put(out, UNICAST + w(20, 32), 3u32); // IpPrefixOriginDhcp
        put(out, UNICAST + w(24, 36), 3u32); // IpSuffixOriginDhcp
        put(out, UNICAST + w(28, 40), 4u32); // IpDadStatePreferred
        put(out, UNICAST + w(32, 44), u32::MAX);
        put(out, UNICAST + w(36, 48), u32::MAX);
        put(out, UNICAST + w(40, 52), 86400u32);
        put(out, UNICAST + w(44, 56), 24u8);
        sockaddr(out, SOCKADDRS, n.iface);
        // DNS server and gateway: the router
        put(out, DNS, w(24, 32) as u32);
        socket_address(out, DNS + w(12, 16), SOCKADDRS + 16);
        sockaddr(out, SOCKADDRS + 16, n.router);
        put(out, GATEWAY, w(24, 32) as u32);
        socket_address(out, GATEWAY + w(12, 16), SOCKADDRS + 32);
        sockaddr(out, SOCKADDRS + 32, n.router);
        put(out, PREFIX, w(24, 40) as u32);
        socket_address(out, PREFIX + w(12, 16), SOCKADDRS + 48);
        put(out, PREFIX + w(20, 32), 24u32);
        sockaddr(out, SOCKADDRS + 48, n.prefix);
    }
    0
}

fn dotted(a: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

/// `IP_ADDR_STRING` at `offset`: an address and the /24 mask.
unsafe fn addr_string(base: *mut u8, offset: usize, addr: [u8; 4]) {
    unsafe {
        put_bytes(base, offset + w(4, 8), dotted(addr).as_bytes());
        put_bytes(base, offset + w(20, 24), b"255.255.255.0");
    }
}

const INFO_SIZE: usize = 704;

unsafe extern "system" fn get_adapters_info(out: *mut u8, size: *mut u32) -> u32 {
    if size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let have = unsafe { *size } as usize;
    unsafe { *size = INFO_SIZE as u32 };
    if out.is_null() || have < INFO_SIZE {
        return ERROR_BUFFER_OVERFLOW;
    }
    let n = net();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    unsafe {
        std::ptr::write_bytes(out, 0, INFO_SIZE);
        put_bytes(out, w(8, 12), b"Fake Ethernet");
        put_bytes(out, w(268, 272), b"Adapter Description");
        put(out, w(400, 404), MAC.len() as u32);
        put_bytes(out, w(404, 408), &MAC);
        put(out, w(412, 416), 1u32); // Index
        put(out, w(416, 420), 6u32); // MIB_IF_TYPE_ETHERNET
        put(out, w(420, 424), 1u32); // DhcpEnabled
        addr_string(out, w(428, 440), n.iface);
        addr_string(out, w(468, 488), n.router);
        addr_string(out, w(508, 536), n.router);
        put(out, w(632, 688), now - 3600);
        put(out, w(640, 696), now + 86400);
    }
    0
}

/// `GetIfEntry`: the adapter's row (`MIB_IFROW`, its index filled by the caller).
unsafe extern "system" fn get_if_entry(row: *mut u8) -> u32 {
    if row.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    unsafe {
        std::ptr::write_bytes(row, 0, 860);
        put_wide(row, 0, "Fake Ethernet");
        put(row, 512, 1u32);
        put(row, 516, 6u32);
        put(row, 520, 1500u32);
        put(row, 524, 1_000_000_000u32);
        put(row, 528, MAC.len() as u32);
        put_bytes(row, 532, &MAC);
        put(row, 540, 1u32); // admin status up
        put(row, 544, 5u32); // IF_OPER_STATUS_OPERATIONAL
    }
    0
}

/// `GetIfTable`: the adapter alone (`MIB_IFTABLE`: the count, then its `MIB_IFROW`).
unsafe extern "system" fn get_if_table(table: *mut u8, size: *mut u32, _order: i32) -> u32 {
    if size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let (given, needed) = unsafe { (*size, 4 + 860) };
    unsafe { *size = needed };
    if table.is_null() || given < needed {
        return ERROR_BUFFER_OVERFLOW;
    }
    unsafe {
        put(table, 0, 1u32);
        get_if_entry(table.add(4));
    }
    0
}

unsafe extern "system" fn get_best_route(_dest: u32, _src: u32, row: *mut u8) -> u32 {
    if row.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    unsafe {
        std::ptr::write_bytes(row, 0, 56);
        put(row, 4, u32::MAX);
        put_bytes(row, 12, &net().router);
        put(row, 16, 1u32);
        put(row, 20, 4u32); // MIB_IPROUTE_TYPE_INDIRECT
        put(row, 24, 3u32); // MIB_IPPROTO_NETMGMT
    }
    0
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn icmp_send_echo2(
    handle: usize,
    event: *mut c_void,
    apc: usize,
    _ctx: usize,
    dest: u32,
    _data: *const u8,
    _size: u16,
    _options: *const u8,
    reply: *mut u8,
    reply_size: u32,
    _timeout: u32,
) -> u32 {
    if handle == 0 || handle == usize::MAX || reply.is_null() || apc != 0 {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return 0;
    }
    if reply_size < 40 {
        unsafe { SetLastError(11001) }; // IP_BUF_TOO_SMALL
        return 0;
    }
    unsafe {
        std::ptr::write_bytes(reply, 0, 40);
        put(reply, 0, dest);
        put(reply, 8, 1u32); // round trip
        put(reply, 14, 1u16); // replies in the buffer
        if !event.is_null() {
            SetEvent(event);
            SetLastError(ERROR_IO_PENDING);
            return 0;
        }
        SetLastError(0);
    }
    1
}

/// `IcmpSendEcho` (the game's own router ping): [`icmp_send_echo2`] without its event.
unsafe extern "system" fn icmp_send_echo(
    handle: usize,
    dest: u32,
    data: *const u8,
    size: u16,
    options: *const u8,
    reply: *mut u8,
    reply_size: u32,
    timeout: u32,
) -> u32 {
    unsafe { icmp_send_echo2(handle, std::ptr::null_mut(), 0, 0, dest, data, size, options, reply, reply_size, timeout) }
}

// DNS (segatools' `platform/dns.c`): the store's router (`tenporouter.loc`, `bbrouter.loc`) and
// the ALL.Net hosts are the virtual router (ALL.Net: the payload's server behind it); the
// mobile and DSL routers must not resolve (the check then takes the store router's line).
// Answered at once.

const DNS_ERROR_RCODE_NAME_ERROR: u32 = 9003;

/// The answer record (`DNS_RECORDA`, type A: 127.0.0.1), never freed (`DnsFree` skips it).
struct Record([u8; 64]);
static RECORD: std::sync::Mutex<Record> = std::sync::Mutex::new(Record([0; 64]));
static HOST: &[u8] = b"localhost\0";

fn resolves(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.is_empty() && name != "mobirouter.loc" && name != "dslrouter.loc"
}

/// The address every name resolves to: the router's (a loopback address is refused as
/// ALL.Net's; its port 80 is the payload's server, `allnet.rs`).
fn address(_name: &str) -> [u8; 4] {
    net().router
}

/// The adapter's own address.
pub(crate) fn iface() -> [u8; 4] {
    net().iface
}

/// The virtual router's address (ALL.Net's hosts in the payload's DNS).
pub(crate) fn router() -> [u8; 4] {
    net().router
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
        put_bytes(p, w(24, 32), &addr);
    }
    p as usize
}

unsafe extern "system" fn dns_query_a(name: *const u8, kind: u16, _options: u32, _extra: usize, results: *mut usize, _reserved: usize) -> u32 {
    let name = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned() };
    let found = kind == 1 && resolves(&name);
    log!("netenv: DnsQuery_A {name} type {kind}: {}", if found { dotted(address(&name)) } else { "not found".into() });
    if !results.is_null() {
        unsafe { *results = if found { record(address(&name)) } else { 0 } };
    }
    if found { 0 } else { DNS_ERROR_RCODE_NAME_ERROR }
}

unsafe extern "system" fn dns_query_ex(request: *const u8, result: *mut u8, _cancel: usize) -> u32 {
    let mut found = false;
    let mut host = String::new();
    if !request.is_null() {
        let name = unsafe { (request.add(w(4, 8)) as *const *const u16).read_unaligned() };
        let len = if name.is_null() { 0 } else { (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count() };
        let name = if name.is_null() { String::new() } else { String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }) };
        found = resolves(&name);
        log!("netenv: DnsQueryEx {name}: {found}");
        host = name;
    }
    let status = if found { 0 } else { DNS_ERROR_RCODE_NAME_ERROR };
    if !result.is_null() {
        // DNS_QUERY_RESULT: version, status, options, records
        unsafe {
            put(result, 4, status);
            put(result, 16, if found { record(address(&host)) } else { 0 });
        }
    }
    status
}

static GETADDRINFO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `getaddrinfo`: a name of the network (not an address, not `localhost`) resolved as
/// `DnsQuery` resolves it, the real function given its address.
unsafe extern "system" fn getaddrinfo(node: *const u8, service: *const u8, hints: *const c_void, result: *mut usize) -> i32 {
    type F = unsafe extern "system" fn(*const u8, *const u8, *const c_void, *mut usize) -> i32;
    let real: F = unsafe { std::mem::transmute(GETADDRINFO.load(std::sync::atomic::Ordering::Relaxed)) };
    let name = if node.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(node.cast()) }.to_string_lossy().into_owned() };
    let numeric = name.parse::<std::net::Ipv4Addr>().is_ok() || name.contains(':');
    if name.is_empty() || numeric || name.eq_ignore_ascii_case("localhost") || !resolves(&name) {
        return unsafe { real(node, service, hints, result) };
    }
    let addr = std::ffi::CString::new(dotted(address(&name))).unwrap();
    log!("netenv: getaddrinfo {name}: {}", addr.to_string_lossy());
    unsafe { real(addr.as_ptr().cast(), service, hints, result) }
}

static DNS_FREE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "system" fn dns_free(data: usize, kind: u32) {
    if data == 0 || data == RECORD.lock().unwrap().0.as_ptr() as usize {
        return;
    }
    let orig: unsafe extern "system" fn(usize, u32) = unsafe { std::mem::transmute(DNS_FREE.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { orig(data, kind) }
}

/// The game's process: the addresses only (its connections to ALL.Net's hosts redirected).
pub(crate) fn init_game() {
    let subnet = crate::registry::Cabinet::from_env().subnet;
    let _ = NET.set(Net::new(subnet));
    unsafe {
        if let Some(orig) = iat::hook("ws2_32.dll", "getaddrinfo", getaddrinfo as *const () as usize) {
            GETADDRINFO.store(orig, std::sync::atomic::Ordering::Relaxed);
        }
    }
    // the game's own view of the adapter (Initial D THE ARCADE: its seat from its address)
    if std::env::var("WAL_ALLS_GAME_ADAPTER").is_ok_and(|v| v.trim() == "1") {
        hook_adapters();
    }
}

pub(crate) fn init(subnet: [u8; 4]) {
    let _ = NET.set(Net::new(subnet));
    hook_adapters();
    unsafe {
        iat::hook("dnsapi.dll", "DnsQuery_A", dns_query_a as *const () as usize);
        if let Some(orig) = iat::hook("ws2_32.dll", "getaddrinfo", getaddrinfo as *const () as usize) {
            GETADDRINFO.store(orig, std::sync::atomic::Ordering::Relaxed);
        }
        iat::hook("dnsapi.dll", "DnsQueryEx", dns_query_ex as *const () as usize);
        if let Some(orig) = iat::hook("dnsapi.dll", "DnsFree", dns_free as *const () as usize) {
            DNS_FREE.store(orig, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// The iphlpapi imports answered with the cabinet's adapter.
fn hook_adapters() {
    unsafe {
        let hooks: [(&str, usize); 7] = [
            ("GetAdaptersAddresses", get_adapters_addresses as *const () as usize),
            ("GetAdaptersInfo", get_adapters_info as *const () as usize),
            ("GetIfEntry", get_if_entry as *const () as usize),
            ("GetIfTable", get_if_table as *const () as usize),
            ("GetBestRoute", get_best_route as *const () as usize),
            ("IcmpSendEcho2", icmp_send_echo2 as *const () as usize),
            ("IcmpSendEcho", icmp_send_echo as *const () as usize),
        ];
        for (name, f) in hooks {
            if iat::hook("iphlpapi.dll", name, f).is_none() {
                log!("netenv: {name} not imported");
            }
        }
    }
    log!("alls: network {} (router {})", dotted(net().iface), dotted(net().router));
}
