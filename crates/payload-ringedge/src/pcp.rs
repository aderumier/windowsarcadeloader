//! The RingEdge system services amLib talks to over PCP on 127.0.0.1, served by the payload:
//! `mxmaster` (amMaster: application control, TCP 40100) and `mxkeychip` (amDongle: the
//! keychip, TCP 40106, its binary data channel 40107).
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
//! `keychip.ds.compute=<challenge>&page=<n>`: the keychip's DS28CN01 SHA-1 MAC (20 bytes) of a
//! 7-byte challenge for one of its 4 pages. amDongle checks the answer against MACs it knows:
//! challenges from the game's challenge file (`SBWX_Table.dat`: 10000 records of 87 bytes, the
//! challenge then the MACs of the 4 pages, each stored with its bytes shuffled, see
//! [`MAC_ORDER`]; then the SSD records) and a fixed one in the executable, followed by its 4
//! MACs. Both are looked up: `WAL_RINGEDGE_DS_FILE=<file>` (from the game directory) and
//! `WAL_RINGEDGE_DS_TABLE=<challenge rva>,<macs rva>` (read in the game's memory).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

use wal_payload_common::log;

const MXMASTER: u16 = 40100;
const MXKEYCHIP: u16 = 40106;
const MXKEYCHIP_DATA: u16 = 40107;

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
    ("mxmaster.develop", "0"),
];

fn values() -> &'static HashMap<String, String> {
    static VALUES: OnceLock<HashMap<String, String>> = OnceLock::new();
    VALUES.get_or_init(|| {
        let mut map: HashMap<String, String> = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
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

pub(crate) fn start() {
    // read before the first connection: an environment lookup in a hook-free thread is fine,
    // but keep the overrides logged once
    let _ = values();
    let _ = ds_table();
    let _ = ds_file();
    for (port, name) in [(MXMASTER, "mxmaster"), (MXKEYCHIP, "mxkeychip"), (MXKEYCHIP_DATA, "mxkeychip data")] {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                std::thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        std::thread::spawn(move || serve(stream, name, port == MXKEYCHIP_DATA));
                    }
                });
            }
            Err(e) => log!("pcp: cannot listen on {port} ({name}): {e}"),
        }
    }
    log!("pcp: mxmaster on {MXMASTER}, mxkeychip on {MXKEYCHIP}/{MXKEYCHIP_DATA}");
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
            let reply = answer(&request);
            log!("pcp: {name}: {request} -> {reply}");
            if stream.write_all(format!("{reply}\r\n>").as_bytes()).is_err() {
                return;
            }
        }
    }
    log!("pcp: {name}: client disconnected");
}

/// The reply to one request: its first key with the value.
fn answer(request: &str) -> String {
    let first = request.split('&').next().unwrap_or("");
    let (key, value) = first.split_once('=').unwrap_or((first, ""));
    if key == "mxmaster.foreground.next" {
        let args = foreground_args(value.trim());
        log!("pcp: next application {value}: relaunch with '{args}'");
        wal_payload_common::request_relaunch(&args);
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
        } else {
            log!("pcp: keychip.ds.compute: unknown challenge {challenge}");
        }
    }
    hex(&mac)
}

/// Challenge file: records of the challenge and the 4 MACs.
const DS_RECORD: usize = 87;
const DS_RECORDS: usize = 10000;
/// MAC byte `j` is byte `MAC_ORDER[j]` of its 20 bytes in the challenge file (amDongle's loader).
const MAC_ORDER: [usize; 20] = [15, 5, 13, 14, 10, 1, 2, 11, 16, 7, 4, 18, 12, 6, 3, 0, 17, 8, 19, 9];

/// The game's challenge file (`WAL_RINGEDGE_DS_FILE`), empty without it.
fn ds_file() -> &'static [u8] {
    static FILE: OnceLock<Vec<u8>> = OnceLock::new();
    FILE.get_or_init(|| {
        let Ok(name) = std::env::var("WAL_RINGEDGE_DS_FILE") else { return Vec::new() };
        // from the game directory: the current directory at startup (the game changes it)
        let path = std::env::current_dir().map(|d| d.join(&name)).unwrap_or_else(|_| name.clone().into());
        match std::fs::read(&path) {
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

/// The game's fixed challenge (7 bytes) and the MACs of the 4 pages (`WAL_RINGEDGE_DS_TABLE`).
fn ds_table() -> Option<(&'static [u8], &'static [u8])> {
    static TABLE: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let (challenge, macs) = (*TABLE.get_or_init(|| {
        let spec = std::env::var("WAL_RINGEDGE_DS_TABLE").ok()?;
        let (c, m) = spec.split_once(',')?;
        let parse = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
        Some((base + parse(c)?, base + parse(m)?))
    }))?;
    // read-only data of the game executable, mapped for the process lifetime
    Some(unsafe { (std::slice::from_raw_parts(challenge as *const u8, 7), std::slice::from_raw_parts(macs as *const u8, 80)) })
}

/// Arguments of the cabinet's application `n` (`WAL_RINGEDGE_FOREGROUND`).
fn foreground_args(n: &str) -> String {
    let spec = std::env::var("WAL_RINGEDGE_FOREGROUND").unwrap_or_else(|_| "3=-t".into());
    spec.split(';').filter_map(|item| item.split_once('=')).find(|(k, _)| k.trim() == n).map(|(_, v)| v.trim().to_string()).unwrap_or_default()
}
