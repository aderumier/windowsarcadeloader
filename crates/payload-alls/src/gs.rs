//! The game's ABAAS game server (Sega World Drivers Championship's player data: Aime card
//! logins, saves, rankings): the game posts to `http://<ALL.Net's uri>:80/api/...` (the uri a
//! bare host name, `WAL_ALLS_TITLE_URI`), resolved to the virtual router; the game's connections
//! to its port 80 go to this server in the game's own process, 127.0.0.1:`WAL_PORT`+104
//! (`allnet.rs`), answered by payload-common's `abaas_gs` (the players' files in `Y:\gs\`).
//! The title key comes from the game's `BCryptGenerateSymmetricKey` call.
//!
//! The title data its servers held (`data/load`'s `free_buckets`, [`title_data`]): each value a
//! JSON string holding JSON, its numbers as strings, its lists of numbers plain (the game turns
//! `[` `]` into Unreal's text lists), its fields the game's reflected structures'. Without a
//! season the power-on test crashes (an event's empty round list), without the time sale period
//! the top menu (a division by zero), without the penalty limits a new player gets the
//! "inconsistent play records" warning.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{abaas_gs, iat, log};

pub(crate) fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 104
}

static GENERATE_KEY: AtomicUsize = AtomicUsize::new(0);

type GenerateKey = unsafe extern "system" fn(usize, *mut usize, *mut u8, u32, *const u8, u32, u32) -> i32;

unsafe extern "system" fn generate_key(alg: usize, key: *mut usize, object: *mut u8, object_len: u32, secret: *const u8, secret_len: u32, flags: u32) -> i32 {
    if !secret.is_null() {
        // a 256-bit key: the title key
        abaas_gs::learn_key(unsafe { std::slice::from_raw_parts(secret, secret_len as usize) });
    }
    let real: GenerateKey = unsafe { std::mem::transmute(GENERATE_KEY.load(Ordering::Relaxed)) };
    unsafe { real(alg, key, object, object_len, secret, secret_len, flags) }
}

fn client(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    // Initial D ARCADE STAGE Zero's news and error pages (`idz.rs`)
    if let Some(text) = crate::idz::news(&path) {
        let mut out = stream;
        out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len()).as_bytes())?;
        return out.write_all(text.as_bytes());
    }
    // `/api/...` (after any prefix)
    let api = path.find("/api/").map_or(path.as_str(), |at| &path[at..]);
    let reply = abaas_gs::reply(api, &body);
    let mut out = stream;
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).as_bytes())?;
    out.write_all(&reply)
}

fn serve() {
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("gs: server on 127.0.0.1:{}", port());
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = client(stream);
                });
            }
        }
        Err(e) => log!("gs: 127.0.0.1:{}: {e}", port()),
    }
}

/// One season running from 2019 to 2100 (`NetSeasonSettingData`: four rounds), its time sale
/// (the bonus course) changing every hour.
const SEASON: &str = r#"{"ID":"1","year":"2019","no":"1","primary_year":"2019","primary_no":"1","time_start":"1546300800","time_end":"4102444800","rounds":[1,2,3,4],"primary_rounds":[1,2,3,4]}"#;
const RECOMMEND: &str = r#"{"scale_percent":"150","update_time":"60"}"#;
/// One closed event, ended in 2019 (`event_setting1`: `NetEventSettingBaseData`,
/// `event_setting2`: `NetEventSettingDateTimeData`).
const EVENT_BASE: &str = r#"{"ID":"1","year":"2019","rounds":[1],"primary_rounds":[1],"grid_sort_rule":"0","car_class":"0","team_category":"0","teams_num":"0","is_close":"1"}"#;
const EVENT_TIME: &str = r#"{"view_time_start":"1546300800","view_time_end":"1548979200","time_start":"1546300800","time_end":"1548979200","time_exec":"1548979200","ranking_id":"0"}"#;

/// Sega World Drivers Championship's title data, as JSON values.
fn title_data(key: &str) -> Option<String> {
    let json_string = |v: &str| format!("\"{}\"", v.replace('"', "\\\""));
    Some(match key {
        "season_setting" => json_string(SEASON),
        "season_recommend_setting" => json_string(RECOMMEND),
        "event_setting1" => json_string(EVENT_BASE),
        "event_setting2" => json_string(EVENT_TIME),
        "penalty_apply_limit_count" | "penalty_recovery_limit_count" => r#""100""#.into(),
        _ => return None,
    })
}

pub(crate) fn init() {
    abaas_gs::configure("Y:\\gs", "WAL_ALLS");
    abaas_gs::set_title_data(title_data);
    match unsafe { iat::hook("bcrypt.dll", "BCryptGenerateSymmetricKey", generate_key as *const () as usize) } {
        Some(orig) => GENERATE_KEY.store(orig, Ordering::Relaxed),
        None => log!("gs: BCryptGenerateSymmetricKey not imported"),
    }
    std::thread::spawn(serve);
}

/// The server only (Initial D ARCADE STAGE Zero's news pages, no ABAAS title).
pub(crate) fn init_plain() {
    std::thread::spawn(serve);
}
