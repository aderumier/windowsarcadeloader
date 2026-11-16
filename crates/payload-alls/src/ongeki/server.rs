//! O.N.G.E.K.I.'s title server (the Aime card players' data): the game posts JSON to
//! `<ALL.Net uri><Api>` (the uri `http://<router>/SDDT/<version>/`, `WAL_ALLS_TITLE_URI`),
//! answered in amdaemon's ALL.Net server (`allnet.rs`). Version 1.00's requests and answers are
//! plain JSON, later versions' zlib-packed (answered the same way as asked). The answers follow
//! ARTEMiS's O.N.G.E.K.I. title server (WTFPL, gitea.tendokyu.moe/Dniel97/artemis,
//! `titles/ongeki/base.py`, the versions' `plus.py` ... `brightmemoryact3.py`, `bright.py` and
//! `brightmemory.py`: Card Maker's calls, gacha, memory chapters). Encrypted requests (the
//! endpoints' names hashed, AES: R.E.D. and later without the community's no-encryption patch)
//! are not answered.
//!
//! The players are kept in `Y:\ongeki\` (one JSON document per Aime user id): the game's own
//! records as it sent them (`userData`, `userOption`, `userRecentRatingList`, `userBpBaseList`,
//! the rating lists, and the lists merged by their ids). The game's events and rewards are listed
//! from its data (`mu3_Data\StreamingAssets\GameData\A000` and the options' folders: `event*\Event.xml`,
//! `reward*\Reward.xml`, `card*\Card.xml`); the song rankings from the plays kept in
//! `Y:\ongeki\playlog.json` (plays per song and day). No presents, technical challenge songs
//! or gacha banners (ARTEMiS's tables filled by hand or from Card Maker's data); no rivals.

use std::collections::HashMap;
use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

const DIR: &str = "Y:\\ongeki";

/// Held for each request (load, change, save).
static LOCK: Mutex<()> = Mutex::new(());

/// The game's data folders (from amdaemon's working directory, the game's `package`): its own
/// and the options'.
fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    for root in ["mu3_Data\\StreamingAssets\\GameData", "C:\\mount\\Option", "..\\option", "option"] {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        entries.sort();
        dirs.extend(entries);
    }
    dirs
}

/// The text of `<tag>...</tag>` at the start of `xml`'s first `<tag>`.
fn xml_inner<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

/// `<tag><id>N</id>...` of an XML document.
fn xml_id(xml: &str, tag: &str) -> Option<i64> {
    xml_inner(xml, tag).and_then(|t| xml_inner(t, "id")).and_then(|v| v.trim().parse().ok())
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

/// A value the game sent as text or a number.
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

/// `secs` (Unix time) as `YYYY-MM-DD hh:mm:ss`.
fn date_time(secs: i64) -> String {
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
}

fn now() -> i64 {
    wal_payload_common::clock::unix_now() as i64
}

/// A small random number generator (gacha rolls).
fn random(below: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9E37_79B9, |d| d.as_nanos() as u64) | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x % below.max(1) as u64) as usize
}

fn path(user: &str) -> String {
    format!("{DIR}\\{user}.json")
}

fn load_file(path: &str) -> Json {
    std::fs::read_to_string(path).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_))).unwrap_or(Json::obj())
}

fn save_file(path: &str, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn load(user: &str) -> Json {
    load_file(&path(user))
}

fn save(user: &str, doc: &Json) {
    save_file(&path(user), doc);
}

/// Every stored player (user id, document): the rankings.
fn players() -> Vec<(String, Json)> {
    let Ok(entries) = std::fs::read_dir(DIR) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let user = name.strip_suffix(".json")?;
            user.parse::<u64>().ok()?;
            Some((user.to_string(), load_file(&e.path().to_string_lossy())))
        })
        .collect()
}

/// The stored lists (as the game sends them in `upsertUserAll`) and the fields identifying
/// their entries (ARTEMiS's tables' unique keys).
const LISTS: [(&str, &[&str]); 24] = [
    ("userActivityList", &["kind", "id"]),
    ("userMusicDetailList", &["musicId", "level"]),
    ("userCharacterList", &["characterId"]),
    ("userCardList", &["cardId"]),
    ("userDeckList", &["deckId"]),
    ("userTrainingRoomList", &["roomId"]),
    ("userStoryList", &["storyId"]),
    ("userChapterList", &["chapterId"]),
    ("userMemoryChapterList", &["chapterId"]),
    ("userItemList", &["itemKind", "itemId"]),
    ("userMusicItemList", &["musicId"]),
    ("userLoginBonusList", &["bonusId"]),
    ("userEventPointList", &["eventId"]),
    ("userMissionPointList", &["eventId"]),
    ("userRatinglogList", &["dataVersion"]),
    ("userBossList", &["musicId", "eventId"]),
    ("userTechCountList", &["levelId"]),
    ("userScenerioList", &["scenarioId"]),
    ("userTradeItemList", &["chapterId", "tradeItemId"]),
    ("userEventMusicList", &["eventId", "type", "musicId", "level"]),
    ("userTechEventList", &["eventId"]),
    ("userKopList", &["kopId"]),
    ("userGachaList", &["gachaId"]),
    ("userPrintDetailList", &["serialId"]),
];

/// The rating lists, kept whole as sent.
const RATINGS: [&str; 6] = [
    "userRatingBaseBestList",
    "userRatingBaseBestNewList",
    "userRatingBaseHotList",
    "userRatingBaseNextList",
    "userRatingBaseNextNewList",
    "userRatingBaseHotNextList",
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

fn merge_list(doc: &mut Json, list: &str, entries: &[Json]) {
    if let Some((_, keys)) = LISTS.iter().find(|(l, _)| *l == list) {
        merge(doc, list, keys, entries);
    }
}

fn ok(api: &str) -> Json {
    Json::obj().with("returnCode", 1i64).with("apiName", api)
}

/// The version from the request path (`/SDDT/100/...`: 100).
fn version(path: &str) -> u32 {
    path.split('/').filter_map(|p| p.parse::<u32>().ok()).next().unwrap_or(100)
}

/// The data version ARTEMiS answers for the version's release (1.00, 1.05 ... 1.45; 1.50 and
/// its updates: 1.50.00).
fn data_version(version: u32) -> &'static str {
    match version {
        0..105 => "1.00.00",
        105..110 => "1.05.00",
        110..115 => "1.10.00",
        115..120 => "1.15.00",
        120..125 => "1.20.00",
        125..130 => "1.25.00",
        130..135 => "1.30.00",
        135..145 => "1.35.00",
        145..150 => "1.45.00",
        _ => "1.50.00",
    }
}

fn game_setting(version: u32) -> Json {
    // the server's maintenance (guest play only, the game rebooted at its start): the game's own
    // default hours, Japan's 04:00-07:00, never reached with the time warp (ARTEMiS's 6-7 hours
    // from now in UTC fall in the day's play hours: an hour of guest play, a reboot)
    let today = date_time(now() + 9 * 3600)[..10].to_string();
    let rival = if version >= 135 { "maxCountRivalMusic" } else { "macCountRivalMusic" };
    let setting = Json::obj()
        .with("dataVersion", data_version(version))
        .with("onlineDataVersion", data_version(version))
        .with("isMaintenance", "false")
        .with("requestInterval", 10i64)
        .with("rebootStartTime", format!("{today} 04:00:00"))
        .with("rebootEndTime", format!("{today} 07:00:00"))
        .with("isBackgroundDistribute", "false")
        .with("maxCountCharacter", 50i64)
        .with("maxCountCard", 300i64)
        .with("maxCountItem", 300i64)
        .with("maxCountMusic", 50i64)
        .with("maxCountMusicItem", 300i64)
        .with(rival, 300i64);
    Json::obj().with("gameSetting", setting).with("isDumpUpload", "false").with("isAou", "true")
}

/// The event types' numbers (ARTEMiS's `EVT_TYPES`, an enum from 1).
const EVENT_TYPES: [&str; 21] = [
    "Announcement",
    "Movie",
    "AddMyList",
    "UnlockChapter",
    "JewelEvent",
    "RankingEvent",
    "AcceptRankingEvent",
    "UnlockMusic",
    "UnlockCard",
    "UnlockTrophy",
    "UnlockNamePlate",
    "UnlockLimitBreakItem",
    "MissionEvent",
    "DailyBonus",
    "UnlockBossLockEarly",
    "UnlockPurchaseItem",
    "TechChallengeEvent",
    "AcceptTechChallengeEvent",
    "SilverJewelEvent",
    "Max",
    "None",
];

/// The reward item types' numbers (ARTEMiS's `REWARD_TYPES`, an enum from 1).
const REWARD_TYPES: [&str; 22] = [
    "Card",
    "NamePlate",
    "Trophy",
    "LimitBreakItem",
    "AlmightyJewel",
    "Money",
    "Music",
    "ProfileVoice",
    "Present",
    "ChapterJewel",
    "GachaTicket",
    "KaikaItem",
    "ExpUpItem",
    "IntimateUpItem",
    "BookItem",
    "SystemVoice",
    "Costume",
    "Medal",
    "Attachment",
    "UnlockItem",
    "Max",
    "None",
];

fn enum_value(names: &[&str], name: &str) -> i64 {
    names.iter().position(|n| *n == name.trim()).map_or(0, |i| i as i64 + 1)
}

fn game_events(req: &Json) -> Json {
    let mut seen = std::collections::HashSet::new();
    let events: Vec<Json> = data_files("event", "Event.xml")
        .iter()
        .filter_map(|xml| {
            let id = xml_id(xml, "Name")?;
            let kind = enum_value(&EVENT_TYPES, xml_inner(xml, "EventType").unwrap_or(""));
            seen.insert((id, kind)).then(|| {
                Json::obj().with("type", kind).with("id", id).with("startDate", "2018-01-01 00:00:00.0").with("endDate", "2099-12-31 00:00:00.0")
            })
        })
        .collect();
    Json::obj().with("type", req.get("type").cloned().unwrap_or(Json::num(0))).with("length", events.len()).with("gameEventList", events)
}

fn game_rewards() -> Json {
    let mut seen = std::collections::HashSet::new();
    let rewards: Vec<Json> = data_files("reward", "Reward.xml")
        .iter()
        .filter_map(|xml| {
            let id = xml_id(xml, "Name").filter(|&id| seen.insert(id))?;
            let kind = enum_value(&REWARD_TYPES, xml_inner(xml, "ItemType").unwrap_or(""));
            let item = xml_inner(xml, "RewardItem").and_then(|r| xml_id(r, "ItemName")).unwrap_or(0);
            Some(Json::obj().with("rewardId", id).with("itemKind", kind).with("itemId", item))
        })
        .collect();
    Json::obj().with("length", rewards.len()).with("gameRewardList", rewards)
}

/// The cards of a rarity (Card.xml's: N 0, R 1, SR 2, SSR 3, SRPlus 12).
fn cards_by_rarity(rarity: i64) -> Vec<Json> {
    data_files("card", "Card.xml")
        .iter()
        .filter_map(|xml| {
            let r = match xml_inner(xml, "Rarity")?.trim() {
                "N" => 0,
                "R" => 1,
                "SR" => 2,
                "SSR" => 3,
                "SRPlus" => 12,
                _ => return None,
            };
            (r == rarity).then(|| Json::obj().with("cardId", xml_id(xml, "Name").unwrap_or(0)).with("rarity", r))
        })
        .collect()
}

/// The JST day `days_back` days before the ranking's day (ARTEMiS: now in JST minus 1 day 7 h).
fn ranking_day(days_back: i64) -> String {
    date_time(now() + 9 * 3600 - 86400 - 7 * 3600 - days_back * 86400)[..10].to_string()
}

fn game_ranking(req: &Json) -> Json {
    let kind = num(req.get("type"));
    let day = ranking_day(if kind == 2 { 1 } else { 0 });
    let plays = load_file(&format!("{DIR}\\playlog.json"));
    let mut counts: Vec<(i64, i64)> = match plays.get(&day) {
        Some(Json::Obj(songs)) => songs.iter().map(|(id, n)| (id.parse().unwrap_or(0), num(Some(n)))).collect(),
        _ => Vec::new(),
    };
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    counts.truncate(10);
    let list: Vec<Json> = if counts.is_empty() || (kind == 1 && counts.len() < 10) {
        Vec::new()
    } else {
        counts.iter().map(|&(id, n)| Json::obj().with("id", id).with("point", n).with("userName", "")).collect()
    };
    Json::obj().with("type", req.get("type").cloned().unwrap_or(Json::num(0))).with("gameRankingList", list)
}

/// The plays of `playlogs` counted per song and day (the song rankings).
fn count_plays(playlogs: &[Json]) {
    if playlogs.is_empty() {
        return;
    }
    let file = format!("{DIR}\\playlog.json");
    let mut plays = load_file(&file);
    for p in playlogs {
        let date = text(p.get("playDate"));
        let date = if date.len() >= 10 { date[..10].to_string() } else { date_time(now() + 9 * 3600)[..10].to_string() };
        let song = text(p.get("musicId"));
        let mut day = plays.get(&date).cloned().unwrap_or(Json::obj());
        let n = num(day.get(&song)) + 1;
        day.set(&song, n);
        plays.set(&date, day);
    }
    // the last 30 days only
    if let Json::Obj(days) = &mut plays {
        days.sort_by(|a, b| a.0.cmp(&b.0));
        let extra = days.len().saturating_sub(30);
        days.drain(..extra);
    }
    save_file(&file, &plays);
}

fn user_preview(user: &Json, doc: &Json) -> Json {
    let (Some(data), Some(option)) = (doc.get("userData"), doc.get("userOption")) else {
        return Json::obj()
            .with("userId", user.clone())
            .with("isLogin", false)
            .with("lastLoginDate", "0000-00-00 00:00:00")
            .with("userName", "")
            .with("reincarnationNum", 0i64)
            .with("level", 0i64)
            .with("exp", 0i64)
            .with("playerRating", 0i64)
            .with("lastGameId", "")
            .with("lastRomVersion", "")
            .with("lastDataVersion", "")
            .with("lastPlayDate", "")
            .with("nameplateId", 0i64)
            .with("trophyId", 0i64)
            .with("cardId", 0i64)
            .with("dispPlayerLv", 0i64)
            .with("dispRating", 0i64)
            .with("dispBP", 0i64)
            .with("headphone", 0i64)
            .with("banStatus", 0i64)
            .with("isWarningConfirmed", true);
    };
    let field = |k: &str| data.get(k).cloned().unwrap_or(Json::num(0));
    let opt = |k: &str| option.get(k).cloned().unwrap_or(Json::num(0));
    Json::obj()
        .with("userId", user.clone())
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
        .with("nameplateId", field("nameplateId"))
        .with("trophyId", field("trophyId"))
        .with("cardId", field("cardId"))
        .with("dispPlayerLv", opt("dispPlayerLv"))
        .with("dispRating", opt("dispRating"))
        .with("dispBP", opt("dispBP"))
        .with("headphone", opt("headphone"))
        .with("banStatus", data.get("banStatus").cloned().unwrap_or(Json::num(0)))
        .with("isWarningConfirmed", true)
}

/// One page of `items` from `next` (`maxCount` long): the page and the next index (0: none,
/// ARTEMiS's end).
fn page(items: &[Json], next: i64, max: i64) -> (Vec<Json>, i64) {
    let start = (next.max(0) as usize).min(items.len());
    let end = (start + max.max(1) as usize).min(items.len());
    (items[start..end].to_vec(), if end < items.len() { end as i64 } else { 0 })
}

/// The music details grouped per music (`maxCount` musics a page), each in `inner`; `drop`: the
/// fields left out (a rival's).
fn music_pages(details: &[Json], next: i64, max: i64, inner: &str, drop: &[&str]) -> (Vec<Json>, i64) {
    let mut details = details.to_vec();
    details.sort_by_key(|d| (num(d.get("musicId")), num(d.get("level"))));
    let mut musics: Vec<Vec<Json>> = Vec::new();
    for mut d in details {
        if let Json::Obj(fields) = &mut d {
            fields.retain(|(k, _)| !drop.contains(&k.as_str()));
        }
        match musics.last_mut() {
            Some(m) if text(m[0].get("musicId")) == text(d.get("musicId")) => m.push(d),
            _ => musics.push(vec![d]),
        }
    }
    let (musics, next) = page(&musics.into_iter().map(Json::from).collect::<Vec<_>>(), next, max);
    let list = musics.into_iter().map(|m| Json::obj().with("length", m.items().len()).with(inner, m)).collect();
    (list, next)
}

/// The items of the kind in `nextIndex` (kind * 10^10 + index).
fn user_items(user: &Json, doc: &Json, next: i64, max: i64) -> Json {
    let (kind, index) = (next / 10_000_000_000, next % 10_000_000_000);
    let items: Vec<Json> = doc.a("userItemList").iter().filter(|i| num(i.get("itemKind")) == kind).cloned().collect();
    let (items, next) = page(&items, index, max);
    let next = if next == 0 { 0 } else { kind * 10_000_000_000 + next };
    Json::obj().with("userId", user.clone()).with("nextIndex", next).with("itemKind", kind).with("length", items.len()).with("userItemList", items)
}

/// A whole list (`stored` in the document, `answer` in the answer).
fn user_list(user: &Json, doc: &Json, stored: &str, answer: &str) -> Json {
    let items = doc.a(stored).to_vec();
    Json::obj().with("userId", user.clone()).with("length", items.len()).with(answer, items)
}

/// A paged list (Card Maker's cards and characters).
fn user_paged(user: &Json, doc: &Json, list: &str, next: i64, max: i64) -> Json {
    let (items, next) = page(doc.a(list), next, max);
    Json::obj().with("userId", user.clone()).with("length", items.len()).with("nextIndex", next).with(list, items)
}

/// Every player's entries of `list` for one event, ranked by `score` (descending) per event:
/// the player's own entries with their rank (ARTEMiS ranks at each request).
fn ranked(user: &str, list: &str, score: &dyn Fn(&Json) -> (i64, i64)) -> Vec<(Json, i64)> {
    let mut all: Vec<(String, Json)> = Vec::new();
    for (id, doc) in players() {
        all.extend(doc.a(list).iter().map(|e| (id.clone(), e.clone())));
    }
    let mut out = Vec::new();
    for (id, e) in all.iter().filter(|(id, _)| id == user) {
        let event = text(e.get("eventId"));
        let mine = score(e);
        let better = all.iter().filter(|(other, o)| other != id && text(o.get("eventId")) == event && score(o) > mine).count();
        out.push((e.clone(), better as i64 + 1));
    }
    out
}

fn event_ranking(user: &Json, uid: &str) -> Json {
    let list: Vec<Json> = ranked(uid, "userEventPointList", &|e| (num(e.get("point")), 0))
        .into_iter()
        .map(|(e, rank)| {
            Json::obj()
                .with("eventId", e.get("eventId").cloned().unwrap_or(Json::num(0)))
                .with("type", 1i64)
                .with("rank", rank)
                .with("date", e.get("_date").cloned().unwrap_or(Json::str("")))
                .with("point", e.get("point").cloned().unwrap_or(Json::num(0)))
        })
        .collect();
    Json::obj().with("userId", user.clone()).with("length", list.len()).with("userEventRankingList", list)
}

fn tech_event_ranking(user: &Json, uid: &str) -> Json {
    let list: Vec<Json> = ranked(uid, "userTechEventList", &|e| (num(e.get("totalTechScore")), num(e.get("totalPlatinumScore"))))
        .into_iter()
        .map(|(e, rank)| {
            Json::obj()
                .with("date", e.get("techRecordDate").cloned().unwrap_or(Json::str("")))
                .with("eventId", e.get("eventId").cloned().unwrap_or(Json::num(0)))
                .with("rank", rank)
                .with("totalTechScore", e.get("totalTechScore").cloned().unwrap_or(Json::num(0)))
                .with("totalPlatinumScore", e.get("totalPlatinumScore").cloned().unwrap_or(Json::num(0)))
        })
        .collect();
    Json::obj().with("userId", user.clone()).with("length", list.len()).with("userTechEventRankingList", list)
}

/// The fields of the stored event points the game's list leaves out.
fn event_points(user: &Json, doc: &Json) -> Json {
    let list: Vec<Json> = doc
        .a("userEventPointList")
        .iter()
        .map(|e| {
            let mut e = e.clone();
            if let Json::Obj(fields) = &mut e {
                fields.retain(|(k, _)| !matches!(k.as_str(), "_date" | "rank" | "type" | "date"));
            }
            e
        })
        .collect();
    Json::obj().with("userId", user.clone()).with("length", list.len()).with("userEventPointList", list)
}

/// bright MEMORY's chapters before any is played (ARTEMiS's defaults).
fn memory_chapters(user: &Json, doc: &Json) -> Json {
    let mut list = doc.a("userMemoryChapterList").to_vec();
    if list.is_empty() {
        list = [70001, 70002, 70003, 70004, 70005, 70099]
            .into_iter()
            .map(|id| {
                Json::obj()
                    .with("gaugeId", 0i64)
                    .with("isClear", false)
                    .with("gaugeNum", 0i64)
                    .with("chapterId", id as i64)
                    .with("jewelCount", 0i64)
                    .with("isBossWatched", false)
                    .with("isStoryWatched", false)
                    .with("isDialogWatched", false)
                    .with("isEndingWatched", false)
                    .with("lastPlayMusicId", 0i64)
                    .with("lastPlayMusicLevel", 0i64)
                    .with("lastPlayMusicCategory", 0i64)
            })
            .collect();
    }
    Json::obj().with("userId", user.clone()).with("length", list.len()).with("userMemoryChapterList", list)
}

/// `userData` stored: its access code too (ARTEMiS drops it and answers the card's).
fn put_user_data(doc: &mut Json, data: &Json) {
    let mut data = data.clone();
    if data.get("accessCode").is_none_or(|c| matches!(c, Json::Null))
        && let Some(code) = doc.get("userData").and_then(|d| d.get("accessCode"))
    {
        data.set("accessCode", code.clone());
    }
    doc.set("userData", data);
}

fn upsert_all(user: &str, doc: &mut Json, upsert: &Json) {
    if let Some(data) = upsert.a("userData").first() {
        put_user_data(doc, data);
    }
    if let Some(option) = upsert.a("userOption").first() {
        doc.set("userOption", option.clone());
    }
    count_plays(upsert.a("userPlaylogList"));
    for record in ["userRecentRatingList", "userBpBaseList"].into_iter().chain(RATINGS) {
        if let Some(list) = upsert.get(record) {
            doc.set(record, list.clone());
        }
    }
    for (list, keys) in LISTS {
        let mut entries = upsert.a(list).to_vec();
        if entries.is_empty() {
            continue;
        }
        // the event points' date (the event rankings')
        if list == "userEventPointList" {
            for e in &mut entries {
                e.set("_date", date_time(now() + 9 * 3600)[..16].to_string());
            }
        }
        merge(doc, list, keys, &entries);
    }
    log!("ongeki: user {user} saved");
}

/// Card Maker's gacha roll (ARTEMiS's `bright.py`): R 77%, SR 20%, SSR 3% (the free gachas
/// 1011, 1012 their own), at least one SR in 11 rolls (or 5 with the weekly guarantee), one
/// SSR with the SSR book.
fn roll_gacha(req: &Json) -> Json {
    let gacha = num(req.get("gachaId"));
    let times = num(req.get("times"));
    let change_rate = matches!(req.get("changeRate"), Some(Json::Bool(true)));
    let book = num(req.get("bookUseCount")) == 1;
    if ![1, 5, 11].contains(&times) {
        return Json::obj();
    }
    let weights: [usize; 3] = match gacha {
        1011 => [94, 5, 1],
        1012 => [0, 92, 8],
        _ => [77, 20, 3],
    };
    let rolls = loop {
        let rolls: Vec<i64> = (0..times)
            .map(|_| {
                let r = random(100);
                if r < weights[0] {
                    1
                } else if r < weights[0] + weights[1] {
                    2
                } else {
                    3
                }
            })
            .collect();
        if book {
            if rolls.contains(&3) {
                break rolls;
            }
        } else if (times == 5 && change_rate) || times == 11 {
            if rolls.iter().any(|&r| r >= 2) {
                break rolls;
            }
        } else {
            break rolls;
        }
    };
    let pools = [cards_by_rarity(1), cards_by_rarity(2), cards_by_rarity(3)];
    let list: Vec<Json> = rolls
        .iter()
        .filter_map(|&r| {
            let pool = &pools[r as usize - 1];
            let card = pool.get(random(pool.len()))?;
            Some(
                Json::obj()
                    .with("gachaId", gacha)
                    .with("cardId", card.get("cardId").cloned().unwrap_or(Json::num(0)))
                    // +1: Card Maker's rarities
                    .with("rarity", r + 1)
                    .with("weight", 1i64)
                    .with("isPickup", false)
                    .with("isSelect", false),
            )
        })
        .collect();
    Json::obj().with("length", list.len()).with("gameGachaCardList", list)
}

/// Card Maker's gacha banner cards: ARTEMiS's fallback (no banner data), selectable.
fn gacha_cards(req: &Json) -> Json {
    let gacha = req.get("gachaId").cloned().unwrap_or(Json::num(0));
    let list: Vec<Json> = [(100984, 4, 1), (100997, 3, 2), (100998, 3, 2), (101020, 2, 3), (101021, 2, 3), (101022, 2, 3)]
        .into_iter()
        .map(|(card, rarity, weight)| {
            Json::obj()
                .with("gachaId", gacha.clone())
                .with("cardId", card as i64)
                .with("rarity", rarity as i64)
                .with("weight", weight as i64)
                .with("isPickup", false)
                .with("isSelect", true)
        })
        .collect();
    Json::obj()
        .with("gachaId", gacha)
        .with("length", list.len())
        .with("isPickup", false)
        .with("gameGachaCardList", list)
        .with("emissionList", Vec::<Json>::new())
        .with("afterCalcList", Vec::<Json>::new())
        .with("ssrBookCalcList", Vec::<Json>::new())
}

/// Card Maker's gacha played: its counters, then the player's records.
fn cm_upsert_gacha(doc: &mut Json, req: &Json) {
    let gacha = num(req.get("gachaId"));
    let count = num(req.get("gachaCnt"));
    let play_date: String = text(req.get("playDate")).chars().take(10).collect();
    let mut entry = doc.a("userGachaList").iter().find(|g| num(g.get("gachaId")) == gacha).cloned().unwrap_or_else(|| {
        Json::obj()
            .with("gachaId", gacha)
            .with("totalGachaCnt", 0i64)
            .with("ceilingGachaCnt", 0i64)
            .with("selectPoint", 0i64)
            .with("useSelectPoint", 0i64)
            .with("dailyGachaCnt", 0i64)
            .with("fiveGachaCnt", 0i64)
            .with("elevenGachaCnt", 0i64)
            .with("dailyGachaDate", "2000-01-01")
    });
    let daily = if text(entry.get("dailyGachaDate")) == play_date { num(entry.get("dailyGachaCnt")) } else { 0 };
    let total = num(entry.get("totalGachaCnt")) + count;
    let ceiling = num(entry.get("ceilingGachaCnt")) + count;
    let five = num(entry.get("fiveGachaCnt")) + (count == 5) as i64;
    let eleven = num(entry.get("elevenGachaCnt")) + (count == 11) as i64;
    entry.set("totalGachaCnt", total);
    entry.set("ceilingGachaCnt", ceiling);
    entry.set("selectPoint", req.get("selectPoint").cloned().unwrap_or(Json::num(0)));
    entry.set("dailyGachaCnt", daily + count);
    entry.set("fiveGachaCnt", five);
    entry.set("elevenGachaCnt", eleven);
    entry.set("dailyGachaDate", play_date);
    merge_list(doc, "userGachaList", &[entry]);
    let upsert = req.get("cmUpsertUserGacha").cloned().unwrap_or(Json::obj());
    cm_upsert(doc, &upsert, &["userCharacterList", "userItemList", "userCardList"]);
}

/// Card Maker's records: `userData` and the `lists`.
fn cm_upsert(doc: &mut Json, upsert: &Json, lists: &[&str]) {
    if let Some(data) = upsert.a("userData").first() {
        put_user_data(doc, data);
    }
    for list in lists {
        let entries = upsert.a(list).to_vec();
        if !entries.is_empty() {
            merge_list(doc, list, &entries);
        }
    }
}

fn serial() -> String {
    (0..20).map(|_| char::from(b'0' + random(10) as u8)).collect()
}

fn answer(path: &str, api: &str, req: &Json) -> Json {
    let version = version(path);
    let user_json = req.get("userId").cloned().unwrap_or(Json::num(0));
    let user = text(Some(&user_json));
    let next = num(req.get("nextIndex"));
    let max = num(req.get("maxCount"));
    // a guest's play (or(0x1000000000001, (placeId & 65535) << 32)) is not kept
    let guest = user.parse::<u64>().is_ok_and(|u| u & 0x1000000000001 == 0x1000000000001);
    let keep = !user.is_empty() && !guest;
    let _lock = LOCK.lock().unwrap();
    let mut doc = if keep { load(&user) } else { Json::obj() };
    let u = &user_json;
    let mut changed = false;
    let reply = match api {
        "GameLoginApi" => ok("gameLogin"),
        "GameLogoutApi" => ok("gameLogout"),
        "ExtendLockTimeApi" => ok("ExtendLockTimeApi"),
        "UpsertClientSettingApi" => ok("UpsertClientSettingApi"),
        "UpsertClientTestmodeApi" => ok("UpsertClientTestmodeApi"),
        "UpsertClientBookkeepingApi" => ok("upsertClientBookkeeping"),
        "UpsertClientDevelopApi" => ok("upsertClientDevelop"),
        "UpsertClientErrorApi" => ok("upsertClientError"),
        "UpsertUserGplogApi" => ok("UpsertUserGplogApi"),
        "PrinterLoginApi" | "PrinterLogoutApi" => Json::obj().with("returnCode", 1i64),
        "GetGameSettingApi" => game_setting(version),
        "GetGameIdlistApi" => Json::obj().with("type", req.get("type").cloned().unwrap_or(Json::num(0))).with("length", 0i64).with("gameIdlistList", Vec::<Json>::new()),
        "GetGameIdListApi" => {
            let kind = num(req.get("type"));
            let ids: Vec<Json> = match kind {
                1 => (1..231).map(|i| Json::obj().with("type", 1i64).with("id", i as i64)).collect(),
                2 => (8000..8051).map(|i| Json::obj().with("type", 2i64).with("id", i as i64)).collect(),
                _ => Vec::new(),
            };
            Json::obj().with("type", kind).with("length", ids.len()).with("gameIdlistList", ids)
        }
        "GetGameRankingApi" => game_ranking(req),
        "GetGamePointApi" => {
            let list: Vec<Json> = [(0, 100), (1, 230), (2, 370), (3, 120), (4, 240), (5, 360)]
                .into_iter()
                .map(|(kind, cost)| Json::obj().with("type", kind as i64).with("cost", cost as i64).with("startDate", "2000-01-01 05:00:00.0").with("endDate", "2099-01-01 05:00:00.0"))
                .collect();
            Json::obj().with("length", list.len()).with("gamePointList", list)
        }
        "GetGameRewardApi" => game_rewards(),
        "GetGamePresentApi" => Json::obj().with("length", 0i64).with("gamePresentList", Vec::<Json>::new()),
        "GetGameMessageApi" => Json::obj().with("length", 0i64).with("gameMessageList", Vec::<Json>::new()),
        "GetGameSaleApi" => Json::obj().with("length", 0i64).with("gameSaleList", Vec::<Json>::new()),
        "GetGameTechMusicApi" => Json::obj().with("length", 0i64).with("gameTechMusicList", Vec::<Json>::new()),
        "GetGameEventApi" => game_events(req),
        "GetGameMusicReleaseStateApi" => Json::obj().with("techScore", 0i64).with("cardNum", 0i64),
        "GetGameGachaApi" => Json::obj().with("length", 0i64).with("gameGachaList", Vec::<Json>::new()).with("registIdList", Vec::<Json>::new()),
        "GetGameGachaCardByIdApi" => gacha_cards(req),
        "GetGameTheaterApi" => Json::obj().with("length", 0i64).with("gameTheaterList", Vec::<Json>::new()).with("registIdList", Vec::<Json>::new()),
        "RollGachaApi" => roll_gacha(req),
        "GetUserRegionApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userRegionList", Vec::<Json>::new()),
        "GetUserPreviewApi" => user_preview(u, &doc),
        "GetUserTechCountApi" => user_list(u, &doc, "userTechCountList", "userTechCountList"),
        "GetUserTechEventApi" => user_list(u, &doc, "userTechEventList", "userTechEventList"),
        "GetUserTechEventRankingApi" => tech_event_ranking(u, &user),
        "GetUserKopApi" => user_list(u, &doc, "userKopList", "userKopList"),
        "GetUserMusicApi" => {
            let (list, next) = music_pages(doc.a("userMusicDetailList"), next, max, "userMusicDetailList", &[]);
            Json::obj().with("userId", u.clone()).with("length", list.len()).with("nextIndex", next).with("userMusicList", list)
        }
        "GetUserItemApi" | "CMGetUserItemApi" => user_items(u, &doc, next, max),
        "GetUserOptionApi" => match doc.get("userOption") {
            Some(o) => Json::obj().with("userId", u.clone()).with("userOption", o.clone()),
            None => Json::obj(),
        },
        "GetUserDataApi" | "CMGetUserDataApi" => match doc.get("userData") {
            Some(d) => Json::obj().with("userId", u.clone()).with("userData", d.clone()),
            None => Json::obj(),
        },
        "GetUserEventRankingApi" => event_ranking(u, &user),
        "GetUserLoginBonusApi" => user_list(u, &doc, "userLoginBonusList", "userLoginBonusList"),
        "GetUserBpBaseApi" => user_list(u, &doc, "userBpBaseList", "userBpBaseList"),
        "GetUserRecentRatingApi" => user_list(u, &doc, "userRecentRatingList", "userRecentRatingList"),
        "GetUserActivityApi" => {
            let kind = text(req.get("kind"));
            let items: Vec<Json> = doc.a("userActivityList").iter().filter(|a| text(a.get("kind")) == kind).cloned().collect();
            Json::obj().with("userId", u.clone()).with("length", items.len()).with("kind", req.get("kind").cloned().unwrap_or(Json::num(0))).with("userActivityList", items)
        }
        "GetUserStoryApi" => user_list(u, &doc, "userStoryList", "userStoryList"),
        "GetUserChapterApi" => user_list(u, &doc, "userChapterList", "userChapterList"),
        "GetUserMemoryChapterApi" => memory_chapters(u, &doc),
        "GetUserTrainingRoomByKeyApi" => user_list(u, &doc, "userTrainingRoomList", "userTrainingRoomList"),
        "GetUserCharacterApi" => user_list(u, &doc, "userCharacterList", "userCharacterList"),
        "GetUserCardApi" => user_list(u, &doc, "userCardList", "userCardList"),
        "CMGetUserCardApi" => user_paged(u, &doc, "userCardList", next, max),
        "CMGetUserCharacterApi" => user_paged(u, &doc, "userCharacterList", next, max),
        "GetUserDeckByKeyApi" => user_list(u, &doc, "userDeckList", "userDeckList"),
        "GetUserTradeItemApi" => user_list(u, &doc, "userTradeItemList", "userTradeItemList"),
        "GetUserScenarioApi" => user_list(u, &doc, "userScenerioList", "userScenarioList"),
        "GetUserRatinglogApi" => user_list(u, &doc, "userRatinglogList", "userRatinglogList"),
        "GetUserMissionPointApi" => user_list(u, &doc, "userMissionPointList", "userMissionPointList"),
        "GetUserEventPointApi" => event_points(u, &doc),
        "GetUserMusicItemApi" => user_list(u, &doc, "userMusicItemList", "userMusicItemList"),
        "GetUserEventMusicApi" => user_list(u, &doc, "userEventMusicList", "userEventMusicList"),
        "GetUserBossApi" => user_list(u, &doc, "userBossList", "userBossList"),
        "GetUserGachaApi" => user_list(u, &doc, "userGachaList", "userGachaList"),
        "CMGetUserGachaSupplyApi" => Json::obj().with("supplyId", 1i64).with("length", 0i64).with("supplyCardList", Vec::<Json>::new()),
        "GetUserRivalApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userRivalList", Vec::<Json>::new()),
        "GetUserRivalDataApi" => {
            let names: HashMap<String, Json> = players().into_iter().filter_map(|(id, d)| Some((id, d.get("userData")?.get("userName")?.clone()))).collect();
            let list: Vec<Json> = req
                .a("userRivalList")
                .iter()
                .filter_map(|r| {
                    let id = r.get("rivalUserId")?;
                    let name = names.get(&text(Some(id)))?;
                    Some(Json::obj().with("rivalUserId", id.clone()).with("rivalUserName", name.clone()))
                })
                .collect();
            Json::obj().with("userId", u.clone()).with("length", list.len()).with("userRivalDataList", list)
        }
        "GetUserRivalMusicApi" => {
            let rival = req.get("rivalUserId").cloned().unwrap_or(Json::num(0));
            let rival_doc = load(&text(Some(&rival)));
            let drop = ["playCount", "isLock", "clearStatus", "isStoryWatched"];
            let (list, next) = music_pages(rival_doc.a("userMusicDetailList"), next, max, "userRivalMusicDetailList", &drop);
            Json::obj().with("userId", u.clone()).with("rivalUserId", rival).with("nextIndex", next).with("length", list.len()).with("userRivalMusicList", list)
        }
        "UpsertUserAllApi" => {
            if keep {
                upsert_all(&user, &mut doc, req.get("upsertUserAll").unwrap_or(&Json::Null));
                changed = true;
            } else {
                log!("ongeki: a guest's play, not kept");
            }
            ok("upsertUserAll")
        }
        "CMUpsertUserGachaApi" => {
            if keep {
                cm_upsert_gacha(&mut doc, req);
                changed = true;
            }
            ok("CMUpsertUserGachaApi")
        }
        "CMUpsertUserSelectGachaApi" => {
            if keep {
                let upsert = req.get("cmUpsertUserSelectGacha").cloned().unwrap_or(Json::obj());
                cm_upsert(&mut doc, &upsert, &["userCharacterList", "userCardList"]);
                for log in req.a("selectGachaLogList") {
                    let gacha = num(log.get("gachaId"));
                    let mut entry = doc.a("userGachaList").iter().find(|g| num(g.get("gachaId")) == gacha).cloned().unwrap_or(Json::obj().with("gachaId", gacha));
                    entry.set("selectPoint", 0i64);
                    entry.set("useSelectPoint", log.get("useSelectPoint").cloned().unwrap_or(Json::num(0)));
                    merge_list(&mut doc, "userGachaList", &[entry]);
                }
                changed = true;
            }
            ok("cmUpsertUserSelectGacha")
        }
        "CMUpsertUserAllApi" => {
            if keep {
                let upsert = req.get("cmUpsertUserAll").cloned().unwrap_or(Json::obj());
                cm_upsert(&mut doc, &upsert, &["userActivityList", "userItemList", "userCardList"]);
                changed = true;
            }
            ok("cmUpsertUserAll")
        }
        "CMUpsertUserPrintPlaylogApi" | "CMUpsertUserPrintlogApi" => {
            ok(api).with("orderId", 0i64).with("serialId", "11111111111111111111")
        }
        "CMUpsertUserPrintApi" => {
            let serial = serial();
            if keep && let Some(detail) = req.get("userPrintDetail") {
                let mut detail = detail.clone();
                if let Json::Obj(fields) = &mut detail {
                    fields.retain(|(k, _)| !matches!(k.as_str(), "orderId" | "printNumber"));
                }
                detail.set("serialId", serial.as_str());
                merge_list(&mut doc, "userPrintDetailList", &[detail]);
                changed = true;
            }
            Json::obj().with("returnCode", 1i64).with("serialId", serial).with("apiName", "CMUpsertUserPrintApi")
        }
        _ => {
            log!("ongeki: {api} not handled");
            Json::obj().with("returnCode", 1i64)
        }
    };
    if changed {
        save(&user, &doc);
    }
    reply
}

/// A request to `.../SDDT/<version>/<Api>`: its answer, packed as the request was (1.00 plain
/// JSON, later versions zlib).
fn title(path: &str, body: &[u8]) -> Vec<u8> {
    let api = path.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    let plain = body.iter().find(|b| !b.is_ascii_whitespace()).is_none_or(|&b| b == b'{');
    let answer = if api.eq_ignore_ascii_case("ping") {
        Json::obj().with("returnCode", 1i64)
    } else if api.len() == 32 && api.bytes().all(|b| b.is_ascii_hexdigit()) {
        log!("ongeki: encrypted request {api} (not answered)");
        Json::obj().with("stat", "0")
    } else {
        let text = if plain {
            String::from_utf8_lossy(body).into_owned()
        } else {
            miniz_oxide::inflate::decompress_to_vec_zlib(body).map(|t| String::from_utf8_lossy(&t).into_owned()).unwrap_or_default()
        };
        let req = Json::parse(&text).unwrap_or(Json::obj());
        let answer = answer(path, &api, &req);
        log!("ongeki: {api} {} -> {}", text.chars().take(1500).collect::<String>(), answer.to_string().chars().take(300).collect::<String>());
        answer
    };
    if plain && !api.eq_ignore_ascii_case("ping") {
        answer.to_string().into_bytes()
    } else {
        miniz_oxide::deflate::compress_to_vec_zlib(answer.to_string().as_bytes(), 6)
    }
}

pub(crate) fn init() {
    wal_payload_common::allnet::set_title(title);
}
