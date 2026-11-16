//! crossbeats REV.'s game server (`https://rev-ent.ac.capcom.jp`), answered in ALL.Net's
//! server: the game's WinHTTP connection to it goes there, in plain HTTP (`cxb.rs`). A port of
//! ARTEMiS' `titles/cxb` (WTFPL: `index.py`, `base.py`, `rss2.py`, SUNRiSE S2, version 10104):
//! JSON requests, JSON answers. crossbeats REV. (its requests' versions up to 10102) gets
//! `rev.py`'s tables (`cxb/data/rev/`), its song list ARTEMiS' and the dump's, only the songs
//! and events its options hold.
//!
//! - `/data` (`{"dldate": {"filetype": "<version>/<folder>/<Name>"}}`): the server's tables,
//!   picked by the file's name (`ShopListDetail`, `Cs2001`: `cs` and 4 digits...); its data in
//!   `cxb/data/` (ARTEMiS' `data/rss2`, in UTF-8), the song list the dump's own
//!   (`resource\ArchiveList\MusicArchiveList.csv`: only its songs are in its options)
//! - `/auth` (`usercheck`, `entry`, `login`, `logout`): the Aime card's player (its id the
//!   user id), registered by its first save
//! - `/action` (`loadrange`, `saveindex`, `rankreg`, `sprankreq`, `addenergy`...): the player's
//!   records, kept in `Y:\cxb\<user id>.json` (each record's index and JSON text as saved; read
//!   back base64 encoded), with ARTEMiS' unlocks (coupons, shop titles and icons, stories)
//!
//! The server's lock covers a whole request.

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

static LOCK: Mutex<()> = Mutex::new(());

const DIR: &str = "Y:\\cxb";

/// The song list the server sends (the dump's, beside the executable).
const MUSIC_LIST: &str = "resource\\ArchiveList\\MusicArchiveList.csv";

fn data(name: &str) -> Option<&'static str> {
    Some(match name {
        "ItemList_Icon" => include_str!("data/ItemList_Icon.csv"),
        "ItemList_Title" => include_str!("data/ItemList_Title.csv"),
        "ShopList_Icon" => include_str!("data/ShopList_Icon.csv"),
        "ShopList_Music" => include_str!("data/ShopList_Music.csv"),
        "ShopList_Sale" => include_str!("data/ShopList_Sale.csv"),
        "ShopList_SkinBg" => include_str!("data/ShopList_SkinBg.csv"),
        "ShopList_SkinEffect" => include_str!("data/ShopList_SkinEffect.csv"),
        "ShopList_SkinNotes" => include_str!("data/ShopList_SkinNotes.csv"),
        "ShopList_Title" => include_str!("data/ShopList_Title.csv"),
        "ExtraStageList" => include_str!("data/ExtraStageList.csv"),
        "FreeCoupon" => include_str!("data/FreeCoupon.csv"),
        "NewsList" => include_str!("data/NewsList.csv"),
        "License" => include_str!("data/License.csv"),
        "CourseList" => include_str!("data/CourseList.csv"),
        "Cs2000" => include_str!("data/Cs2000.csv"),
        "Cs2001" => include_str!("data/Cs2001.csv"),
        "Cs2002" => include_str!("data/Cs2002.csv"),
        "Cs2003" => include_str!("data/Cs2003.csv"),
        "Cs2004" => include_str!("data/Cs2004.csv"),
        "Partner0000" => include_str!("data/Partner0000.csv"),
        _ => return None,
    })
}

/// crossbeats REV.'s tables (ARTEMiS' `data/rev`, in UTF-8).
fn rev_data(name: &str) -> Option<&'static str> {
    Some(match name {
        "MusicArchiveList" => include_str!("data/rev/MusicArchiveList.csv"),
        "ItemArchiveList_Icon" => include_str!("data/rev/ItemArchiveList_Icon.csv"),
        "ItemArchiveList_SkinNotes" => include_str!("data/rev/ItemArchiveList_SkinNotes.csv"),
        "ItemArchiveList_SkinEffect" => include_str!("data/rev/ItemArchiveList_SkinEffect.csv"),
        "ItemArchiveList_SkinBg" => include_str!("data/rev/ItemArchiveList_SkinBg.csv"),
        "ItemList_Title" => include_str!("data/rev/ItemList_Title.csv"),
        "ShopList_Music" => include_str!("data/rev/ShopList_Music.csv"),
        "ShopList_Icon" => include_str!("data/rev/ShopList_Icon.csv"),
        "ShopList_Title" => include_str!("data/rev/ShopList_Title.csv"),
        "ShopList_Sale" => include_str!("data/rev/ShopList_Sale.csv"),
        "ExtraStageList" => include_str!("data/rev/ExtraStageList.csv"),
        "Ex0000" => include_str!("data/rev/Ex0000.csv"),
        "Ex0001" => include_str!("data/rev/Ex0001.csv"),
        "NewsList" => include_str!("data/rev/NewsList.csv"),
        "License_Offline" => include_str!("data/rev/License_Offline.csv"),
        "CourseList" => include_str!("data/rev/CourseList.csv"),
        "Cs0000" => include_str!("data/rev/Cs0000.csv"),
        "Cs0001" => include_str!("data/rev/Cs0001.csv"),
        "Cs0002" => include_str!("data/rev/Cs0002.csv"),
        "Cs0003" => include_str!("data/rev/Cs0003.csv"),
        "Cs0004" => include_str!("data/rev/Cs0004.csv"),
        "Cs0005" => include_str!("data/rev/Cs0005.csv"),
        "Cs0006" => include_str!("data/rev/Cs0006.csv"),
        "Cs0007" => include_str!("data/rev/Cs0007.csv"),
        "Cs0008" => include_str!("data/rev/Cs0008.csv"),
        "Cs0009" => include_str!("data/rev/Cs0009.csv"),
        "MissionList" => include_str!("data/rev/MissionList.csv"),
        "EventArchiveList" => include_str!("data/rev/EventArchiveList.csv"),
        "EventStampList" => include_str!("data/rev/EventStampList.csv"),
        _ => return None,
    })
}

/// A table's lines, each ended by `\r\n`.
fn lines(text: &str) -> String {
    text.lines().map(|l| format!("{l}\r\n")).collect()
}

/// `{"data": text}`.
fn answer(text: impl Into<Json>) -> Json {
    Json::obj().with("data", text)
}

/// The dump's own song list (beside the executable).
fn dump_music_list() -> String {
    let file = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(MUSIC_LIST)));
    file.and_then(|f| std::fs::read(f).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// A song list's lines: their first 15 columns each.
fn song_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split(',').collect();
            let mut out: String = (0..15).map(|i| format!("{},", cols.get(i).copied().unwrap_or(""))).collect();
            out.push_str("\r\n");
            out
        })
        .collect()
}

/// The dump's song list: its first 15 columns each line.
fn music_list() -> String {
    song_lines(&dump_music_list())
}

/// The songs the options hold: their archives' names (`Option\NNNN\archive\music\<n>_m.arc`,
/// `n` the song code's CRC-32, inverted, in 10 digits).
fn option_songs() -> &'static std::collections::HashSet<String> {
    static SONGS: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    SONGS.get_or_init(|| {
        let folders = std::fs::read_dir("C:\\Mount\\Option").map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
        folders
            .iter()
            .filter_map(|f| std::fs::read_dir(f.join("archive").join("music")).ok())
            .flat_map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()))
            .filter_map(|n| n.strip_suffix("_m.arc").map(str::to_string))
            .collect()
    })
}

fn song_in_options(code: &str) -> bool {
    // the CRC-32 before its final inversion
    let inverted = code.bytes().fold(!0u32, |mut crc, b| {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
        crc
    });
    option_songs().contains(&format!("{inverted:010}"))
}

/// crossbeats REV.'s events: ARTEMiS', those the options hold (`archive\event\<id>.arc`) only.
/// The game asks each listed event's 10 tables and rankings at its boot, a few a second.
fn rev_event_list() -> String {
    let folders = std::fs::read_dir("C:\\Mount\\Option").map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
    let held = |id: &str| folders.iter().any(|f| f.join("archive").join("event").join(format!("{id}.arc")).exists());
    let events: String = rev_data("EventArchiveList").unwrap_or("").lines().filter(|l| l.split(',').next().is_some_and(held)).map(|l| format!("{l}\r\n")).collect();
    log!("cxb: REV events: {} in the options", events.lines().count());
    events
}

/// crossbeats REV.'s song list: ARTEMiS' (the last version's songs) and the dump's, those of
/// the songs its options hold only (a listed song without its archives is not played).
fn rev_music_list() -> String {
    let dump = dump_music_list();
    let mut seen = std::collections::HashSet::new();
    let text: Vec<&str> = rev_data("MusicArchiveList")
        .unwrap_or("")
        .lines()
        .chain(dump.lines())
        .filter(|l| l.split(',').next().is_some_and(|code| !code.trim().is_empty() && song_in_options(code) && seen.insert(code.to_string())))
        .collect();
    log!("cxb: REV song list: {} songs in the options", text.len());
    song_lines(&text.join("\n"))
}

/// `PathList` → `path_list`, `Cs2001` → `csxxxx` (ARTEMiS' handler names: inflection's
/// `underscore`, letters and 4 digits as `xxxx`).
fn handler_name(file: &str) -> String {
    let digits = file.bytes().rev().take_while(u8::is_ascii_digit).count();
    let letters = &file[..file.len() - digits];
    let (stem, suffix) = if digits == 4 && letters.bytes().all(|b| b.is_ascii_alphabetic()) { (letters, "xxxx") } else { (file, "") };
    let mut out = String::new();
    let b = stem.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 && (b[i - 1].is_ascii_lowercase() || b[i - 1].is_ascii_digit()) {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase() as char);
    }
    out + suffix
}

/// A `/data` request: the table its file names.
fn dldate(req: &Json) -> Json {
    let filetype = req.get("dldate").map_or("", |d| d.s("filetype")).to_string();
    let parts: Vec<&str> = filetype.split('/').collect();
    let file = parts.last().copied().unwrap_or("");
    let number = file.get(file.len().saturating_sub(4)..).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    // ARTEMiS' versions: up to 10102 crossbeats REV. (its own tables), later ones SUNRiSE
    if parts.len() >= 2 && parts[0].parse::<u32>().is_ok_and(|v| v <= 10102) {
        return rev_dldate(file, number);
    }
    let table = |name: &str| data(name).map(lines).unwrap_or_default();
    match handler_name(file).as_str() {
        "music_list" => answer(music_list()),
        "random_music_list" => answer(
            music_list().lines().filter_map(|l| l.split(',').next()).map(|code| format!("0,{code},{code},\r\n")).collect::<String>(),
        ),
        "item_list_detail" => answer(format!("#ItemListIcon\r\n{}\r\n#ItemListTitle\r\n{}", table("ItemList_Icon"), table("ItemList_Title"))),
        "shop_list_detail" => answer(
            [("ShopListIcon", "ShopList_Icon"), ("ShopListMusic", "ShopList_Music"), ("ShopListSale", "ShopList_Sale"), ("ShopListSkinBg", "ShopList_SkinBg"), ("ShopListSkinEffect", "ShopList_SkinEffect"), ("ShopListSkinNotes", "ShopList_SkinNotes"), ("ShopListTitle", "ShopList_Title")]
                .iter()
                .map(|(title, name)| format!("#{title}\r\n{}", table(name)))
                .collect::<Vec<_>>()
                .join("\r\n"),
        ),
        "extra_stage_list" => answer(table("ExtraStageList")),
        "free_coupon" => answer(table("FreeCoupon")),
        "news_list" => answer(table("NewsList")),
        "license" => answer(table("License")),
        "course_list" => answer(table("CourseList")),
        "csxxxx" => answer(table(&format!("Cs{number}"))),
        "partner_list" => {
            let mut s = String::new();
            for i in 0..13 {
                for (kind, value) in [(0, 10000), (1, 10500), (2, 10500)] {
                    s += &format!("8000{i:02},{i},{i},{kind},{value},,\r\n");
                }
            }
            s += "\r\n---\r\n0,150,100,100,100,100,\r\n";
            for i in 1..130 {
                s += &format!("{i},100,100,100,100,100,\r\n");
            }
            s += "---\r\n";
            answer(s)
        }
        "partnerxxxx" => answer(format!("{number},,{number},1,10000,\r\n{}", table("Partner0000"))),
        "server_state" => answer(true),
        "settings" => answer("2,\r\n"),
        "story_list" => answer(
            "\r\nst0000,RISING PURPLE,10104,1464370990,4096483201,Cs1000,-1,purple,\r\n\
             st0001,REBEL YELL,10104,1467999790,4096483201,Cs1000,-1,chaset,\r\n\
             st0002,REMNANT,10104,1502127790,4096483201,Cs1000,-1,overcl,\r\n",
        ),
        "stxxxx" => answer((1..=10).map(|i| format!("{i},st000{number}_{},,,,,,,,,,,,,,,,1,,-1,1,\r\n", i - 1)).collect::<String>()),
        "event_stamp_list" => answer("Cs1002,1,1,1,1,1,1,1,1,1,1,\r\n"),
        "premium_list" => answer("1,,,,10,,,,,99,,,,,,,,,100,,\r\n"),
        "event_list" => answer("Cs4001,0,10000,1601510400,1604188799,1,nv2006,1,\r\nCs4005,0,10000,1609459200,1615766399,1,nv2006,1,\r\n"),
        _ if filetype.contains("EventStampMapListCs1002") => answer("1,2,1,1,2,3,9,5,6,7,8,9,10,\r\n"),
        _ if filetype.contains("EventStampList") => answer("Cs1002,1,1,1,1,1,1,1,1,1,1,\r\n"),
        _ if parts.get(2).is_some_and(|p| p.contains("Cs4001")) => answer(
            "#EventMusicList\r\n1,zonzon2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n2,moonki,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n3,tricko,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n",
        ),
        _ if parts.get(2).is_some_and(|p| p.contains("Cs4005")) => answer(
            "#EventMusicList\r\n2,firstl,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n2,valent,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n2,dazzli2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,\r\n",
        ),
        // its parser skips a header line, then reads on to the end: without a line ending it
        // reads on past the text (the game hung at its boot, one time out of two)
        "unlimited_mission" => answer("\r\n"),
        // the others (PathList, Tips, MissionList, OneMoreExtraList...): empty
        _ => answer(""),
    }
}

/// A crossbeats REV. `/data` request (ARTEMiS' `rev.py`).
fn rev_dldate(file: &str, number: u32) -> Json {
    let table = |name: &str| rev_data(name).map(lines).unwrap_or_default();
    let titled = |title: &str, name: &str| answer(format!("\r\n#{title}\r\n{}", table(name)));
    match handler_name(file).as_str() {
        "music_list" => answer(rev_music_list()),
        "item_list_icon" => titled("ItemListIcon", "ItemArchiveList_Icon"),
        "item_list_skin_notes" => titled("ItemListSkinNotes", "ItemArchiveList_SkinNotes"),
        "item_list_skin_effect" => titled("ItemListSkinEffect", "ItemArchiveList_SkinEffect"),
        "item_list_skin_bg" => titled("ItemListSkinBg", "ItemArchiveList_SkinBg"),
        "item_list_title" => titled("ItemListTitle", "ItemList_Title"),
        "shop_list_music" => titled("ShopListMusic", "ShopList_Music"),
        "shop_list_icon" => titled("ShopListIcon", "ShopList_Icon"),
        "shop_list_title" => titled("ShopListTitle", "ShopList_Title"),
        "shop_list_sale" => titled("ShopListSale", "ShopList_Sale"),
        "extra_stage_list" => answer(table("ExtraStageList")),
        "exxxxx" => answer(table(&format!("Ex{number:04}"))),
        "news_list" => answer(table("NewsList")),
        "license" => answer(table("License_Offline")),
        "course_list" => answer(table("CourseList")),
        "csxxxx" => answer(table(&format!("Cs{number:04}"))),
        "mission_list" => answer(table("MissionList")),
        "event_list" => answer(rev_event_list()),
        "event_stamp_list" => answer(table("EventStampList")),
        "event_stamp_map_list_csxxxx" => answer("1,2,1,1,2,3,9,5,6,7,8,9,10,\r\n"),
        "server_state" => answer(true),
        // as SUNRiSE's: a line ending for its parser
        "unlimited_mission" => answer("\r\n"),
        // the others (PathList, the skins' shop lists, BonusList10100, Tips, the event
        // rankings...): empty
        _ => answer(""),
    }
}

// The players

fn path(user: &str) -> String {
    format!("{DIR}\\{user}.json")
}

/// A player's document: `records` (index: the record's JSON text), `ranks`, `energy`.
fn load(user: &str) -> Option<Json> {
    std::fs::read_to_string(path(user)).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_)))
}

fn save(user: &str, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(user));
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path(user));
    }
}

/// A user id from a request (a number or a string; digits only: a file name).
fn user_of(v: Option<&Json>) -> String {
    let s = match v {
        Some(Json::Str(s)) => s.clone(),
        Some(Json::Num(n)) => n.clone(),
        _ => String::new(),
    };
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// A JSON value's text (a string as is).
fn text_of(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => other.to_string(),
    }
}

fn records(doc: &Json) -> Vec<(u64, String)> {
    match doc.get("records") {
        Some(Json::Obj(fields)) => fields.iter().filter_map(|(k, v)| Some((k.parse().ok()?, text_of(v)))).collect(),
        _ => Vec::new(),
    }
}

/// The player's profile (record 0), parsed.
fn profile(doc: &Json) -> Option<Json> {
    records(doc).into_iter().find(|(i, _)| *i == 0).and_then(|(_, t)| Json::parse(&t))
}

fn b64(json: &str) -> Json {
    Json::str(wal_payload_common::allnet::base64_encode(json.as_bytes()))
}

fn auth(kind: &str, req: &Json) -> Json {
    let body = req.get(kind).cloned().unwrap_or(Json::obj());
    let user = user_of(body.get("authid"));
    match kind {
        "usercheck" => {
            let exists = load(&user).is_some_and(|d| profile(&d).is_some());
            log!("cxb: user {user} {}", if exists { "has a profile" } else { "is new" });
            Json::obj().with("exist", if exists { "true" } else { "false" }).with("logout", "true")
        }
        "entry" => Json::obj().with("token", user.clone()).with("uid", user),
        "login" if load(&user).is_some_and(|d| profile(&d).is_some()) => Json::obj().with("token", user.clone()).with("uid", user),
        "login" => Json::obj(),
        "logout" => Json::obj().with("auth", true),
        _ => Json::obj(),
    }
}

/// ARTEMiS' unlocks: the coupons (500-509), the shop's titles and icons, the stories.
fn unlocks(index: &mut Vec<Json>, out: &mut Vec<Json>) {
    let mut push = |i: u32, value: String| {
        index.push(Json::str(i.to_string()));
        out.push(b64(&value));
    };
    for i in 500..510 {
        push(i, format!(r#"{{"couponId":"{}","couponNum":"1","couponLog":[]}}"#, i - 500));
    }
    for i in (200000..201451).chain(202000..202264) {
        push(i, format!(r#"{{"shopId":{},"shopState":"2","isDisable":"t","isDeleted":"f","isSpecialFlag":"f"}}"#, i - 200000));
    }
    let states: String = (1..=16).map(|n| format!(r#","unlockState{n}":["t","t","t","t","t","t","t","t","t","t"]"#)).collect();
    for i in 900000..900003 {
        push(i, format!(r#"{{"storyId":{}{states}}}"#, i - 900000));
    }
}

fn load_range(body: &Json) -> Json {
    let range = body.a("range");
    let num = |v: Option<&Json>| v.map(text_of).and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
    let (start, end) = (num(range.first()), num(range.get(1)));
    if start == 800000 {
        return Json::obj().with("index", Json::num(start)).with("data", Vec::<Json>::new()).with("version", Json::num(10400));
    }
    let user = user_of(body.get("uid"));
    let doc = load(&user).unwrap_or(Json::obj());
    let (mut index, mut out) = (Vec::new(), Vec::new());
    let generated = |i: u64| (500..=510).contains(&i) || (200000..=210000).contains(&i) || (900000..=900200).contains(&i);
    let mut recs = records(&doc);
    recs.sort_by_key(|(i, _)| *i);
    for (i, text) in recs {
        if (start..=end).contains(&i) && !generated(i) {
            index.push(Json::str(i.to_string()));
            out.push(b64(&text));
        }
    }
    unlocks(&mut index, &mut out);
    let version = profile(&doc).map(|p| text_of(p.get("appVersion").unwrap_or(&Json::str("10400")))).unwrap_or_else(|| "10400".into());
    let versions: Vec<Json> = (0..index.len()).map(|_| Json::num(&version)).collect();
    log!("cxb: user {user}: records {start}-{end}: {} sent", index.len());
    Json::obj().with("index", index).with("data", out).with("version", versions)
}

fn save_index(body: &Json) -> Json {
    // SUNRiSE: `index` and `data` side by side; REV.: `data` alone, `[index, record, version]`
    // each (ARTEMiS' "REV Omnimix" form)
    let (indexes, values): (Vec<Json>, Vec<Json>) = if body.get("index").is_some() {
        (body.a("index").to_vec(), body.a("data").to_vec())
    } else {
        body.a("data").iter().filter_map(|e| Some((e.items().first()?.clone(), e.items().get(1)?.clone()))).unzip()
    };
    let (indexes, values) = (&indexes[..], &values[..]);
    let profile_user = indexes
        .iter()
        .position(|i| text_of(i) == "0")
        .and_then(|p| values.get(p))
        .and_then(|v| Json::parse(&text_of(v)))
        .map(|p| user_of(p.get("aimeId")));
    let user = Some(user_of(body.get("uid"))).filter(|u| !u.is_empty()).or(profile_user).unwrap_or_default();
    if user.is_empty() {
        return answer("");
    }
    let mut doc = load(&user).unwrap_or(Json::obj());
    if !matches!(doc.get("records"), Some(Json::Obj(_))) {
        doc.set("records", Json::obj());
    }
    let recs = doc.get_mut("records").expect("cxb: records");
    for (i, v) in indexes.iter().zip(values) {
        recs.set(&text_of(i), Json::str(text_of(v)));
    }
    save(&user, &doc);
    log!("cxb: user {user}: {} records saved", indexes.len());
    Json::obj()
}

fn action(kind: &str, req: &Json) -> Json {
    let body = req.get(kind).cloned().unwrap_or(Json::obj());
    match kind {
        "loadrange" => load_range(&body),
        "saveindex" => save_index(&body),
        "hitreq" => Json::obj().with("data", Vec::<Json>::new()),
        "getadv" => answer(vec![Json::obj().with("r", "1").with("i", "100300").with("c", "20")]),
        "getmsg" => Json::obj().with("msgs", Vec::<Json>::new()),
        "eventreq" => Json::obj().with("eventreq", ""),
        "stampreq" => Json::obj().with("stampreq", ""),
        "rankreg" => {
            let user = user_of(body.get("uid"));
            let mut doc = load(&user).unwrap_or(Json::obj());
            let mut ranks: Vec<Json> = doc.a("ranks").to_vec();
            for r in body.a("data") {
                let rid = text_of(r.get("rid").unwrap_or(&Json::Null));
                ranks.retain(|old| text_of(old.get("rid").unwrap_or(&Json::Null)) != rid);
                let mut rank = Json::obj().with("sc", r.get("sc").cloned().unwrap_or(Vec::<Json>::new().into())).with("rid", r.get("rid").cloned().unwrap_or(Json::Null));
                rank.set("clear", r.get("clear").cloned().unwrap_or(Json::num(0)));
                ranks.push(rank);
            }
            doc.set("ranks", ranks);
            save(&user, &doc);
            Json::obj()
        }
        "sprankreq" => {
            let user = user_of(body.get("uid"));
            let ranks = load(&user).map(|d| d.a("ranks").to_vec()).unwrap_or_default();
            Json::obj()
                .with("uid", body.get("uid").cloned().unwrap_or(Json::Null))
                .with("aid", body.get("aid").cloned().unwrap_or(Json::Null))
                .with("rank", ranks)
                .with("rankx", vec![Json::num(1), Json::num(1), Json::num(1)])
        }
        "addenergy" => {
            let user = user_of(body.get("uid"));
            let mut doc = load(&user).unwrap_or(Json::obj());
            let class = profile(&doc).map(|p| text_of(p.get("myClass").unwrap_or(&Json::str("0")))).unwrap_or_else(|| "0".into());
            let energy = doc.i("energy");
            let granted = if energy <= 995 { 5 } else { 0 };
            doc.set("energy", Json::num(energy + granted));
            save(&user, &doc);
            Json::obj().with("class", class).with("granted", granted.to_string()).with("total", (energy + granted).to_string()).with("threshold", "1000")
        }
        // rpreq...
        _ => Json::obj(),
    }
}

/// A request: its JSON answer.
fn title(path: &str, body: &[u8]) -> Vec<u8> {
    let _lock = LOCK.lock().unwrap();
    let text = String::from_utf8_lossy(body).into_owned();
    let req = Json::parse(&text).unwrap_or(Json::obj());
    let kind = match &req {
        Json::Obj(fields) => fields.first().map(|(k, _)| k.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    let route = path.split('?').next().unwrap_or("");
    let answer = if route.ends_with("/data") {
        dldate(&req)
    } else if route.ends_with("/auth") {
        auth(&kind, &req)
    } else if route.ends_with("/action") {
        action(&kind, &req)
    } else {
        Json::obj()
    };
    let out = answer.to_string();
    log!("cxb: {route} {} -> {}", text.chars().take(300).collect::<String>(), out.chars().take(200).collect::<String>());
    out.into_bytes()
}

pub(crate) fn init() {
    wal_payload_common::allnet::set_title(title);
}

