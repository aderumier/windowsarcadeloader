//! Groove Coaster 2's card server (`WAL_NESICA_GAME=groovecoaster`): the NESYS card requests
//! (card select = read, card update = write, card insert = registration) of the NESiCA card
//! service, each a data type and an XML document `<root><info>...</info><data>...</data></root>`,
//! answered as the Groove Coaster card server does (GC-local-server-rewrite: its types and
//! answers, `/root/<element>` documents).
//!
//! The saves are files of `D:\groovecoaster\<card id>\`: `card.xml` (the player: name, total
//! score, flags), `detail_<pcol1>_<pcol2>_<pcol3>.xml` (a record per song and difficulty: scores,
//! clears, play counts), `bdata.xml` (the player's settings block); each the fields of the game's
//! last write. A card without `card.xml` is unregistered (status 23): the game offers the new
//! player's registration, which creates it. Everything is unlocked: the avatars, items, skins and
//! titles of the game's `data/boot` lists (their counts, the first 16 bits big-endian), the songs
//! of `data/boot/stage_param.dat`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use wal_payload_common::log;

/// Card statuses: done, an unregistered card.
const OK: u32 = 1;
const NOT_REGISTERED: u32 = 23;

const READ_CARD: u32 = 259;
const READ_DETAIL: u32 = 260;
const READ_DETAILS: u32 = 261;
const READ_BDATA: u32 = 264;
const READ_AVATAR: u32 = 418;
const READ_ITEM: u32 = 420;
const READ_SKIN: u32 = 422;
const READ_TITLE: u32 = 424;
const READ_MUSIC: u32 = 428;
const READ_EVENT_REWARD: u32 = 441;
const READ_NAVIGATOR: u32 = 443;
const READ_MUSIC_EXTRA: u32 = 465;
const READ_MUSIC_AOU: u32 = 467;
const READ_COIN: u32 = 468;
const GET_SESSION: u32 = 401;
const START_SESSION: u32 = 402;
const WRITE_CARD: u32 = 771;
const WRITE_DETAIL: u32 = 772;
const WRITE_BDATA: u32 = 776;

/// The coins of every card (the card server's default).
const COINS: u32 = 900000;

const DATE: &str = "2013-01-01 08:00:00";

/// The text of the first `<name>` element of `xml` (`<name/>`: empty).
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(&xml[start..end])
}

/// The inner XML of `<data>` (the game's fields), without its `card_id`.
fn data_fields(xml: &str) -> String {
    let inner = tag(xml, "data").unwrap_or("");
    let mut out = String::new();
    let mut rest = inner;
    // drop <card_id>...</card_id> and <card_id/>
    while let Some(i) = rest.find("<card_id") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let skip = if after.starts_with("<card_id/>") {
            "<card_id/>".len()
        } else {
            after.find("</card_id>").map_or(after.len(), |e| e + "</card_id>".len())
        };
        rest = &after[skip..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

fn document(body: &str) -> Vec<u8> {
    format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<root>{body}</root>\r\n").into_bytes()
}

fn element(name: &str, fields: &str) -> String {
    if fields.is_empty() { format!("<{name} />") } else { format!("<{name}>{fields}</{name}>") }
}

fn records(list: &str, items: impl Iterator<Item = String>) -> String {
    let body: String = items.enumerate().map(|(i, f)| format!("<record id=\"{i}\">{f}</record>")).collect();
    element(list, &body)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn card_dir(card_id: &str) -> PathBuf {
    Path::new(&crate::drive::data_dir()).join("groovecoaster").join(card_id)
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn write(path: &Path, text: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, text) {
        log!("groovecoaster: cannot save {}: {e}", path.display());
    }
}

fn number(xml: &str, name: &str) -> i64 {
    tag(xml, name).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

fn detail_path(card_id: &str, xml: &str) -> PathBuf {
    let key = ["pcol1", "pcol2", "pcol3"].map(|k| number(xml, k));
    card_dir(card_id).join(format!("detail_{}_{}_{}.xml", key[0], key[1], key[2]))
}

/// A list's length: the first 16 bits (big-endian) of the game's `data/boot/<file>`.
fn count(file: &str) -> usize {
    std::fs::read(format!("data\\boot\\{file}")).ok().filter(|d| d.len() >= 2).map_or(0, |d| u16::from_be_bytes([d[0], d[1]]) as usize)
}

/// The songs of `data/boot/stage_param.dat`: id, title, artist. Records are a 32-bit id and
/// length-prefixed strings (title, short ASCII name, artist) followed by other fields: found by
/// that pattern, ids increasing.
fn songs() -> &'static Vec<(u32, String, String)> {
    static SONGS: OnceLock<Vec<(u32, String, String)>> = OnceLock::new();
    SONGS.get_or_init(|| {
        let d = std::fs::read("data\\boot\\stage_param.dat").unwrap_or_default();
        let string = |pos: usize, empty: bool| -> Option<(&[u8], usize)> {
            let len = *d.get(pos)? as usize;
            ((len > 0 || empty) && pos + 1 + len <= d.len()).then(|| (&d[pos + 1..pos + 1 + len], pos + 1 + len))
        };
        let text = |b: &[u8]| std::str::from_utf8(b).ok().filter(|t| t.chars().all(|c| c >= ' ')).map(str::to_string);
        let mut out = Vec::new();
        let (mut i, mut last) = (2usize, 0u32);
        while i + 8 < d.len() {
            let id = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
            let found = (d[i] == 0 && d[i + 1] == 0 && id > last).then_some(()).and_then(|_| {
                let (title, next) = string(i + 4, false)?;
                let title = text(title)?;
                let (short, next) = string(next, false)?;
                if !short.iter().all(|c| (0x21..0x7f).contains(c)) {
                    return None;
                }
                let (artist, next) = string(next, true)?;
                let artist = if artist.is_empty() { String::new() } else { text(artist)? };
                Some((title, artist, next))
            });
            match found {
                Some((title, artist, next)) => {
                    out.push((id, title, artist));
                    last = id;
                    i = next;
                }
                None => i += 1,
            }
        }
        log!("groovecoaster: {} songs in stage_param.dat", out.len());
        out
    })
}

/// An unlock list: `count` entries, ids from 1, all usable.
fn unlocks(list: &str, id_name: &str, card_id: &str, count: usize, extra: &str, date: &str) -> Vec<u8> {
    document(&records(
        list,
        (1..=count).map(|id| {
            format!(
                "<card_id>{card_id}</card_id><{id_name}>{id}</{id_name}>{extra}<created>{date}</created><modified>{date}</modified><new_flag>0</new_flag><use_flag>1</use_flag>"
            )
        }),
    ))
}

/// The session of a session read (the card server's fixed session).
fn session(card_id: &str, param: &str) -> Vec<u8> {
    let mac = tag(param, "mac_addr").unwrap_or("000000000000");
    document(&element(
        "session",
        &format!(
            "<card_id>{card_id}</card_id><mac_addr>{mac}</mac_addr><session_id>12345678901234567890123456789012</session_id><expires>9999</expires><player_id>1</player_id>"
        ),
    ))
}

/// A card select (read) of `data_type`: its status and answer, `None` for a type the server
/// does not know (the NESYS service's default answer).
pub(crate) fn select(card_id: &str, data_type: u32, param: &str) -> Option<(u32, Vec<u8>)> {
    let dir = card_dir(card_id);
    let registered = dir.join("card.xml").exists();
    let need_card = |answer: Vec<u8>| if registered { (OK, answer) } else { (NOT_REGISTERED, Vec::new()) };
    log!("groovecoaster: card {card_id} read {data_type}{}", if registered { "" } else { " (unregistered)" });
    let reply = match data_type {
        READ_CARD => {
            let fields = read(&dir.join("card.xml")).unwrap_or_default();
            need_card(document(&element("card", &format!("<card_id>{card_id}</card_id>{fields}"))))
        }
        READ_DETAIL => {
            let fields = read(&detail_path(card_id, param));
            let body = fields.map_or_else(String::new, |f| format!("<card_id>{card_id}</card_id>{f}"));
            need_card(document(&element("card_detail", &body)))
        }
        READ_DETAILS => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map(|e| e.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("detail_"))).collect())
                .unwrap_or_default();
            files.sort();
            let list = files.iter().filter_map(|p| read(p)).map(|f| format!("<card_id>{card_id}</card_id>{f}"));
            need_card(document(&records("card_detail", list)))
        }
        READ_BDATA => {
            let body = read(&dir.join("bdata.xml")).map_or_else(String::new, |f| format!("<card_id>{card_id}</card_id>{f}"));
            need_card(document(&element("card_bdata", &body)))
        }
        READ_AVATAR => (OK, unlocks("avatar", "avatar_id", card_id, count("player.dat"), "", DATE)),
        READ_ITEM => (OK, unlocks("item", "item_id", card_id, count("item.dat"), "<item_num>90</item_num>", "2013-01-01")),
        READ_SKIN => (OK, unlocks("skin", "skin_id", card_id, count("skin.dat"), "", DATE)),
        READ_TITLE => (OK, unlocks("title", "title_id", card_id, count("title.dat"), "", DATE)),
        READ_NAVIGATOR => (OK, document(&element("navigator", ""))),
        READ_EVENT_REWARD => (OK, document(&element("event_reward", ""))),
        READ_MUSIC => (
            OK,
            document(&records(
                "music",
                songs().iter().map(|(id, title, artist)| {
                    format!(
                        "<music_id>{id}</music_id><title>{}</title><artist>{}</artist><release_date>2021-01-01</release_date><end_date>2030-01-01</end_date><new_flag>0</new_flag><use_flag>1</use_flag><calc_flag>0</calc_flag>",
                        escape(title),
                        escape(artist)
                    )
                }),
            )),
        ),
        READ_MUSIC_EXTRA => (OK, document(&element("music_extra", ""))),
        READ_MUSIC_AOU => (OK, document(&element("music_aou", ""))),
        READ_COIN => (
            OK,
            document(&element(
                "coin",
                &format!("<card_id>{card_id}</card_id><current>{COINS}</current><total>{COINS}</total><monthly>{COINS}</monthly><created>{DATE}</created><modified>{DATE}</modified>"),
            )),
        ),
        GET_SESSION | START_SESSION => (OK, session(card_id, param)),
        _ => {
            log!("groovecoaster: card read type {data_type} not answered");
            return None;
        }
    };
    Some(reply)
}

/// A card update (write) of `data_type` with the game's document: its status. The card's
/// player and the song records are saved; the other writes (unlocks, settings of the lists) are
/// acknowledged, everything being unlocked.
pub(crate) fn update(card_id: &str, data_type: u32, xml: &str) -> u32 {
    let dir = card_dir(card_id);
    if !dir.join("card.xml").exists() {
        return NOT_REGISTERED;
    }
    log!("groovecoaster: card {card_id} write {data_type}");
    match data_type {
        WRITE_CARD => {
            // the player's name and creation date stay those of the registration
            let saved = read(&dir.join("card.xml")).unwrap_or_default();
            let new = data_fields(xml);
            let keep = |name: &str| tag(&saved, name).map(|v| format!("<{name}>{v}</{name}>"));
            let mut fields = String::new();
            for name in ["player_name", "score_i1", "fcol1", "fcol2", "fcol3", "achieve_status", "created", "modified"] {
                let value = match name {
                    "player_name" | "created" => keep(name),
                    "modified" => Some(format!("<modified>{}</modified>", now())),
                    _ => tag(&new, name).map(|v| format!("<{name}>{v}</{name}>")).or_else(|| keep(name)),
                };
                fields.push_str(&value.unwrap_or_else(|| format!("<{name}></{name}>")));
            }
            write(&dir.join("card.xml"), &fields);
        }
        WRITE_DETAIL => write(&detail_path(card_id, xml), &data_fields(xml)),
        WRITE_BDATA => write(&dir.join("bdata.xml"), &data_fields(xml)),
        _ => log!("groovecoaster: card write type {data_type} acknowledged"),
    }
    OK
}

/// A card insert (the new player's registration), the game's document: the player saved, its
/// fields as the game sent them (`fcol3` is the game's version, 20601: a card without it is a
/// RHYTHMVADERS EX one, whose data the game offers to transfer).
pub(crate) fn register(card_id: &str, xml: &str) -> u32 {
    let new = data_fields(xml).replace("<achieve_status/>", "<achieve_status></achieve_status>");
    let time = now();
    write(&card_dir(card_id).join("card.xml"), &format!("{new}<created>{time}</created><modified>{time}</modified>"));
    log!("groovecoaster: card {card_id} registered (player {:?})", tag(&new, "player_name").unwrap_or(""));
    OK
}

/// The local time, `yyyy-mm-dd hh:mm:ss`.
fn now() -> String {
    let t = unsafe {
        let mut t = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        t
    };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}
