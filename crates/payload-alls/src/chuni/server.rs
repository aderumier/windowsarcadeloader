//! CHUNITHM's title server (the Aime card players' data): the game posts zlib-packed JSON to
//! `<ALL.Net uri>ChuniServlet/<Api>` (the uri `http://<router>/SDBT/<version>/`, `WAL_ALLS_TITLE_URI`:
//! `120` STAR, `125` STAR PLUS),
//! answered in ALL.Net's server (`allnet.rs`) the same way packed. The answers follow ARTEMiS's
//! CHUNITHM title server (WTFPL, gitea.tendokyu.moe/Dniel97/artemis, `titles/chuni/base.py`,
//! `star.py`, `starplus.py`: their requests are not encrypted).
//!
//! The players are kept in `Y:\chuni\` (one JSON document per Aime user id): the game's own
//! records as it sent them (`userData`, `userDataEx`, `userGameOption`, `userGameOptionEx`,
//! `userRecentRatingList`, and the lists merged by their ids: characters, items, maps, courses,
//! duels, activities, charges, music details). The game's events and charge items are listed
//! (the events but the news, type 1, each a screen at login: `WAL_CHUNI_NEWS=1` lists them too)
//! from its data (`data\A000` and the options' folders: `event*\Event.xml`,
//! `chargeItem*\ChargeItem.xml`). No login bonuses, rankings, teams or rivals.

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

const DIR: &str = "Y:\\chuni";

/// Held for each request (load, change, save).
static LOCK: Mutex<()> = Mutex::new(());

/// The game's data folders (from its working directory, `App\bin`): its own and the options'.
fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![std::path::PathBuf::from("..\\data\\A000")];
    if let Ok(options) = std::fs::read_dir("C:\\mount\\Option") {
        let mut options: Vec<_> = options.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        options.sort();
        dirs.extend(options);
    }
    dirs
}

/// The first `<tag>...<id>N</id>` of an XML document.
fn xml_id(xml: &str, tag: &str) -> Option<i64> {
    let at = xml.find(&format!("<{tag}>"))?;
    let rest = &xml[at..];
    let id = rest.find("<id>")? + 4;
    rest[id..].split('<').next()?.trim().parse().ok()
}

/// The first `<tag>N</tag>` after `<after>`.
fn xml_value_after(xml: &str, after: &str, tag: &str) -> Option<i64> {
    let at = xml.find(&format!("<{after}>"))?;
    let rest = &xml[at..];
    let v = rest.find(&format!("<{tag}>"))? + tag.len() + 2;
    rest[v..].split('<').next()?.trim().parse().ok()
}

/// The data's `<kind>*\<file>` documents.
fn data_files(kind: &str, file: &str) -> Vec<String> {
    let mut out = Vec::new();
    for dir in data_dirs() {
        let Ok(entries) = std::fs::read_dir(dir.join(kind)) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path().join(file)).collect();
        entries.sort();
        out.extend(entries.iter().filter_map(|p| std::fs::read_to_string(p).ok()));
    }
    out
}

/// A value the game sent as text (its numbers are strings) or a number.
fn text(v: Option<&Json>) -> String {
    match v {
        Some(Json::Str(s)) => s.clone(),
        Some(Json::Num(n)) => n.clone(),
        Some(Json::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn num(v: Option<&Json>) -> i64 {
    text(v).parse().unwrap_or(0)
}

/// A name the game sent byte by byte (each UTF-8 byte a character): its text (ARTEMiS's
/// `read_wtf8`), sent back as UTF-8.
fn read_wtf8(s: &str) -> String {
    if s.chars().all(|c| (c as u32) < 0x100) {
        let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
        if let Ok(t) = String::from_utf8(bytes) {
            return t;
        }
    }
    s.to_string()
}

fn path(user: &str) -> String {
    format!("{DIR}\\{user}.json")
}

fn load(user: &str) -> Json {
    std::fs::read_to_string(path(user)).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_))).unwrap_or(Json::obj())
}

fn save(user: &str, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(user));
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path(user));
    }
}

/// The stored lists and the fields identifying their entries.
const LISTS: [(&str, &[&str]); 8] = [
    ("userCharacterList", &["characterId"]),
    ("userItemList", &["itemKind", "itemId"]),
    ("userMapList", &["mapId"]),
    ("userCourseList", &["courseId"]),
    ("userDuelList", &["duelId"]),
    ("userActivityList", &["kind", "id"]),
    ("userChargeList", &["chargeId"]),
    ("userMusicDetailList", &["musicId", "level"]),
];

/// Merges `entries` into the document's `list` (an entry with the same ids replaced).
fn merge(doc: &mut Json, list: &str, keys: &[&str], entries: &[Json]) {
    let mut items = doc.a(list).to_vec();
    for e in entries {
        let id = |j: &Json| keys.iter().map(|k| text(j.get(k))).collect::<Vec<_>>();
        match items.iter_mut().find(|i| id(i) == id(e)) {
            Some(i) => *i = e.clone(),
            None => items.push(e.clone()),
        }
    }
    doc.set(list, items);
}

/// One page of `items` from `next` (`maxCount` long): the page and the next index (-1: none).
fn page(items: &[Json], next: i64, max: i64) -> (Vec<Json>, i64) {
    let start = (next.max(0) as usize).min(items.len());
    let end = (start + max.max(1) as usize).min(items.len());
    (items[start..end].to_vec(), if end < items.len() { end as i64 } else { -1 })
}

fn ok() -> Json {
    Json::obj().with("returnCode", "1")
}

/// The game's data version from the request path's version (`/SDBT/125/...`: `1.25.00`; STAR's
/// `1.20.00` by default).
fn data_version(path: &str) -> String {
    let v = path.split('/').filter_map(|p| p.parse::<u32>().ok()).next().unwrap_or(120);
    format!("{}.{:02}.00", v / 100, v % 100)
}

fn game_setting(path: &str) -> Json {
    // the reboot time: 6-7 hours from now (UTC), as ARTEMiS's default
    let now = crate::clock::unix_now() as i64;
    let at = |secs: i64| {
        let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + (month <= 2) as i64;
        format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
    };
    let setting = Json::obj()
        .with("dataVersion", data_version(path))
        .with("isMaintenance", "false")
        .with("requestInterval", 10i64)
        .with("rebootStartTime", at(now + 6 * 3600))
        .with("rebootEndTime", at(now + 7 * 3600))
        .with("isBackgroundDistribute", "false")
        .with("maxCountCharacter", 300i64)
        .with("maxCountItem", 300i64)
        .with("maxCountMusic", 300i64);
    Json::obj().with("gameSetting", setting).with("isDumpUpload", "false").with("isAou", "false")
}

fn game_events(req: &Json) -> Json {
    // each option ships the earlier ones' events again (PARADISE LOST: 3065 files, 325 events;
    // the game failed to read the answer listing every copy)
    let mut seen = std::collections::HashSet::new();
    let events: Vec<Json> = data_files("event", "Event.xml")
        .iter()
        .filter_map(|xml| {
            let id = xml_id(xml, "name").filter(|&id| seen.insert(id))?;
            let kind = xml_value_after(xml, "substances", "type").unwrap_or(0);
            // the news (type 1, information): a screen each at login, unless `WAL_CHUNI_NEWS=1`
            if kind == 1 && std::env::var("WAL_CHUNI_NEWS").map_or(true, |v| v.trim() != "1") {
                return None;
            }
            (id > 0).then(|| Json::obj().with("id", id).with("type", kind).with("startDate", "2017-12-05 07:00:00").with("endDate", "2099-12-31 00:00:00"))
        })
        .collect();
    Json::obj().with("type", text(req.get("type"))).with("length", events.len()).with("gameEventList", events)
}

fn game_charges() -> Json {
    let mut seen = std::collections::HashSet::new();
    let charges: Vec<Json> = data_files("chargeItem", "ChargeItem.xml")
        .iter()
        .filter_map(|xml| xml_id(xml, "name"))
        .filter(|&id| seen.insert(id))
        .enumerate()
        .map(|(order, id)| {
            Json::obj()
                .with("orderId", order)
                .with("chargeId", id)
                .with("price", 1i64)
                .with("startDate", "2017-12-05 07:00:00.0")
                .with("endDate", "2099-12-31 00:00:00.0")
                .with("salePrice", 1i64)
                .with("saleStartDate", "2017-12-05 07:00:00.0")
                .with("saleEndDate", "2099-12-31 00:00:00.0")
        })
        .collect();
    Json::obj().with("length", charges.len()).with("gameChargeList", charges)
}

fn user_preview(user: &str, doc: &Json) -> Json {
    let Some(data) = doc.get("userData") else { return ok() };
    let option = doc.get("userGameOption").cloned().unwrap_or(Json::obj());
    let character_id = text(data.get("characterId"));
    let character = doc.a("userCharacterList").iter().find(|c| text(c.get("characterId")) == character_id).cloned().unwrap_or(Json::obj());
    let field = |k: &str| data.get(k).cloned().unwrap_or(Json::Null);
    Json::obj()
        .with("userId", user)
        .with("isLogin", false)
        .with("lastLoginDate", field("lastPlayDate"))
        .with("userName", field("userName"))
        .with("reincarnationNum", field("reincarnationNum"))
        .with("level", field("level"))
        .with("exp", field("exp"))
        .with("playerRating", field("playerRating"))
        .with("lastGameId", field("lastGameId"))
        .with("lastRomVersion", field("lastRomVersion"))
        .with("lastDataVersion", field("lastDataVersion"))
        .with("lastPlayDate", field("lastPlayDate"))
        .with("trophyId", field("trophyId"))
        .with("nameplateId", field("nameplateId"))
        .with("userCharacter", character)
        .with("playerLevel", option.get("playerLevel").cloned().unwrap_or(Json::Null))
        .with("rating", option.get("rating").cloned().unwrap_or(Json::Null))
        .with("headphone", option.get("headphone").cloned().unwrap_or(Json::Null))
        .with("chargeState", 1i64)
        .with("userNameEx", field("userName"))
}

/// The music details, one entry per music (its levels' details), `maxCount` musics a page.
fn user_music(user: &str, doc: &Json, next: i64, max: i64) -> Json {
    let mut details = doc.a("userMusicDetailList").to_vec();
    details.sort_by_key(|d| (num(d.get("musicId")), num(d.get("level"))));
    let mut musics: Vec<Vec<Json>> = Vec::new();
    for d in details {
        match musics.last_mut() {
            Some(m) if text(m[0].get("musicId")) == text(d.get("musicId")) => m.push(d),
            _ => musics.push(vec![d]),
        }
    }
    let (musics, next) = page(&musics.into_iter().map(Json::from).collect::<Vec<_>>(), next, max);
    let list: Vec<Json> = musics.into_iter().map(|m| Json::obj().with("length", m.items().len()).with("userMusicDetailList", m)).collect();
    Json::obj().with("userId", user).with("length", list.len()).with("nextIndex", next).with("userMusicList", list)
}

/// The items of the kind in `nextIndex` (kind * 10^10 + index).
fn user_items(user: &str, doc: &Json, next: i64, max: i64) -> Json {
    let (kind, index) = (next / 10_000_000_000, next % 10_000_000_000);
    let items: Vec<Json> = doc.a("userItemList").iter().filter(|i| num(i.get("itemKind")) == kind).cloned().collect();
    let (items, next) = page(&items, index, max);
    let next = if next < 0 { -1 } else { kind * 10_000_000_000 + next };
    Json::obj().with("userId", user).with("nextIndex", next).with("itemKind", kind).with("length", items.len()).with("userItemList", items)
}

/// A paged list (characters, courses).
fn user_paged(user: &str, doc: &Json, stored: &str, answer: &str, next: i64, max: i64) -> Json {
    let (items, next) = page(doc.a(stored), next, max);
    Json::obj().with("userId", user).with("length", items.len()).with("nextIndex", next).with(answer, items)
}

/// A whole list.
fn user_list(user: &str, doc: &Json, stored: &str, answer: &str) -> Json {
    let items = doc.a(stored).to_vec();
    Json::obj().with("userId", user).with("length", items.len()).with(answer, items)
}

fn upsert_all(user: &str, doc: &mut Json, upsert: &Json) {
    for record in ["userData", "userDataEx", "userGameOption", "userGameOptionEx"] {
        if let Some(first) = upsert.a(record).first() {
            let mut first = first.clone();
            if record == "userData" {
                let name = read_wtf8(first.s("userName"));
                first.set("userName", name);
            }
            doc.set(record, first);
        }
    }
    if let Some(recent) = upsert.get("userRecentRatingList") {
        doc.set("userRecentRatingList", recent.clone());
    }
    for (list, keys) in LISTS {
        let entries = upsert.a(list);
        if !entries.is_empty() {
            merge(doc, list, keys, entries);
        }
    }
    log!("chuni: user {user} saved");
}

fn answer(path: &str, api: &str, req: &Json) -> Json {
    let user = text(req.get("userId"));
    let next = num(req.get("nextIndex"));
    let max = num(req.get("maxCount"));
    // a guest's play (a user id with these bits: ARTEMiS's check) is not kept
    let guest = user.parse::<u64>().is_ok_and(|u| u & 0x1000000000001 == 0x1000000000001);
    let _lock = LOCK.lock().unwrap();
    let doc = if user.is_empty() || guest { Json::obj() } else { load(&user) };
    let empty = |list: &str| Json::obj().with("type", text(req.get("type"))).with("length", 0i64).with(list, Vec::<Json>::new());
    match api {
        "GameLoginApi" | "GameLogoutApi" | "UpsertClientBookkeepingApi" | "UpsertClientDevelopApi" | "UpsertClientErrorApi" | "UpsertClientSettingApi" | "UpsertClientTestmodeApi" => ok(),
        "GetGameSettingApi" => game_setting(path),
        "GetGameEventApi" => game_events(req),
        "GetGameChargeApi" => game_charges(),
        "GetGameIdlistApi" => empty("gameIdlistList"),
        "GetGameSaleApi" => empty("gameSaleList"),
        "GetGameRankingApi" => Json::obj().with("type", text(req.get("type"))).with("gameRankingList", Vec::<Json>::new()),
        "GetGameMessageApi" => {
            let message = Json::obj()
                .with("id", 1i64)
                .with("type", 1i64)
                .with("message", "Welcome to WindowsArcadeLoader!")
                .with("startDate", "2017-12-05 07:00:00.0")
                .with("endDate", "2099-12-31 00:00:00.0");
            Json::obj().with("type", text(req.get("type"))).with("length", 1i64).with("gameMessageList", vec![message])
        }
        "GetUserPreviewApi" => user_preview(&user, &doc),
        "GetUserDataApi" => match doc.get("userData") {
            Some(d) => Json::obj().with("userId", user.as_str()).with("userData", d.clone()),
            None => Json::obj(),
        },
        "GetUserDataExApi" => match doc.get("userDataEx") {
            Some(d) => Json::obj().with("userId", user.as_str()).with("userDataEx", d.clone()),
            None => Json::obj(),
        },
        "GetUserOptionApi" => Json::obj().with("userId", user.as_str()).with("userGameOption", doc.get("userGameOption").cloned().unwrap_or(Json::obj())),
        "GetUserOptionExApi" => Json::obj().with("userId", user.as_str()).with("userGameOptionEx", doc.get("userGameOptionEx").cloned().unwrap_or(Json::obj())),
        "GetUserCharacterApi" => user_paged(&user, &doc, "userCharacterList", "userCharacterList", next, max),
        "GetUserCourseApi" => user_paged(&user, &doc, "userCourseList", "userCourseList", next, max),
        "GetUserItemApi" => user_items(&user, &doc, next, max),
        "GetUserMusicApi" => user_music(&user, &doc, next, max),
        "GetUserMapApi" => user_list(&user, &doc, "userMapList", "userMapList"),
        "GetUserDuelApi" => user_list(&user, &doc, "userDuelList", "userDuelList"),
        "GetUserChargeApi" => user_list(&user, &doc, "userChargeList", "userChargeList"),
        "GetUserRecentRatingApi" => user_list(&user, &doc, "userRecentRatingList", "userRecentRatingList"),
        "GetUserActivityApi" => {
            let kind = text(req.get("kind"));
            let items: Vec<Json> = doc.a("userActivityList").iter().filter(|a| text(a.get("kind")) == kind).cloned().collect();
            Json::obj().with("userId", user.as_str()).with("length", items.len()).with("kind", num(req.get("kind"))).with("userActivityList", items)
        }
        "GetUserRecentPlayerApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("userRecentPlayerList", Vec::<Json>::new()),
        "GetUserFavoriteMusicApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("userFavoriteMusicList", Vec::<Json>::new()),
        "GetUserLoginBonusApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("userLoginBonusList", Vec::<Json>::new()),
        "GetUserRegionApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("userRegionList", Vec::<Json>::new()),
        "GetUserTeamApi" => Json::obj().with("userId", user.as_str()).with("teamId", 0i64),
        "UpsertUserAllApi" => {
            if !user.is_empty() && !guest {
                let mut doc = doc;
                upsert_all(&user, &mut doc, req.get("upsertUserAll").unwrap_or(&Json::Null));
                save(&user, &doc);
            }
            ok()
        }
        "UpsertUserChargelogApi" => {
            if !user.is_empty() && !guest
                && let Some(charge) = req.get("userCharge")
            {
                let mut doc = doc;
                merge(&mut doc, "userChargeList", &["chargeId"], std::slice::from_ref(charge));
                save(&user, &doc);
            }
            ok()
        }
        _ => {
            log!("chuni: {api} not handled");
            Json::obj().with("returnCode", 1i64)
        }
    }
}

/// A request to `.../ChuniServlet/<Api>`: its zlib-packed answer.
fn title(path: &str, body: &[u8]) -> Vec<u8> {
    let api = path.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    let answer = if api.eq_ignore_ascii_case("ping") {
        ok()
    } else {
        let text = miniz_oxide::inflate::decompress_to_vec_zlib(body).map(|t| String::from_utf8_lossy(&t).into_owned()).unwrap_or_default();
        let req = Json::parse(&text).unwrap_or(Json::obj());
        let answer = answer(path, &api, &req);
        log!("chuni: {api} {} -> {}", text.chars().take(1500).collect::<String>(), answer.to_string().chars().take(300).collect::<String>());
        answer
    };
    miniz_oxide::deflate::compress_to_vec_zlib(answer.to_string().as_bytes(), 6)
}

pub(crate) fn init() {
    crate::allnet::set_title(title);
    crate::allnet::serve_billing();
}
