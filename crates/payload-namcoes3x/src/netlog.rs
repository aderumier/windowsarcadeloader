//! Pass-through IAT hooks for the game's WinHttp card-server client and its network
//! self-test probes (ICMP ping, IP forward table, adapters).
//!
//! The cabinet's card server is an HTTP service on the cabinet network (192.168.112.254:80).
//! The game cannot set the `Host` header itself (it does not import `WinHttpAddRequestHeaders`),
//! so `WinHttpConnect`'s server name is the `Host` of every request: logging it, with the
//! URL path, request headers and response bodies, shows exactly what the game asks the
//! server for.
//!
//! `WAL_NAMCOES3_CARD_SERVER=<host>[:<port>]` redirects the card server: every WinHttp
//! connection (the game's only WinHttp traffic; its host is `192.168.112.254`, or empty once
//! the AMCUS auth answers) goes to that address instead, as plain HTTP (the game asks for HTTPS
//! on the default port). The cabinet network names (`tenporouter.loc`, `bbrouter.loc`, ...)
//! resolve to 127.0.0.1. Every other call is forwarded to the original function unchanged.

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use wal_payload_common::iat;

static WH_OPEN: AtomicUsize = AtomicUsize::new(0);
static WH_CONNECT: AtomicUsize = AtomicUsize::new(0);
static WH_OPEN_REQUEST: AtomicUsize = AtomicUsize::new(0);
static WH_SEND_REQUEST: AtomicUsize = AtomicUsize::new(0);
static WH_RECEIVE_RESPONSE: AtomicUsize = AtomicUsize::new(0);
static WH_READ_DATA: AtomicUsize = AtomicUsize::new(0);
static WH_QUERY_HEADERS: AtomicUsize = AtomicUsize::new(0);
static ICMP_CREATE: AtomicUsize = AtomicUsize::new(0);
static ICMP_SEND: AtomicUsize = AtomicUsize::new(0);
static ICMP_CLOSE: AtomicUsize = AtomicUsize::new(0);
static GET_ADAPTERS: AtomicUsize = AtomicUsize::new(0);
static GET_FORWARD: AtomicUsize = AtomicUsize::new(0);
static GETADDRINFO: AtomicUsize = AtomicUsize::new(0);
static WSA_IOCTL: AtomicUsize = AtomicUsize::new(0);

/// The cabinet's network (bananatools' MKDX setup): the boot local network check wants an
/// interface address 192.168.112.11-20 and an adapter with a gateway.
const CABINET_IP: [u8; 4] = [192, 168, 112, 11];
const CABINET_MASK: [u8; 4] = [255, 255, 255, 0];
const CABINET_GATEWAY: [u8; 4] = [192, 168, 112, 1];

/// Connection handles redirected to `WAL_NAMCOES3_CARD_SERVER` (their requests drop HTTPS).
static REDIRECTED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// `WAL_NAMCOES3_CARD_SERVER` as (host, port); port 80 when not given.
pub(crate) fn card_server() -> Option<(String, u16)> {
    let v = std::env::var("WAL_NAMCOES3_CARD_SERVER").ok()?;
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    Some(match v.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(80)),
        None => (v.to_string(), 80),
    })
}

/// Safety: the game passes the pointer; it must be a readable UTF-16 C string (or null).
/// `max` caps the length; `len` (characters, `u32::MAX` = terminated) bounds it as well.
fn wstr(ptr: *const u16, len: u32, max: usize) -> String {
    if ptr.is_null() {
        return "-".into();
    }
    let mut units = Vec::new();
    for i in 0..max.min(len as usize) {
        let c = unsafe { *ptr.add(i) };
        if c == 0 {
            break;
        }
        units.push(c);
    }
    String::from_utf16_lossy(&units)
}

/// (WinHttp handle, response body bytes logged so far): only the first 1 KB of a body is written.
static BODY: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// Safety: the game passes the pointer and length; the buffer must be readable.
fn cbuf(ptr: *const c_char, len: u32) -> String {
    if ptr.is_null() || len == 0 {
        return "-".into();
    }
    let mut out = String::new();
    for i in 0..len.min(256) {
        out.push(unsafe { *ptr.add(i as usize) } as u8 as char);
    }
    if len > 256 {
        out.push_str(" …");
    }
    out
}

fn ip(a: u32) -> String {
    format!("{}.{}.{}.{}", (a >> 24) & 0xff, (a >> 16) & 0xff, (a >> 8) & 0xff, a & 0xff)
}

/// Installs the pass-through logging hooks (the game imports every function listed).
pub unsafe fn init() {
    macro_rules! h {
        ($dll:literal, $name:literal, $slot:expr, $f:ident) => {
            if let Some(o) = iat::hook($dll, $name, $f as *const () as usize) {
                $slot.store(o, Ordering::Relaxed);
            }
        };
    }
    h!("WINHTTP.dll", "WinHttpOpen", WH_OPEN, winhttp_open);
    h!("WINHTTP.dll", "WinHttpConnect", WH_CONNECT, winhttp_connect);
    h!("WINHTTP.dll", "WinHttpOpenRequest", WH_OPEN_REQUEST, winhttp_open_request);
    h!("WINHTTP.dll", "WinHttpSendRequest", WH_SEND_REQUEST, winhttp_send_request);
    h!("WINHTTP.dll", "WinHttpReceiveResponse", WH_RECEIVE_RESPONSE, winhttp_receive_response);
    h!("WINHTTP.dll", "WinHttpReadData", WH_READ_DATA, winhttp_read_data);
    h!("WINHTTP.dll", "WinHttpQueryHeaders", WH_QUERY_HEADERS, winhttp_query_headers);
    h!("IPHLPAPI.DLL", "IcmpCreateFile", ICMP_CREATE, icmp_create_file);
    h!("IPHLPAPI.DLL", "IcmpSendEcho", ICMP_SEND, icmp_send_echo);
    h!("IPHLPAPI.DLL", "IcmpCloseHandle", ICMP_CLOSE, icmp_close_handle);
    h!("IPHLPAPI.DLL", "GetAdaptersInfo", GET_ADAPTERS, get_adapters_info);
    h!("IPHLPAPI.DLL", "GetIpForwardTable", GET_FORWARD, get_ip_forward_table);
    h!("WS2_32.dll", "getaddrinfo", GETADDRINFO, getaddrinfo);
    h!("WS2_32.dll", "WSAIoctl", WSA_IOCTL, wsa_ioctl);
    wal_payload_common::log!("namcoes3: network IAT hooks installed (WinHttp card server + self-test probes)");
}

unsafe extern "system" fn winhttp_open(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> usize {
    wal_payload_common::log!(
        "netlog: WinHttpOpen agent=\"{}\" access={:#x} proxy=\"{}\" bypass=\"{}\" flags={:#x}",
        wstr(agent, u32::MAX, 128), access, wstr(proxy, u32::MAX, 64), wstr(bypass, u32::MAX, 64), flags
    );
    let f: unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> usize =
        unsafe { std::mem::transmute(WH_OPEN.load(Ordering::Relaxed)) };
    unsafe { f(agent, access, proxy, bypass, flags) }
}

/// The server name is what WinHttp puts in the `Host` header of every request on the session
/// (port 0 means the default port). Redirected to `WAL_NAMCOES3_CARD_SERVER` when set.
unsafe extern "system" fn winhttp_connect(sess: usize, server: *const u16, port: u16, reserved: u32) -> usize {
    let name = wstr(server, u32::MAX, 128);
    let f: unsafe extern "system" fn(usize, *const u16, u16, u32) -> usize =
        unsafe { std::mem::transmute(WH_CONNECT.load(Ordering::Relaxed)) };
    if let Some((host, to_port)) = card_server() {
        let wide: Vec<u16> = host.encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe { f(sess, wide.as_ptr(), to_port, reserved) };
        wal_payload_common::log!("netlog: WinHttpConnect \"{name}\":{port} -> redirected to http://{host}:{to_port} (handle {h:#x})");
        if h != 0 {
            REDIRECTED.lock().unwrap().push(h);
        }
        return h;
    }
    wal_payload_common::log!("netlog: WinHttpConnect session={sess:#x} server=\"{name}\" port={port}");
    unsafe { f(sess, server, port, reserved) }
}

/// A request on a redirected connection drops `WINHTTP_FLAG_SECURE`: the local server is HTTP.
unsafe extern "system" fn winhttp_open_request(
    req: usize, verb: *const u16, path: *const u16, version: *const u16,
    referer: *const u16, accept: *const *const u16, mut flags: u32,
) -> usize {
    if REDIRECTED.lock().unwrap().contains(&req) {
        flags &= !0x0080_0000; // WINHTTP_FLAG_SECURE
    }
    wal_payload_common::log!(
        "netlog: WinHttpOpenRequest connection={:#x} verb=\"{}\" path=\"{}\" flags={:#x}",
        req, wstr(verb, u32::MAX, 16), wstr(path, u32::MAX, 256), flags
    );
    let f: unsafe extern "system" fn(usize, *const u16, *const u16, *const u16, *const u16, *const *const u16, u32) -> usize =
        unsafe { std::mem::transmute(WH_OPEN_REQUEST.load(Ordering::Relaxed)) };
    unsafe { f(req, verb, path, version, referer, accept, flags) }
}

unsafe extern "system" fn winhttp_send_request(
    req: usize, headers: *const u16, hdr_len: u32, body: *mut c_void, body_len: u32, total: u32, ctx: usize,
) -> i32 {
    wal_payload_common::log!("netlog: WinHttpSendRequest request={:#x} total={} bytes", req, total);
    if !headers.is_null() && hdr_len != 0 {
        wal_payload_common::log!("netlog:   headers: {}", wstr(headers, hdr_len, 512));
    }
    if !body.is_null() && body_len != 0 {
        wal_payload_common::log!("netlog:   request body ({} bytes): {}", body_len, cbuf(body.cast(), body_len));
    }
    let f: unsafe extern "system" fn(usize, *const u16, u32, *mut c_void, u32, u32, usize) -> i32 =
        unsafe { std::mem::transmute(WH_SEND_REQUEST.load(Ordering::Relaxed)) };
    unsafe { f(req, headers, hdr_len, body, body_len, total, ctx) }
}

unsafe extern "system" fn winhttp_receive_response(req: usize, reserved: usize) -> i32 {
    let f: unsafe extern "system" fn(usize, usize) -> i32 =
        unsafe { std::mem::transmute(WH_RECEIVE_RESPONSE.load(Ordering::Relaxed)) };
    let r = unsafe { f(req, reserved) };
    wal_payload_common::log!("netlog: WinHttpReceiveResponse request={:#x} -> {}", req, r);
    r
}

unsafe extern "system" fn winhttp_read_data(req: usize, buf: *mut c_char, n: u32, read: *mut u32) -> i32 {
    let f: unsafe extern "system" fn(usize, *mut c_char, u32, *mut u32) -> i32 =
        unsafe { std::mem::transmute(WH_READ_DATA.load(Ordering::Relaxed)) };
    let r = unsafe { f(req, buf, n, read) };
    if r != 0 {
        let got = if read.is_null() { 0 } else { unsafe { *read } };
        let mut body = BODY.lock().unwrap();
        let already = match body.iter_mut().find(|(h, _)| *h == req) {
            Some(e) => {
                let a = e.1;
                e.1 += got;
                a
            }
            None => {
                body.push((req, got));
                0
            }
        };
        if already < 1024 {
            wal_payload_common::log!("netlog:   response body ({} bytes, {} so far): {}", got, already + got, cbuf(buf, got));
        }
    }
    r
}

unsafe extern "system" fn winhttp_query_headers(
    handle: usize, flags: u32, name: *const u16, buf: *mut u16, buf_len: *mut u32, index: *mut u32,
) -> i32 {
    let label = if name as usize == 0xFFFFFFFF {
        "<status line>".into()
    } else if name as usize == 0xFFFFFFFE {
        "<raw headers>".into()
    } else {
        format!("<{}>", wstr(name, u32::MAX, 64))
    };
    let f: unsafe extern "system" fn(usize, u32, *const u16, *mut u16, *mut u32, *mut u32) -> i32 =
        unsafe { std::mem::transmute(WH_QUERY_HEADERS.load(Ordering::Relaxed)) };
    let r = unsafe { f(handle, flags, name, buf, buf_len, index) };
    if r != 0 && !buf.is_null() && !buf_len.is_null() {
        // the length is in bytes, the buffer UTF-16 (flags without WINHTTP_QUERY_FLAG_NUMBER)
        let text = if flags & 0x2000_0000 != 0 { format!("{}", unsafe { *(buf as *const u32) }) } else { wstr(buf, unsafe { *buf_len } / 2, 512) };
        wal_payload_common::log!("netlog: WinHttpQueryHeaders {} = \"{}\"", label, text);
    }
    r
}

// Wine's iphlpapi ICMP is incompatible with the game's pings: IcmpCreateFile returns a handle
// the game cannot use and IcmpSendEcho faults on it (an access violation inside Wine, taking
// the process down when the ping runs in attract mode). Never call it: every echo is answered
// at once with a successful 1 ms reply, so the venue router, game server and online network
// checks (test menu) pass, as on a cabinet with a working network.
unsafe extern "system" fn icmp_create_file() -> usize {
    0x1c3c // any non-INVALID_HANDLE_VALUE handle: the game only passes it back
}

/// Fills one ICMP_ECHO_REPLY (32-bit layout, 28 bytes), followed by the echoed data when the
/// reply buffer has room for it.
unsafe extern "system" fn icmp_send_echo(
    _handle: usize, dest: u32, req: *const u8, req_size: u16, _options: *const c_void, reply: *mut u8, reply_size: u32, timeout: u32,
) -> u32 {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| wal_payload_common::log!(
        "netlog: IcmpSendEcho {} data {req_size} reply buffer {reply_size} timeout {timeout} -> echo reply (simulated, 1 ms)",
        ip(dest.swap_bytes())
    ));
    if reply.is_null() || reply_size < 28 {
        return 0;
    }
    let copy = (req_size as u32).min(reply_size - 28) as usize;
    unsafe {
        std::ptr::write_bytes(reply, 0, 28);
        (reply as *mut u32).write_unaligned(dest); // Address
        (reply.add(4) as *mut u32).write_unaligned(0); // Status: IP_SUCCESS
        (reply.add(8) as *mut u32).write_unaligned(1); // RoundTripTime (ms)
        (reply.add(12) as *mut u16).write_unaligned(copy as u16); // DataSize
        let data = reply.add(28);
        if !req.is_null() && copy > 0 {
            std::ptr::copy_nonoverlapping(req, data, copy);
        }
        (reply.add(16) as *mut usize).write_unaligned(data as usize); // Data
        *reply.add(20) = 64; // Options.Ttl
    }
    1
}

unsafe extern "system" fn icmp_close_handle(_handle: usize) -> i32 {
    1
}

fn dotted(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// One fake Ethernet adapter (IP_ADAPTER_INFO, 32-bit layout, 648 bytes) on the cabinet
/// network: the local network check reads its gateway.
unsafe extern "system" fn get_adapters_info(info: *mut u8, len: *mut u32) -> u32 {
    const SIZE: u32 = 648;
    if len.is_null() {
        return 87; // ERROR_INVALID_PARAMETER
    }
    if info.is_null() || unsafe { *len } < SIZE {
        unsafe { *len = SIZE };
        return 111; // ERROR_BUFFER_OVERFLOW
    }
    let put = |off: usize, s: &str| unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), info.add(off), s.len());
    };
    unsafe {
        std::ptr::write_bytes(info, 0, SIZE as usize);
        put(8, "{00000000-0000-0000-0000-000000000001}"); // AdapterName
        put(268, "Ethernet"); // Description
        *(info.add(400) as *mut u32) = 6; // AddressLength
        std::ptr::copy_nonoverlapping([0x00, 0x1c, 0x42, 0x11, 0x12, 0x13].as_ptr(), info.add(404), 6);
        *(info.add(412) as *mut u32) = 1; // Index
        *(info.add(416) as *mut u32) = 6; // MIB_IF_TYPE_ETHERNET
    }
    put(432, &dotted(CABINET_IP)); // IpAddressList
    put(448, &dotted(CABINET_MASK));
    put(472, &dotted(CABINET_GATEWAY)); // GatewayList
    put(488, "255.255.255.255");
    unsafe { *len = SIZE };
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| wal_payload_common::log!("netlog: GetAdaptersInfo -> fake adapter {} gateway {}", dotted(CABINET_IP), dotted(CABINET_GATEWAY)));
    0
}

/// `SIO_GET_INTERFACE_LIST` answers one interface on the cabinet network (INTERFACE_INFO,
/// 76 bytes); every other control code goes to Winsock.
unsafe extern "system" fn wsa_ioctl(
    socket: usize, code: u32, inbuf: *mut c_void, inlen: u32, outbuf: *mut u8, outlen: u32, bytes: *mut u32, ov: *mut c_void, routine: *mut c_void,
) -> i32 {
    const SIO_GET_INTERFACE_LIST: u32 = 0x4004_747f;
    if code == SIO_GET_INTERFACE_LIST && !outbuf.is_null() && outlen >= 76 {
        let sockaddr = |off: usize, ip: [u8; 4]| unsafe {
            *(outbuf.add(off) as *mut u16) = 2; // AF_INET
            std::ptr::copy_nonoverlapping(ip.as_ptr(), outbuf.add(off + 4), 4);
        };
        unsafe {
            std::ptr::write_bytes(outbuf, 0, 76);
            *(outbuf as *mut u32) = 1 | 4; // IFF_UP | IFF_BROADCAST
        }
        sockaddr(4, CABINET_IP);
        sockaddr(28, [192, 168, 112, 255]);
        sockaddr(52, CABINET_MASK);
        if !bytes.is_null() {
            unsafe { *bytes = 76 };
        }
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| wal_payload_common::log!("netlog: SIO_GET_INTERFACE_LIST -> {}", dotted(CABINET_IP)));
        return 0;
    }
    let f: unsafe extern "system" fn(usize, u32, *mut c_void, u32, *mut u8, u32, *mut u32, *mut c_void, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(WSA_IOCTL.load(Ordering::Relaxed)) };
    unsafe { f(socket, code, inbuf, inlen, outbuf, outlen, bytes, ov, routine) }
}

unsafe extern "system" fn get_ip_forward_table(table: *mut c_void, size: *mut u32, order: u32) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void, *mut u32, u32) -> u32 = unsafe { std::mem::transmute(GET_FORWARD.load(Ordering::Relaxed)) };
    let r = unsafe { f(table, size, order) };
    let s = if size.is_null() { 0 } else { unsafe { *size } };
    wal_payload_common::log!("netlog: GetIpForwardTable order={} -> {:#x} (size {})", order, r, s);
    r
}

/// The cabinet network names (`tenporouter.loc`, ...) resolve to 127.0.0.1: unresolved, the
/// game raises an error (0x1b) when it renews its hop count to the shop router.
unsafe extern "system" fn getaddrinfo(node: *const c_char, service: *const c_char, hints: *const c_void, result: *mut *mut c_void) -> i32 {
    let f: unsafe extern "system" fn(*const c_char, *const c_char, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { std::mem::transmute(GETADDRINFO.load(Ordering::Relaxed)) };
    let name = if node.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(node) }.to_string_lossy().into_owned() };
    if name.ends_with(".loc") {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| wal_payload_common::log!("netlog: getaddrinfo {name} -> 127.0.0.1 (cabinet network name)"));
        return unsafe { f(c"127.0.0.1".as_ptr(), service, hints, result) };
    }
    unsafe { f(node, service, hints, result) }
}
