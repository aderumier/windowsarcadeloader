//! The RingEdge system services amLib talks to over PCP on 127.0.0.1, served by the payload:
//! `mxmaster` (amMaster: application control, TCP 40100), `mxkeychip` (amDongle: the
//! keychip, TCP 40106, its binary data channel 40107) and the auth service (amAuth, RingEdge 2:
//! TCP 40190). The network service (amNetwork, 40104); the test menus also open the
//! installer (amInstall, 40102), the game delivery
//! services (amGdeliver 40108, amGcatcher 40110) and the storage service (amStorage, 40114, its
//! binary channel 40115).
//!
//! PCP is a text protocol: the service sends the prompt `>`, the client a request line
//! `key=value&key=value\r\n` (`?` for a query, e.g. `keychip.appboot.gameid=?&cache=1`), the
//! service answers with the same first key and its value (`keychip.appboot.gameid=SBWX\r\n`);
//! a `code=<n>` pair reports an error. Keychip values are text (game id, billing ids) or the
//! hex digits of binary values (region, flags, counters).
//!
//! Answers: built-in defaults below, overridden with `WAL_RINGEDGE_PCP=key=value;...`
//! (e.g. `keychip.appboot.gameid=SBWX`). Unknown keys are logged and answered `0`.
//!
//! `mxmaster.foreground.next=<n>`: the game hands over to application `n` of the cabinet and
//! exits (the test button: 3, the test menu; the test menu's exit: the game). The launcher
//! starts the game again with that application's arguments: `WAL_RINGEDGE_FOREGROUND=
//! <n>=<arguments>;...` (default `3=-t`, others: no argument).
//!
//! `keychip.ssd.proof=<challenge>`: the keychip's answer to a 16-byte challenge, checked
//! against the challenge file's SSD records (Transformers: Human Alliance).
//!
//! `keychip.ds.compute=<challenge>&page=<n>`: the keychip's DS28CN01 SHA-1 MAC (20 bytes) of a
//! 7-byte challenge for one of its 4 pages. amDongle checks the answer against MACs it knows:
//! challenges from the game's challenge file (`SBWX_Table.dat`: 10000 records of 87 bytes, the
//! challenge then the MACs of the 4 pages, each stored with its bytes shuffled, see
//! [`MAC_ORDER`]; then the SSD records) and a fixed one in the executable, followed by its 4
//! MACs. Both are looked up: `WAL_RINGEDGE_DS_FILE=<file>` (from the game directory) and
//! the fixed one found in the running executable ([`ds_table`], or `WAL_RINGEDGE_DS_TABLE=
//! <challenge rva>,<macs rva>`). A challenge file missing from the dump is generated in the
//! save folder and opened in its place ([`init_ds_file`]).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

use wal_payload_common::log;

const MXMASTER: u16 = 40100;
const MXKEYCHIP: u16 = 40106;
const MXKEYCHIP_DATA: u16 = 40107;
const MXINSTALLER: u16 = 40102;
const MXNETWORK: u16 = 40104;
const MXGDELIVER: u16 = 40108;
const MXGCATCHER: u16 = 40110;
const MXSTORAGE: u16 = 40114;
const MXSTORAGE_DATA: u16 = 40115;
const AUTH: u16 = 40190;
const AUTH_DATA: u16 = 40191;
/// `place.info`'s binary record (shop information), sent on the auth data channel: empty.
const PLACE_INFO_SIZE: usize = 0xCE;

const DEFAULTS: &[(&str, &str)] = &[
    ("keychip.version", "0104"),
    ("keychip.appboot.gameid", "SBWX"),
    ("keychip.appboot.platformid", "AAL"),
    ("keychip.appboot.region", "01"),
    ("keychip.appboot.modeltype", "00"),
    ("keychip.appboot.systemflag", "00"),
    ("keychip.appboot.formattype", "00"),
    ("keychip.appboot.dvdflag", "00"),
    ("keychip.appboot.networkaddr", "C0A80000"),
    ("keychip.billing.keyid", "A72E-01A00000001"),
    ("keychip.billing.mainid", "A72E-01A00000001"),
    ("keychip.billing.playlimit", "FFFFFFFF"),
    ("keychip.billing.playcount", "00000000"),
    ("keychip.billing.nearfull", "00000000"),
    ("mxmaster.logging_available", "0"),
    ("auth.serial.main", "AAVE-01A00000001"),
    ("mxmaster.develop", "0"),
];

fn values() -> &'static HashMap<String, String> {
    static VALUES: OnceLock<HashMap<String, String>> = OnceLock::new();
    VALUES.get_or_init(|| {
        let mut map: HashMap<String, String> = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        // the keychip's network: the machine's (games compare their address with it)
        if let Some(network) = local_address().map(|ip| u32::from(ip) & 0xFFFF_FF00) {
            map.insert("keychip.appboot.networkaddr".into(), format!("{network:08X}"));
            log!("pcp: keychip network {}.{}.{}.0", network >> 24, (network >> 16) & 0xFF, (network >> 8) & 0xFF);
        }
        if let Ok(spec) = std::env::var("WAL_RINGEDGE_PCP") {
            for item in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some((k, v)) = item.split_once('=') {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        map
    })
}

/// The machine's IPv4 address (the default route's: a UDP socket "connected" outside, no
/// packet sent).
fn local_address() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("192.0.2.1", 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

pub(crate) fn start() {
    // read before the first connection: an environment lookup in a hook-free thread is fine,
    // but keep the overrides logged once
    let _ = values();
    let _ = ds_table();
    let _ = ds_file();
    let services = [
        (MXMASTER, "mxmaster"),
        (MXINSTALLER, "mxinstaller"),
        (MXNETWORK, "mxnetwork"),
        (MXKEYCHIP, "mxkeychip"),
        (MXKEYCHIP_DATA, "mxkeychip data"),
        (MXGDELIVER, "mxgdeliver"),
        (MXGCATCHER, "mxgcatcher"),
        (MXSTORAGE, "mxstorage"),
        (MXSTORAGE_DATA, "mxstorage data"),
        (AUTH, "auth"),
        (AUTH_DATA, "auth data"),
    ];
    for (port, name) in services {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                std::thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        std::thread::spawn(move || match port {
                            AUTH_DATA => serve_place_info(stream),
                            _ => serve(stream, name, port == MXKEYCHIP_DATA),
                        });
                    }
                });
            }
            Err(e) => log!("pcp: cannot listen on {port} ({name}): {e}"),
        }
    }
    log!("pcp: mxmaster on {MXMASTER}, mxkeychip on {MXKEYCHIP}/{MXKEYCHIP_DATA}, auth on {AUTH}/{AUTH_DATA}");
}

fn serve(mut stream: TcpStream, name: &str, binary: bool) {
    let _ = stream.set_nodelay(true);
    log!("pcp: {name}: client connected");
    if binary {
        // data channel of the keychip's encrypt/decrypt requests: logged only
        let mut buf = [0u8; 512];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            log!("pcp: {name}: {n} bytes {:02X?}", &buf[..n.min(64)]);
        }
        return;
    }
    if stream.write_all(b">").is_err() {
        return;
    }
    let mut line = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        line.extend_from_slice(&buf[..n]);
        while let Some(end) = line.windows(2).position(|w| w == b"\r\n") {
            let request = String::from_utf8_lossy(&line[..end]).into_owned();
            line.drain(..end + 2);
            let reply = match name {
                "auth" => auth_answer(&request),
                "mxmaster" | "mxkeychip" => answer(&request),
                "mxnetwork" => network_answer(&request),
                _ => service_answer(&request),
            };
            log!("pcp: {name}: {request} -> {reply}");
            if stream.write_all(format!("{reply}\r\n>").as_bytes()).is_err() {
                return;
            }
        }
    }
    log!("pcp: {name}: client disconnected");
}

/// The auth data channel: the `place.info` record.
fn serve_place_info(mut stream: TcpStream) {
    log!("pcp: auth data: place.info record sent");
    let _ = stream.write_all(&[0u8; PLACE_INFO_SIZE]);
}

/// The auth service's reply (amAuth, RingEdge 2: ALL.Net / NESiCA authentication): the command
/// with `ok` (`busy`, `ng`: failures), then the values asked.
fn auth_answer(request: &str) -> String {
    let command = request.split('&').next().unwrap_or("").split('=').next().unwrap_or("");
    match command {
        // no error, network icon normal, not asked to exit
        "current.status" => format!("{command}=ok&error=0&icon=normal&exit=false"),
        // the game's online server (a number: the address as an integer): none
        "title.server.address" => format!("{command}=ok&ipaddr=0"),
        // open all day
        "close.time" => format!("{command}=ok&time=alltime"),
        // the record follows on the data channel
        "place.info" => format!("{command}=ok&size={PLACE_INFO_SIZE}"),
        // the keychip's and the main board's serial ids
        "serial.id" => {
            let keychip = values().get("keychip.billing.keyid").map_or("", |v| v.as_str());
            let main = values().get("auth.serial.main").map_or("", |v| v.as_str());
            format!("{command}=ok&keychip={keychip}&main={main}")
        }
        "system.time" => {
            let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
            unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
            format!("{command}=ok&time={:04}{:02}{:02}{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
        }
        _ => format!("{command}=ok"),
    }
}

/// The reply to one request: its first key with the value.
fn answer(request: &str) -> String {
    let first = request.split('&').next().unwrap_or("");
    let (key, value) = first.split_once('=').unwrap_or((first, ""));
    if key == "mxmaster.foreground.next" {
        let args = foreground_args(value.trim());
        log!("pcp: next application {value}: relaunch with '{args}'");
        wal_payload_common::request_relaunch(&args);
        // the cabinet's master ends the game for the next application: a game that stays
        // (Teratoma starts again in place, its amMaster then fails: error 3005) is ended
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            log!("pcp: the game did not end for the next application: ended");
            std::process::exit(0);
        });
    }
    // the starts of application <n> since the cabinet's boot: this one, the first (amMaster
    // 1.03, RingWide: the answer needs `count`, else "Failed to get FirstBootFlag")
    if key == "mxmaster.foreground.getcount" {
        return format!("{key}={value}&count=1");
    }
    if key == "keychip.ssd.proof" {
        return format!("{key}={}", ssd_proof(value));
    }
    if key == "keychip.ds.compute" {
        return format!("{key}={}", ds_compute(request));
    }
    if value != "?" {
        // a command (mxmaster.foreground.next=..., keychip.tracedata.put=...): acknowledged
        return format!("{key}={value}");
    }
    match values().get(key) {
        Some(v) => format!("{key}={v}"),
        None => {
            log!("pcp: unknown key {key}");
            format!("{key}=0")
        }
    }
}

/// The network service (amNetwork): the machine's network card, up, its address from DHCP.
/// Each query's answer carries its keyword (`query_dhcp_status` -> `dhcp_status=`; without
/// it amNetwork reports "Response Error!!! No Keyword").
fn network_answer(request: &str) -> String {
    let name = request.split('&').find_map(|p| p.strip_prefix("request=")).unwrap_or("");
    let ip = local_address().unwrap_or(std::net::Ipv4Addr::new(192, 168, 0, 2));
    let o = ip.octets();
    let value = match name {
        // 0: link up (another value: the game stops asking)
        "query_nic_status" => Some(("status", "0".to_string())),
        // 0..4; 3: the address obtained (the game then asks for the card's state)
        "query_dhcp_status" => Some(("dhcp_status", "3".to_string())),
        "query_ip_address" => Some(("ip_address", ip.to_string())),
        "query_subnetmask" => Some(("subnetmask", "255.255.255.0".to_string())),
        "query_gateway" | "query_primary_dns" => {
            let key = if name == "query_gateway" { "gateway" } else { "primary_dns" };
            Some((key, format!("{}.{}.{}.1", o[0], o[1], o[2])))
        }
        "query_secondary_dns" => Some(("secondary_dns", "0.0.0.0".to_string())),
        "query_mac_address" => Some(("mac_address", "00:D0:F1:00:00:01".to_string())),
        _ => None,
    };
    match value {
        Some((key, value)) => format!("response={name}&result=success&{key}={value}"),
        None => format!("response={name}&result=success"),
    }
}

/// The other services (installer, game delivery, storage): `request=<name>&...` answered
/// `response=<name>&result=success`. The installer's application data area query carries its
/// state (unknown, checking, formatting, needed, available, restored, error) and, available,
/// the id of the game it holds: `available`, the keychip's game. Without them amInstall fails
/// (-6): Operation G.H.O.S.T. reads them on each boot but its first ("AMU::MainStorage::Init()
/// Faild", error 3005 "AMLIB INITIALIZE FAILED").
fn service_answer(request: &str) -> String {
    let name = request.split('&').find_map(|p| p.strip_prefix("request=")).unwrap_or("");
    match name {
        "query_appdata_status" => {
            let id = values().get("keychip.appboot.gameid").map_or("", String::as_str);
            format!("response={name}&result=success&status=available&id={id}")
        }
        _ => format!("response={name}&result=success"),
    }
}

/// The MAC for `keychip.ds.compute=<challenge hex>&page=<n>`, hex: the game's table entry for
/// its fixed challenge, else zero.
fn ds_compute(request: &str) -> String {
    let param = |name: &str| request.split('&').find_map(|p| p.strip_prefix(name)?.strip_prefix('='));
    let challenge = param("keychip.ds.compute").unwrap_or("");
    let page: usize = param("page").and_then(|p| p.parse().ok()).unwrap_or(0);
    let mut mac = [0u8; 20];
    let bytes: Vec<u8> = (0..challenge.len() / 2).filter_map(|i| u8::from_str_radix(challenge.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    if page < 4 && bytes.len() == 7 {
        if let Some((fixed, table)) = ds_table().filter(|(fixed, _)| *fixed == &bytes[..]) {
            let _ = fixed;
            mac.copy_from_slice(&table[page * 20..page * 20 + 20]);
        } else if let Some(record) = ds_file().chunks_exact(DS_RECORD).take(DS_RECORDS).find(|r| r[..7] == bytes[..]) {
            let stored = &record[7 + page * 20..7 + page * 20 + 20];
            for (m, &i) in mac.iter_mut().zip(MAC_ORDER.iter()) {
                *m = stored[i];
            }
        } else if let Some(at) = find_in_image(&bytes, true) {
            // a dump's own records: amDongle's 100 records (challenge + the 4 MACs, 87 bytes)
            // kept in its data as filled at dump time (Transformers' test menu uses them as is)
            let at = at + 7 + page * 20;
            mac.copy_from_slice(unsafe { std::slice::from_raw_parts(at as *const u8, 20) });
        } else {
            log!("pcp: keychip.ds.compute: unknown challenge {challenge}");
        }
    }
    hex(&mac)
}

/// The answer to `keychip.ssd.proof=<16-byte challenge>`, hex: the challenge file's SSD record
/// for it (after the DS records: 10000 records of 32 bytes, the challenge then the answer,
/// shuffled, see [`SSD_ORDER`]), else zero.
fn ssd_proof(challenge: &str) -> String {
    let bytes: Vec<u8> = (0..challenge.len() / 2).filter_map(|i| u8::from_str_radix(challenge.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    let mut answer = [0u8; 16];
    let records = ds_file().get(DS_RECORD * DS_RECORDS..).unwrap_or(&[]);
    match records.chunks_exact(SSD_RECORD).find(|r| bytes.len() == 16 && r[..16] == bytes[..]) {
        Some(record) => {
            for (a, &i) in answer.iter_mut().zip(SSD_ORDER.iter()) {
                *a = record[16 + i];
            }
        }
        // a dump's own SSD records (challenge + answer), as the DS ones
        None => match find_in_image(&bytes, true).filter(|_| bytes.len() == 16) {
            Some(at) => answer.copy_from_slice(unsafe { std::slice::from_raw_parts((at + 16) as *const u8, 16) }),
            None => log!("pcp: keychip.ssd.proof: unknown challenge {challenge}"),
        },
    }
    hex(&answer)
}

/// Answer byte `j` is byte `SSD_ORDER[j]` of the 16 answer bytes of its SSD record.
const SSD_ORDER: [usize; 16] = [6, 8, 4, 12, 7, 13, 1, 10, 2, 3, 11, 14, 15, 0, 5, 9];

/// Challenge file: records of the challenge and the 4 MACs.
const DS_RECORD: usize = 87;
const DS_RECORDS: usize = 10000;
/// MAC byte `j` is byte `MAC_ORDER[j]` of its 20 bytes in the challenge file (amDongle's loader).
const MAC_ORDER: [usize; 20] = [15, 5, 13, 14, 10, 1, 2, 11, 16, 7, 4, 18, 12, 6, 3, 0, 17, 8, 19, 9];

/// SSD records after the DS ones: the challenge then the answer.
const SSD_RECORD: usize = 32;
const SSD_RECORDS: usize = 10000;

/// The challenge file's path (`WAL_RINGEDGE_DS_FILE`, from the game directory: the current
/// directory at startup, the game changes it) and, when the dump lacks it, the generated
/// one the game gets instead (`save_dir`). Resolved by [`init_ds_file`].
static DS_PATH: OnceLock<Option<(std::path::PathBuf, Option<std::path::PathBuf>)>> = OnceLock::new();

/// Resolves the challenge file. A dump without it (SEGA Golden Gun: empty `fs\table`, amDongle
/// fails to set up, error 0949 "keychip not found") gets a generated one in `save_dir`: the
/// records are random, amDongle only checks the keychip's answers against them and the
/// keychip answers from the same file.
pub fn init_ds_file(save_dir: &str) {
    DS_PATH.get_or_init(|| {
        let name = std::env::var("WAL_RINGEDGE_DS_FILE").ok()?;
        let path = std::env::current_dir().map(|d| d.join(&name)).unwrap_or_else(|_| name.clone().into());
        if path.exists() {
            return Some((path, None));
        }
        let generated = std::path::Path::new(save_dir).join(path.file_name()?);
        if !generated.exists() {
            let size = DS_RECORD * DS_RECORDS + SSD_RECORD * SSD_RECORDS;
            // xorshift, seeded by the file name: the same file on every machine
            let mut x = name.bytes().fold(0x9E37_79B9_7F4A_7C15u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01B3));
            let data: Vec<u8> = (0..size).map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            }).collect();
            if let Err(e) = std::fs::write(&generated, data) {
                log!("pcp: cannot write {}: {e}", generated.display());
                return Some((path, None));
            }
        }
        log!("pcp: {} missing: generated {}", path.display(), generated.display());
        Some((path, Some(generated)))
    });
}

/// The generated challenge file for a path the game opens, when it is the missing one.
pub fn ds_file_replacement(path: &str) -> Option<&'static std::path::Path> {
    let (missing, generated) = DS_PATH.get()?.as_ref()?;
    let generated = generated.as_deref()?;
    let name = path.rsplit(['\\', '/']).next()?;
    let same = missing.file_name().is_some_and(|m| m.to_string_lossy().eq_ignore_ascii_case(name));
    // GetFileAttributesW, not Path::exists (it opens the file: CreateFileW, this hook's caller
    // when the system's CreateFileW is hot patched, packed executables)
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let exists = unsafe { windows_sys::Win32::Storage::FileSystem::GetFileAttributesW(wide.as_ptr()) } != u32::MAX;
    (same && !exists).then_some(generated)
}

/// The game's challenge file (`WAL_RINGEDGE_DS_FILE`, or the generated one), empty without it.
fn ds_file() -> &'static [u8] {
    static FILE: OnceLock<Vec<u8>> = OnceLock::new();
    FILE.get_or_init(|| {
        let Some(Some((path, generated))) = DS_PATH.get() else { return Vec::new() };
        let path = generated.as_ref().unwrap_or(path);
        match std::fs::read(path) {
            Ok(data) => {
                log!("pcp: challenge file {} ({} bytes)", path.display(), data.len());
                data
            }
            Err(e) => {
                log!("pcp: cannot read {}: {e}", path.display());
                Vec::new()
            }
        }
    })
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect()
}

/// amDongle's fixed challenge, the same in every game (its 4 MACs follow it, 12 bytes after).
const FIXED_CHALLENGE: [u8; 7] = [0x2C, 0xFE, 0xCB, 0xC7, 0x1C, 0xF1, 0xE4];

/// The game's fixed challenge (7 bytes) and the MACs of the 4 pages: found in the executable's
/// image (the same table in every game so far), else `WAL_RINGEDGE_DS_TABLE=<challenge
/// rva>,<macs rva>`. The scan also works in a game's other programs (its test menu).
fn ds_table() -> Option<(&'static [u8], &'static [u8])> {
    static TABLE: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let (challenge, macs) = (*TABLE.get_or_init(|| {
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
        if let Some(at) = find_in_image(&FIXED_CHALLENGE, false) {
            log!("pcp: keychip fixed challenge at +{:#x}", at - base);
            return Some((at, at + 12));
        }
        let spec = std::env::var("WAL_RINGEDGE_DS_TABLE").ok()?;
        let (c, m) = spec.split_once(',')?;
        let parse = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
        Some((base + parse(c)?, base + parse(m)?))
    }))?;
    // read-only data of the game executable, mapped for the process lifetime
    Some(unsafe { (std::slice::from_raw_parts(challenge as *const u8, 7), std::slice::from_raw_parts(macs as *const u8, 80)) })
}

/// The address of `needle` in the game executable's image (its headers' SizeOfImage): the
/// first or the `last` one.
fn find_in_image(needle: &[u8], last: bool) -> Option<usize> {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe {
        let pe = base + std::ptr::read_unaligned((base + 0x3C) as *const u32) as usize;
        if std::ptr::read_unaligned(pe as *const u32) != 0x4550 {
            return None;
        }
        let size = std::ptr::read_unaligned((pe + 24 + 56) as *const u32) as usize;
        let image = std::slice::from_raw_parts(base as *const u8, size);
        let mut matches = image.windows(needle.len()).enumerate().filter(|(_, w)| *w == needle).map(|(i, _)| base + i);
        if last { matches.last() } else { matches.next() }
    }
}

/// Arguments of the cabinet's application `n` (`WAL_RINGEDGE_FOREGROUND`).
fn foreground_args(n: &str) -> String {
    let spec = std::env::var("WAL_RINGEDGE_FOREGROUND").unwrap_or_else(|_| "3=-t".into());
    spec.split(';').filter_map(|item| item.split_once('=')).find(|(k, _)| k.trim() == n).map(|(_, v)| v.trim().to_string()).unwrap_or_default()
}
