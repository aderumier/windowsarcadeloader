//! Pass-through IAT hooks for the game's WinHttp card-server client and its network
//! self-test probes (ICMP ping, IP forward table, adapters).
//!
//! The cabinet's card server is an HTTP service on the cabinet network (192.168.112.254:80).
//! The game cannot set the `Host` header itself (it does not import `WinHttpAddRequestHeaders`),
//! so `WinHttpConnect`'s server name is the `Host` of every request: logging it, with the
//! URL path, request headers and response bodies, shows exactly what the game asks the
//! server for. Every call is forwarded to the original function unchanged.

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

/// (WinHttp handle, response body bytes logged so far): only the first 1 KB of a body is written.
static BODY: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// Safety: the game passes the pointer; it must be a readable C string (or null).
fn cstr(ptr: *const c_char, max: usize) -> String {
    if ptr.is_null() {
        return "-".into();
    }
    let mut out = String::new();
    for i in 0..max {
        let b = unsafe { *ptr.add(i) } as u8;
        if b == 0 {
            break;
        }
        out.push(b as char);
    }
    out
}

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
    wal_payload_common::log!("namcoes3: network IAT hooks installed (WinHttp card server + self-test probes)");
}

unsafe extern "system" fn winhttp_open(agent: *const c_char, access: u32, proxy: *const c_char, bypass: *const c_char, flags: u32) -> usize {
    wal_payload_common::log!(
        "netlog: WinHttpOpen agent=\"{}\" access={:#x} proxy=\"{}\" bypass=\"{}\" flags={:#x}",
        cstr(agent, 128), access, cstr(proxy, 64), cstr(bypass, 64), flags
    );
    let f: unsafe extern "system" fn(*const c_char, u32, *const c_char, *const c_char, u32) -> usize =
        unsafe { std::mem::transmute(WH_OPEN.load(Ordering::Relaxed)) };
    unsafe { f(agent, access, proxy, bypass, flags) }
}

/// The server name is what WinHttp puts in the `Host` header of every request on the session
/// (port 0 means the HTTP default, 80).
unsafe extern "system" fn winhttp_connect(sess: usize, server: *const c_char, port: u16, reserved: u32) -> usize {
    wal_payload_common::log!(
        "netlog: WinHttpConnect session={:#x} server=\"{}\" port={}  <- Host of every request on this session",
        sess, cstr(server, 128), port
    );
    let f: unsafe extern "system" fn(usize, *const c_char, u16, u32) -> usize =
        unsafe { std::mem::transmute(WH_CONNECT.load(Ordering::Relaxed)) };
    unsafe { f(sess, server, port, reserved) }
}

unsafe extern "system" fn winhttp_open_request(
    req: usize, verb: *const c_char, path: *const c_char, referer: *const c_char,
    reserved: *const c_char, accept: *const *const c_char, flags: u32,
) -> usize {
    wal_payload_common::log!(
        "netlog: WinHttpOpenRequest request={:#x} verb=\"{}\" path=\"{}\" referer=\"{}\" flags={:#x}",
        req, cstr(verb, 16), cstr(path, 256), cstr(referer, 64), flags
    );
    let f: unsafe extern "system" fn(usize, *const c_char, *const c_char, *const c_char, *const c_char, *const *const c_char, u32) -> usize =
        unsafe { std::mem::transmute(WH_OPEN_REQUEST.load(Ordering::Relaxed)) };
    unsafe { f(req, verb, path, referer, reserved, accept, flags) }
}

unsafe extern "system" fn winhttp_send_request(
    req: usize, headers: *const c_char, hdr_len: u32, body: *mut c_void, body_len: u32, total: u32, ctx: usize,
) -> i32 {
    wal_payload_common::log!("netlog: WinHttpSendRequest request={:#x} total={} bytes", req, total);
    if !headers.is_null() && hdr_len != 0 {
        wal_payload_common::log!("netlog:   headers: {}", cbuf(headers, hdr_len));
    }
    if !body.is_null() && body_len != 0 {
        wal_payload_common::log!("netlog:   request body ({} bytes): {}", body_len, cbuf(body.cast(), body_len));
    }
    let f: unsafe extern "system" fn(usize, *const c_char, u32, *mut c_void, u32, u32, usize) -> i32 =
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
    handle: usize, flags: u32, name: *const c_char, buf: *mut c_char, buf_len: *mut u32, index: *mut u32,
) -> i32 {
    let label = if name as usize == 0xFFFFFFFF {
        "<status line>".into()
    } else if name as usize == 0xFFFFFFFE {
        "<raw headers>".into()
    } else {
        format!("<{}>", cstr(name, 64))
    };
    let f: unsafe extern "system" fn(usize, u32, *const c_char, *mut c_char, *mut u32, *mut u32) -> i32 =
        unsafe { std::mem::transmute(WH_QUERY_HEADERS.load(Ordering::Relaxed)) };
    let r = unsafe { f(handle, flags, name, buf, buf_len, index) };
    if r != 0 && !buf.is_null() && !buf_len.is_null() {
        wal_payload_common::log!("netlog: WinHttpQueryHeaders {} = \"{}\"", label, cbuf(buf, unsafe { *buf_len }));
    }
    r
}

// Wine's iphlpapi ICMP is incompatible with the game's game-server ping: IcmpCreateFile
// returns a handle the game cannot use and IcmpSendEcho faults on it (an access violation
// inside Wine, taking the process down when the ping runs in attract mode). The ping is the
// network self-test whose error-raise we already suppress, so short-circuit all three ICMP
// calls: report "no ping" without ever calling Wine's iphlpapi. The ping loop then does a few
// fast no-op retries and returns, and the game proceeds as if offline.
unsafe extern "system" fn icmp_create_file(_a: u32, _b: u32) -> u32 {
    wal_payload_common::log!("netlog: IcmpCreateFile -> 0 (short-circuited; Wine iphlpapi ICMP is unsafe here)");
    0 // INVALID_HANDLE_VALUE-ish failure
}

unsafe extern "system" fn icmp_send_echo(
    _handle: u32, _dest: u32, _req: *const c_char, req_size: u16, _reply: *mut c_char, _reply_size: u16, timeout: u32,
) -> u32 {
    wal_payload_common::log!("netlog: IcmpSendEcho req={} timeout={} -> 0 (short-circuited, no echo)", req_size, timeout);
    0
}

unsafe extern "system" fn icmp_close_handle(_handle: u32) -> i32 {
    0
}

unsafe extern "system" fn get_adapters_info(info: *mut c_void, len: *mut u32) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void, *mut u32) -> u32 = unsafe { std::mem::transmute(GET_ADAPTERS.load(Ordering::Relaxed)) };
    let r = unsafe { f(info, len) };
    let size = if len.is_null() { 0 } else { unsafe { *len } };
    wal_payload_common::log!("netlog: GetAdaptersInfo -> {:#x} (size {})", r, size);
    r
}

unsafe extern "system" fn get_ip_forward_table(table: *mut c_void, size: *mut u32, order: u32) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void, *mut u32, u32) -> u32 = unsafe { std::mem::transmute(GET_FORWARD.load(Ordering::Relaxed)) };
    let r = unsafe { f(table, size, order) };
    let s = if size.is_null() { 0 } else { unsafe { *size } };
    wal_payload_common::log!("netlog: GetIpForwardTable order={} -> {:#x} (size {})", order, r, s);
    r
}
