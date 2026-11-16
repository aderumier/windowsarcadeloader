//! CHUNITHM's title server (the Aime card players' data): the game posts zlib-packed JSON to
//! `<ALL.Net uri>ChuniServlet/<Api>` (the uri `http://<router>/SDBT/<version>/`, `WAL_ALLS_TITLE_URI`:
//! `120` STAR, `125` STAR PLUS),
//! answered in ALL.Net's server (`allnet.rs`) the same way packed. The answers follow ARTEMiS's
//! CHUNITHM title server (WTFPL, gitea.tendokyu.moe/Dniel97/artemis, `titles/chuni/base.py`,
//! `star.py`, `starplus.py`, `new.py`: their requests are not encrypted; NEW!!'s, `SDHD`, with
//! its unencrypted program).
//!
//! The players are kept in `Y:\chuni\` (one JSON document per Aime user id): the game's own
//! records as it sent them (`userData`, `userDataEx`, `userGameOption`, `userGameOptionEx`,
//! `userRecentRatingList`, and the lists merged by their ids: characters, items, maps, courses,
//! duels, activities, charges, music details). The game's events and charge items are listed
//! (the events but the news, type 1, each a screen at login: `WAL_CHUNI_NEWS=1` lists them too)
//! from its data (`data\A000` and the options' folders: `event*\Event.xml`,
//! `chargeItem*\ChargeItem.xml`). NEW!! and later: their lists too (map areas, missions,
//! unlock challenges, linked verses, over power, e-money, the net battle data, favorite songs,
//! the last 50 plays), the matching rooms of a lone cabinet (in memory), the rivals (other saved
//! players), Card Maker's gachas (none: its data is not here), cards and prints. No login
//! bonuses, rankings or teams.

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
const LISTS: [(&str, &[&str]); 16] = [
    ("userCharacterList", &["characterId"]),
    ("userItemList", &["itemKind", "itemId"]),
    ("userMapList", &["mapId"]),
    ("userCourseList", &["courseId"]),
    ("userDuelList", &["duelId"]),
    ("userActivityList", &["kind", "id"]),
    ("userChargeList", &["chargeId"]),
    ("userMusicDetailList", &["musicId", "level"]),
    ("userMapAreaList", &["mapAreaId"]),
    ("userLoginBonusList", &["presetId"]),
    ("userCMissionList", &["missionId"]),
    ("userUnlockChallengeList", &["unlockChallengeId"]),
    ("userLinkedVerseList", &["linkedVerseId"]),
    ("userOverPowerList", &["genreId", "difficulty"]),
    ("userEmoneyList", &["emoneyBrand"]),
    ("userGachaList", &["gachaId"]),
];

/// The plays kept (ARTEMiS keeps them all: its rival and rating pages).
const PLAYLOGS: usize = 50;

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
    let now = wal_payload_common::clock::unix_now() as i64;
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
    // NEW!! and later (SDHD): the matching servers' times and addresses (ARTEMiS's `new.py`),
    // the title server's own (its power-on test ends otherwise in a crash)
    let setting = if path.contains("/SDHD/") {
        let r = crate::netenv::router();
        let uri = format!("http://{}.{}.{}.{}{}", r[0], r[1], r[2], r[3], path.rsplit_once('/').map_or("", |(dir, _)| dir));
        setting
            .with("romVersion", data_version(path))
            .with("matchStartTime", at(now + 8 * 3600))
            .with("matchEndTime", at(now + 16 * 3600))
            .with("matchTimeLimit", 60i64)
            .with("matchErrorLimit", 9999i64)
            .with("matchingUri", format!("{uri}/"))
            .with("matchingUriX", format!("{uri}/"))
            .with("udpHolePunchUri", format!("{uri}/"))
            .with("reflectorUri", format!("{uri}/"))
    } else {
        setting
    };
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

fn user_preview(user: &str, doc: &Json, new: bool) -> Json {
    let Some(data) = doc.get("userData") else { return ok() };
    let option = doc.get("userGameOption").cloned().unwrap_or(Json::obj());
    let character_id = text(data.get("characterId"));
    let character = doc.a("userCharacterList").iter().find(|c| text(c.get("characterId")) == character_id).cloned().unwrap_or(Json::obj());
    let field = |k: &str| data.get(k).cloned().unwrap_or(Json::Null);
    let preview = Json::obj()
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
        .with("chargeState", 1i64);
    if !new {
        return preview.with("userNameEx", field("userName"));
    }
    // NEW!!'s (`new.py`)
    preview
        .with("emoneyBrandId", 0i64)
        .with("userNameEx", "")
        .with("banState", 0i64)
        .with("classEmblemMedal", field("classEmblemMedal"))
        .with("classEmblemBase", field("classEmblemBase"))
        .with("battleRankId", field("battleRankId"))
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
    if let Some(battle) = upsert.a("userNetBattleData").first() {
        doc.set("userNetBattleData", battle.clone());
    }
    // the favorite songs: the list as sent (-1: an empty slot)
    if upsert.get("userFavoriteMusicList").is_some() {
        let ids: Vec<Json> = upsert.a("userFavoriteMusicList").iter().map(|m| text(m.get("musicId"))).filter(|id| id != "-1").map(Json::from).collect();
        doc.set("favoriteMusicIds", ids);
    }
    let plays = upsert.a("userPlaylogList");
    if !plays.is_empty() {
        let mut kept = doc.a("userPlaylogList").to_vec();
        kept.extend(plays.iter().cloned());
        let from = kept.len().saturating_sub(PLAYLOGS);
        doc.set("userPlaylogList", kept.split_off(from));
    }
    log!("chuni: user {user} saved");
}

/// A matching room (NEW!!'s online battles, `new.py`): its owner, members, the seconds left.
struct Room {
    id: i64,
    owner: String,
    members: Vec<Json>,
    rest: i64,
}

static ROOMS: Mutex<Vec<Room>> = Mutex::new(Vec::new());

/// A room's wait state as `matchingWaitState` (`first`: the asking member first).
fn wait_state(room: &Room, first: Option<&Json>) -> Json {
    let mut members: Vec<Json> = first.into_iter().cloned().collect();
    let first_id = first.map(|m| text(m.get("userId")));
    members.extend(room.members.iter().filter(|m| Some(text(m.get("userId"))) != first_id).cloned());
    Json::obj().with("isFinish", room.rest == 0).with("restMSec", room.rest).with("pollingInterval", 1i64).with("matchingMemberInfoList", members)
}

/// A member's info with its name read back from the game's WTF-8.
fn member(req: &Json) -> Json {
    let mut m = req.get("matchingMemberInfo").cloned().unwrap_or(Json::obj());
    let name = read_wtf8(m.s("userName"));
    m.set("userName", name);
    m
}

fn matching(api: &str, req: &Json) -> Json {
    let mut rooms = ROOMS.lock().unwrap();
    match api {
        "BeginMatchingApi" => {
            let m = member(req);
            let index = match rooms.iter().position(|r| r.rest > 0 && r.members.len() < 4) {
                Some(i) => {
                    rooms[i].members.push(m);
                    i
                }
                None => {
                    let id = rooms.iter().map(|r| r.id).max().unwrap_or(0) + 1;
                    rooms.push(Room { id, owner: text(m.get("userId")), members: vec![m], rest: 60 });
                    rooms.len() - 1
                }
            };
            Json::obj().with("roomId", rooms[index].id).with("matchingWaitState", wait_state(&rooms[index], None))
        }
        "GetMatchingStateApi" => {
            let id = num(req.get("roomId"));
            let current = member(req);
            let Some(room) = rooms.iter_mut().find(|r| r.id == id) else { return Json::obj().with("roomId", id).with("matchingWaitState", Json::obj().with("isFinish", true).with("restMSec", 0i64).with("pollingInterval", 1i64).with("matchingMemberInfoList", vec![current])) };
            if room.owner == text(current.get("userId")) {
                room.rest = (room.rest - 1).max(0);
            }
            if let Some(m) = room.members.iter_mut().find(|m| text(m.get("userId")) == text(current.get("userId"))) {
                *m = current.clone();
            }
            Json::obj().with("roomId", id).with("matchingWaitState", wait_state(room, Some(&current)))
        }
        "EndMatchingApi" => {
            let id = num(req.get("roomId"));
            let Some(room) = rooms.iter_mut().find(|r| r.id == id) else { return Json::obj().with("matchingResult", 1i64).with("matchingMemberInfoList", Vec::<Json>::new()).with("matchingMemberRoleList", Vec::<Json>::new()) };
            room.rest = 0;
            let roles: Vec<Json> = room.members.iter().map(|m| Json::obj().with("role", (text(m.get("userId")) == room.owner) as i64)).collect();
            let r = crate::netenv::router();
            Json::obj().with("matchingResult", 1i64).with("matchingMemberInfoList", room.members.clone()).with("matchingMemberRoleList", roles).with("reflectorUri", format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3]))
        }
        _ => {
            // RemoveMatchingMemberApi
            let user = text(req.get("userId"));
            for room in rooms.iter_mut() {
                room.members.retain(|m| text(m.get("userId")) != user);
            }
            rooms.retain(|r| !r.members.is_empty());
            ok()
        }
    }
}

/// Card Maker's print states (`userPrintStateList`): the cards not printed yet, or printed.
fn print_states(doc: &Json, completed: bool) -> Vec<Json> {
    doc.a("userPrintStateList").iter().filter(|p| p.get("hasCompleted").and_then(Json::as_bool) == Some(completed)).cloned().collect()
}

/// `fields` of `record` but `drop`.
fn without(record: &Json, drop: &[&str]) -> Json {
    match record {
        Json::Obj(fields) => Json::Obj(fields.iter().filter(|(k, _)| !drop.contains(&k.as_str())).cloned().collect()),
        other => other.clone(),
    }
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
        "GetUserPreviewApi" => user_preview(&user, &doc, path.contains("/SDHD/")),
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
        // NEW!! and later (`new.py`, `base.py`)
        "CreateTokenApi" | "DeleteTokenApi" | "RemoveTokenApi" | "PrinterLoginApi" | "PrinterLogoutApi" => ok(),
        "BeginMatchingApi" | "GetMatchingStateApi" | "EndMatchingApi" | "RemoveMatchingMemberApi" => matching(api, req),
        "GetUserMapAreaApi" => {
            let ids: Vec<String> = req.a("mapAreaIdList").iter().map(|a| text(a.get("mapAreaId"))).collect();
            let areas: Vec<Json> = doc.a("userMapAreaList").iter().filter(|a| ids.contains(&text(a.get("mapAreaId")))).cloned().collect();
            Json::obj().with("userId", user.as_str()).with("userMapAreaList", areas)
        }
        "GetUserSymbolChatSettingApi" => Json::obj().with("userId", user.as_str()).with("symbolCharInfoList", Vec::<Json>::new()),
        "GetUserNetBattleDataApi" => Json::obj().with("userId", user.as_str()).with("userNetBattleData", doc.get("userNetBattleData").cloned().unwrap_or(Json::obj().with("recentNBSelectMusicList", Vec::<Json>::new()))),
        "GetUserFavoriteItemApi" => {
            // kind 1: the favorite songs
            let kind = num(req.get("kind"));
            let favorites: Vec<Json> = if kind == 1 { doc.a("favoriteMusicIds").iter().map(|id| Json::obj().with("id", num(Some(id)))).collect() } else { Vec::new() };
            let (items, next) = page(&favorites, next, max);
            Json::obj().with("userId", user.as_str()).with("length", items.len()).with("kind", kind).with("nextIndex", next).with("userFavoriteItemList", items)
        }
        "GetUserRivalDataApi" => {
            let rival = text(req.get("rivalId"));
            match load(&rival).get("userData") {
                Some(d) => Json::obj().with("userId", user.as_str()).with("userRivalData", Json::obj().with("rivalId", rival.as_str()).with("rivalName", d.get("userName").cloned().unwrap_or(Json::Null))),
                None => Json::obj(),
            }
        }
        "GetUserRivalMusicApi" => {
            let rival = text(req.get("rivalId"));
            let levels: Vec<String> = req.a("userRivalMusicLevelList").iter().map(|l| text(l.get("level"))).collect();
            let rival_doc = load(&rival);
            let mut details: Vec<Json> = rival_doc.a("userMusicDetailList").iter().filter(|d| levels.contains(&text(d.get("level")))).cloned().collect();
            details.sort_by_key(|d| (num(d.get("musicId")), num(d.get("level"))));
            let mut musics: Vec<Json> = Vec::new();
            let mut current: Option<(String, Vec<Json>)> = None;
            for d in details {
                let id = text(d.get("musicId"));
                let detail = Json::obj().with("level", d.get("level").cloned().unwrap_or(Json::Null)).with("scoreMax", d.get("scoreMax").cloned().unwrap_or(Json::Null));
                match current.as_mut() {
                    Some((cur, list)) if *cur == id => list.push(detail),
                    _ => {
                        if let Some((cur, list)) = current.take() {
                            musics.push(Json::obj().with("musicId", Json::num(cur)).with("length", list.len()).with("userRivalMusicDetailList", list));
                        }
                        current = Some((id, vec![detail]));
                    }
                }
            }
            if let Some((cur, list)) = current {
                musics.push(Json::obj().with("musicId", Json::num(cur)).with("length", list.len()).with("userRivalMusicDetailList", list));
            }
            let (musics, next) = page(&musics, next, max);
            Json::obj().with("userId", user.as_str()).with("rivalId", rival.as_str()).with("nextIndex", next).with("userRivalMusicList", musics)
        }
        "GetTeamCourseRuleApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("nextIndex", -1i64).with("teamCourseRuleList", Vec::<Json>::new()),
        "GetTeamCourseSettingApi" => Json::obj().with("userId", user.as_str()).with("length", 0i64).with("nextIndex", -1i64).with("teamCourseSettingList", Vec::<Json>::new()),
        // Card Maker: no gachas (its own data), the player's cards and prints
        "GetGameGachaApi" => Json::obj().with("length", 0i64).with("gameGachaList", Vec::<Json>::new()).with("registIdList", Vec::<Json>::new()),
        "GetGameGachaCardByIdApi" => Json::obj()
            .with("gachaId", req.get("gachaId").cloned().unwrap_or(Json::Null))
            .with("length", 0i64)
            .with("isPickup", false)
            .with("gameGachaCardList", Vec::<Json>::new())
            .with("emissionList", Vec::<Json>::new())
            .with("afterCalcList", Vec::<Json>::new())
            .with("ssrBookCalcList", Vec::<Json>::new()),
        "RollGachaApi" => Json::obj().with("length", 0i64).with("gameGachaCardList", Vec::<Json>::new()),
        "GetUserGachaApi" => user_list(&user, &doc, "userGachaList", "userGachaList"),
        "GetUserPrintedCardApi" => {
            let cards: Vec<Json> = print_states(&doc, true).iter().map(|p| p.get("cardId").cloned().unwrap_or(Json::Null)).collect();
            let (cards, next) = page(&cards, next, max);
            Json::obj().with("userId", user.as_str()).with("length", cards.len()).with("nextIndex", next).with("userPrintedCardList", cards)
        }
        "GetUserCardPrintErrorApi" => {
            let states = print_states(&doc, false);
            Json::obj().with("userId", user.as_str()).with("length", states.len()).with("userCardPrintStateList", states)
        }
        "CMGetUserPreviewApi" => match doc.get("userData") {
            Some(d) => Json::obj()
                .with("userName", d.get("userName").cloned().unwrap_or(Json::Null))
                .with("level", d.get("level").cloned().unwrap_or(Json::Null))
                .with("medal", d.get("medal").cloned().unwrap_or(Json::Null))
                .with("lastDataVersion", "2.00.00")
                .with("isLogin", false),
            None => Json::obj(),
        },
        "CMGetUserDataApi" => match doc.get("userData") {
            Some(d) => {
                let emoney = Json::obj().with("type", 0i64).with("emoneyCredit", 100i64).with("emoneyBrand", 1i64).with("ext1", 0i64).with("ext2", 0i64).with("ext3", 0i64);
                Json::obj().with("userId", user.as_str()).with("userData", d.clone()).with("userEmoney", vec![emoney])
            }
            None => Json::obj(),
        },
        "CMGetUserCharacterApi" => user_paged(&user, &doc, "userCharacterList", "userCharacterList", next, max),
        "CMGetUserItemApi" => user_items(&user, &doc, next, max),
        "CMUpsertUserGachaApi" => {
            let upsert = req.get("cmUpsertUserGacha").cloned().unwrap_or(Json::obj());
            let gacha = upsert.get("userGacha").cloned().unwrap_or(Json::obj());
            let gacha_id = text(gacha.get("gachaId"));
            let mut doc = doc;
            if !user.is_empty() && !guest {
                if let Some(data) = upsert.get("userData") {
                    let mut merged = doc.get("userData").cloned().unwrap_or(Json::obj());
                    if let Json::Obj(fields) = without(data, &["rankUpChallengeResults", "userEmoney"]) {
                        for (k, v) in fields {
                            merged.set(&k, v);
                        }
                    }
                    doc.set("userData", merged);
                }
                merge(&mut doc, "userGachaList", &["gachaId"], std::slice::from_ref(&gacha));
                let items = upsert.a("userItemList").to_vec();
                merge(&mut doc, "userItemList", &["itemKind", "itemId"], &items);
                let mut states = doc.a("userPrintStateList").to_vec();
                let mut order = states.iter().map(|p| num(p.get("orderId"))).max().unwrap_or(0);
                for card in upsert.a("gameGachaCardList") {
                    order += 1;
                    states.push(
                        Json::obj()
                            .with("orderId", order)
                            .with("hasCompleted", false)
                            .with("limitDate", "2099-12-31")
                            .with("placeId", req.get("placeId").cloned().unwrap_or(Json::Null))
                            .with("cardId", card.get("cardId").cloned().unwrap_or(Json::Null))
                            .with("gachaId", card.get("gachaId").cloned().unwrap_or(Json::Null)),
                    );
                }
                doc.set("userPrintStateList", states);
                save(&user, &doc);
            }
            let states: Vec<Json> = print_states(&doc, false).into_iter().filter(|p| text(p.get("gachaId")) == gacha_id).collect();
            Json::obj().with("returnCode", "1").with("apiName", "CMUpsertUserGachaApi").with("userCardPrintStateList", states)
        }
        "CMUpsertUserPrintlogApi" => Json::obj().with("returnCode", 1i64).with("orderId", 0i64).with("serialId", "11111111111111111111").with("apiName", api),
        "CMUpsertUserPrintApi" => {
            // a serial number from the clock (20 digits)
            let serial = format!("{:020}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() % 100_000_000_000_000_000_000));
            if !user.is_empty() && !guest {
                let mut doc = doc;
                let detail = without(req.get("userPrintDetail").unwrap_or(&Json::Null), &["orderId", "printNumber", "serialId"]).with("serialId", serial.as_str());
                let mut details = doc.a("userPrintDetailList").to_vec();
                details.push(detail);
                doc.set("userPrintDetailList", details);
                save(&user, &doc);
            }
            Json::obj().with("returnCode", 1i64).with("orderId", 0i64).with("serialId", serial).with("apiName", api)
        }
        "CMUpsertUserPrintSubtractApi" | "CMUpsertUserPrintCancelApi" => {
            let orders: Vec<String> = if api == "CMUpsertUserPrintCancelApi" {
                req.a("orderIdList").iter().map(|o| text(Some(o))).collect()
            } else {
                vec![text(req.get("userCardPrintState").and_then(|s| s.get("orderId")))]
            };
            if !user.is_empty() && !guest {
                let mut doc = doc;
                let items = req.a("userItemList").to_vec();
                merge(&mut doc, "userItemList", &["itemKind", "itemId"], &items);
                let states: Vec<Json> = doc
                    .a("userPrintStateList")
                    .iter()
                    .map(|p| if orders.contains(&text(p.get("orderId"))) { without(p, &["hasCompleted"]).with("hasCompleted", true) } else { p.clone() })
                    .collect();
                doc.set("userPrintStateList", states);
                save(&user, &doc);
            }
            Json::obj().with("returnCode", "1").with("apiName", api)
        }
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
    wal_payload_common::allnet::set_title(title);
    crate::allnet::serve_billing();
}
