//! maimai FiNALE's title server (the old maimai series' `MaimaiServlet`): the game posts zlib
//! packed JSON (`Content-Encoding: deflate`) to `<ALL.Net uri>MaimaiServlet/<Api>` (the uri
//! `http://<router>/SDEY/197/`, the power-on answer of [`crate::allnetproc`]) and reads the
//! answer packed the same way; served in the payload's ALL.Net server
//! (`wal_payload_common::allnet`). Enabled with `WAL_RINGEDGE_ALLNET=maimai`.
//!
//! The answers follow ARTEMiS's maimai title server (WTFPL, gitea.tendokyu.moe/Dniel97/artemis,
//! `titles/mai2/base.py` `Mai2Base`, the handler of every version before maimai DX, and
//! `index.py`'s routes: `MaimaiServlet/old/...` answered `ok` / `{}`, `usbdl` `OK`, `deliver`
//! nothing, `api/movie` `{}`).
//!
//! The players are kept in the save folder (`WAL_RINGEDGE_SAVE`, default `sv`), `maimai\<user
//! id>.json`: the game's own records as it sent them in `UpsertUserAllApi` (`userData` without
//! its access code and user id, `userOption`, `userWebOption`, `userGradeStatus`, `userBoss`,
//! `userExtend`, `userGhost`, `userRating`, `userRecentRatingList`, and the lists merged by
//! their ids: ARTEMiS's tables' unique keys), the consecutive login days, and the plays (each
//! `userPlaylog`). The song rankings count the plays of every player. The game's events are
//! its own table (`data\tables\mmEvent.bin`: AES-128-CBC, the game's table key, then gzip and
//! UTF-16 text, ARTEMiS's `read.py`), every one open; no tickets, presents or tournaments (ARTEMiS's
//! tables filled by hand).
//!
//! maimai PiNK (SDCQ 1.60) speaks the protocol before `MaimaiServlet` ([`old_title`], read from
//! its executable: ARTEMiS lists the version but serves only `MaimaiServlet`): plain JSON GETs
//! `<uri>/<command>/<place>/<keychip>/...` (`gameconfig` {"GameConfig"}, `shopconfig`,
//! `logincheck` {"loginStatus": "OK"}, `userdata` the player's {"UserData"} or 404 for a new
//! one, `login` {"login": {"userId"}}), the credit's end a multipart `logout` (UserID,
//! UserData.z: the player's {"UserData"} zlib packed, PlayData.z), kept as `<user id>.userdata`.

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

/// Held for each request (load, change, save).
static LOCK: Mutex<()> = Mutex::new(());

/// The game's settings answered once (`GetGameSettingApi`, the boot's "Game Server" check): the
/// network process's accounting is enabled after it ([`crate::allnetproc`]).
pub(crate) static GAME_SETTING_SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The players' sessions (user id, Unix time of its `UserLoginApi`; ended by `UserLogoutApi`):
/// that player's automatic card waits (`crate::maimai`).
static SESSIONS: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new());

/// The player of `user` is logged in (for 20 minutes at most: a session the game never closed).
pub(crate) fn player_session(user: &str) -> bool {
    SESSIONS.lock().unwrap().iter().any(|(u, since)| u == user && now() - since < 20 * 60)
}

fn session(user: &str, logged_in: bool) {
    let mut sessions = SESSIONS.lock().unwrap();
    sessions.retain(|(u, since)| u != user && now() - since < 20 * 60);
    if logged_in {
        sessions.push((user.to_string(), now()));
    }
}

/// The tables' AES-128 key (`WAL_RINGEDGE_MAIMAI_TABLE_KEY`, hex: another version's), maimai
/// FiNALE's (the game image's +0x6B8500).
const TABLE_KEY: &str = "4e2039d03c7ac1a36d10bac801321d26";

fn dir() -> std::path::PathBuf {
    let save = std::env::var("WAL_RINGEDGE_SAVE").unwrap_or_else(|_| "sv".into());
    std::path::Path::new(save.trim()).join("maimai")
}

fn user_path(user: &str) -> std::path::PathBuf {
    dir().join(format!("{user}.json"))
}

fn load_file(path: &std::path::Path) -> Json {
    std::fs::read_to_string(path).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_))).unwrap_or(Json::obj())
}

fn save(user: &str, doc: &Json) {
    let _ = std::fs::create_dir_all(dir());
    let path = user_path(user);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Every stored player's document (the rankings).
fn players() -> Vec<Json> {
    let Ok(entries) = std::fs::read_dir(dir()) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().strip_suffix(".json").is_some_and(|u| u.parse::<u64>().is_ok()))
        .map(|e| load_file(&e.path()))
        .collect()
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

/// `obj` without `keys`.
fn without(obj: &Json, keys: &[&str]) -> Json {
    match obj {
        Json::Obj(fields) => Json::Obj(fields.iter().filter(|(k, _)| !keys.contains(&k.as_str())).cloned().collect()),
        other => other.clone(),
    }
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

/// Days since 1970 of a `YYYY-MM-DD...` date.
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (parts.next()??, parts.next()??, parts.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn now() -> i64 {
    wal_payload_common::clock::unix_now() as i64
}

/// The stored lists (as the game sends them in `upsertUserAll`) and the fields identifying
/// their entries (ARTEMiS's tables' unique keys).
const LISTS: [(&str, &[&str]); 12] = [
    ("userActivityList", &["kind", "id"]),
    ("userChargeList", &["chargeId"]),
    ("userCharacterList", &["characterId"]),
    ("userItemList", &["itemKind", "itemId"]),
    ("userLoginBonusList", &["bonusId"]),
    ("userMapList", &["mapId"]),
    ("userMusicDetailList", &["musicId", "level"]),
    ("userCourseList", &["courseId"]),
    ("userFavoriteList", &["kind"]),
    ("userFriendSeasonRankingList", &["seasonId"]),
    ("userCardList", &["cardId"]),
    ("userPresentList", &["itemKind", "itemId"]),
];

/// Merges `entries` into the document's `list` (an entry with the same ids replaced).
fn merge(doc: &mut Json, list: &str, entries: &[Json]) {
    let Some((_, keys)) = LISTS.iter().find(|(l, _)| *l == list) else { return };
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

fn ok(api: &str) -> Json {
    Json::obj().with("returnCode", 1i64).with("apiName", api)
}

/// `items[next..next + max]` and the next index (0: the end).
fn page(items: &[Json], next: i64, max: i64) -> (Vec<Json>, i64) {
    let start = (next.max(0) as usize).min(items.len());
    let max = if max <= 0 { items.len() } else { max as usize };
    let end = (start + max).min(items.len());
    (items[start..end].to_vec(), if end < items.len() { end as i64 } else { 0 })
}

/// The game's event ids, from its table (`mmEvent.bin`; ARTEMiS's reader: AES-128-CBC with a
/// throwaway first block, gzip, 0x12 bytes, UTF-16LE text, a `MMEVENT( id, ... )` line each).
fn event_ids() -> &'static [i64] {
    static IDS: std::sync::OnceLock<Vec<i64>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        let text = match read_table("mmEvent.bin") {
            Some(t) => t,
            None => {
                log!("maimai server: data\\tables\\mmEvent.bin not read: no events");
                return Vec::new();
            }
        };
        let ids: Vec<i64> = text.lines().filter_map(|l| l.trim().strip_prefix("MMEVENT(")?.split(',').next()?.trim().parse().ok()).collect();
        log!("maimai server: {} events", ids.len());
        ids
    })
}

/// A table of `data\tables`, decrypted and unpacked; the earlier versions' plain UTF-16 text
/// (maimai PiNK's `.tbl`) when there.
fn read_table(name: &str) -> Option<String> {
    use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
    if let Ok(data) = std::fs::read(format!("data\\tables\\{}", name.replace(".bin", ".tbl")))
        && data.starts_with(&[0xff, 0xfe])
    {
        let utf16: Vec<u16> = data[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&utf16));
    }
    let data = std::fs::read(format!("data\\tables\\{name}")).ok()?;
    let key_hex = std::env::var("WAL_RINGEDGE_MAIMAI_TABLE_KEY").unwrap_or_else(|_| TABLE_KEY.into());
    let key: Vec<u8> = (0..key_hex.trim().len() / 2).filter_map(|i| u8::from_str_radix(key_hex.trim().get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    let cipher = aes::Aes128::new_from_slice(&key).ok()?;
    let mut plain = Vec::with_capacity(data.len());
    for (i, block) in data.chunks_exact(16).enumerate().skip(1) {
        let mut b = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut b);
        plain.extend(b.iter().zip(&data[(i - 1) * 16..i * 16]).map(|(x, c)| x ^ c));
    }
    let unpacked = gunzip(&plain)?;
    let utf16: Vec<u16> = unpacked.get(0x12..)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&utf16))
}

/// A gzip member's content.
fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    if data.get(..3)? != [0x1f, 0x8b, 8] {
        return None;
    }
    let flags = data[3];
    let mut at = 10;
    if flags & 4 != 0 {
        at += 2 + u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]) as usize;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            at += data.get(at..)?.iter().position(|&b| b == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(data.get(at..)?).ok()
}

/// `GetGameSettingApi`: the reboot hours a few hours from now (ARTEMiS's default), the old /
/// movie / delivery servers this one (none for delivery and USB downloads).
fn game_setting() -> Json {
    let old_server = format!("{}MaimaiServlet/", crate::allnetproc::title_uri());
    let setting = Json::obj()
        .with("isMaintenance", false)
        .with("requestInterval", 1800i64)
        .with("rebootStartTime", date_time(now() + 6 * 3600))
        .with("rebootEndTime", date_time(now() + 7 * 3600))
        .with("movieUploadLimit", 100i64)
        .with("movieStatus", 1i64)
        .with("movieServerUri", "movie")
        .with("deliverServerUri", "")
        .with("oldServerUri", format!("{old_server}old"))
        .with("usbDlServerUri", "");
    Json::obj().with("isDevelop", false).with("isAouAccession", false).with("gameSetting", setting)
}

/// `GetGameRankingApi`: the 100 songs played most (every player's plays).
fn game_ranking() -> Json {
    let mut counts: Vec<(String, i64)> = Vec::new();
    for doc in players() {
        for play in doc.a("userPlaylogList") {
            let id = text(play.get("musicId"));
            match counts.iter_mut().find(|(m, _)| *m == id) {
                Some((_, n)) => *n += 1,
                None => counts.push((id, 1)),
            }
        }
    }
    if counts.is_empty() {
        return Json::obj().with("length", 0i64).with("gameRankingList", Vec::<Json>::new());
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let list: Vec<Json> = counts.iter().take(100).map(|(id, n)| Json::obj().with("id", Json::num(id)).with("point", *n).with("userName", "")).collect();
    Json::obj().with("type", 1i64).with("gameRankingList", list).with("gameRankingInstantList", Json::Null)
}

fn game_events(req: &Json) -> Json {
    let kind = req.get("type").cloned().unwrap_or(Json::num(0));
    let events: Vec<Json> = event_ids()
        .iter()
        .map(|id| Json::obj().with("type", 0i64).with("id", *id).with("startDate", "2017-12-05 07:00:00.0").with("endDate", "2099-12-31 00:00:00.0"))
        .collect();
    Json::obj().with("type", kind).with("length", events.len()).with("gameEventList", events)
}

/// `GetUserPreviewApi`: the player's summary; a new player (registration) only the user id
/// (ARTEMiS answers nothing: FiNALE keeps the preview's `userId` as the player's, 0 then, and
/// saves the play as a guest's).
fn user_preview(user: &Json, doc: &Json) -> Json {
    let (Some(data), Some(web)) = (doc.get("userData"), doc.get("userWebOption")) else { return Json::obj().with("userId", user.clone()) };
    let f = |k: &str| data.get(k).cloned().unwrap_or(Json::Null);
    let w = |k: &str| web.get(k).cloned().unwrap_or(Json::Null);
    Json::obj()
        .with("userId", user.clone())
        .with("userName", f("userName"))
        .with("isLogin", false)
        .with("lastDataVersion", f("lastDataVersion"))
        .with("lastLoginDate", f("lastPlayDate"))
        .with("lastPlayDate", f("lastPlayDate"))
        .with("playerRating", f("playerRating"))
        .with("nameplateId", f("nameplateId"))
        .with("frameId", f("frameId"))
        .with("iconId", f("iconId"))
        .with("trophyId", f("trophyId"))
        .with("dispRate", w("dispRate"))
        .with("dispRank", w("dispRank"))
        .with("dispHomeRanker", w("dispHomeRanker"))
        .with("dispTotalLv", w("dispTotalLv"))
        .with("totalLv", f("totalLv"))
}

/// `UserLoginApi`: the last login, the play count, the consecutive login days (counted here).
fn user_login(doc: &mut Json, req: &Json) -> Json {
    let data = doc.get("userData").cloned();
    let (last, count) = match &data {
        // FiNALE's player record has no lastLoginDate: its last play's date
        Some(d) => (text(d.get("lastLoginDate").or(d.get("lastPlayDate"))), num(d.get("playCount"))),
        None => ("2017-12-05 07:00:00.0".to_string(), 0),
    };
    if let (Some(region), Some(_)) = (req.get("regionId"), &data) {
        doc.set("regionId", region.clone());
    }
    let today = now().div_euclid(86400);
    let streak = doc.get("consecutiveLogin").cloned().unwrap_or(Json::obj());
    let (mut logins, seen) = (num(streak.get("logins")), num(streak.get("day")));
    logins = if logins == 0 || data.is_none() {
        1
    } else if day_number(&last).is_some_and(|d| d < today - 1) || seen < today - 1 {
        1
    } else if seen < today {
        logins + 1
    } else {
        logins
    };
    doc.set("consecutiveLogin", Json::obj().with("logins", logins).with("day", today));
    Json::obj().with("returnCode", 1i64).with("lastLoginDate", last).with("loginCount", count).with("consecutiveLoginCount", logins)
}

/// `UpsertUserAllApi`: the game's records kept (a guest's ignored).
fn upsert_all(doc: &mut Json, upsert: &Json) {
    let first = |k: &str| upsert.a(k).first().cloned();
    if let Some(d) = first("userData") {
        doc.set("userData", without(&d, &["accessCode", "userId"]));
    }
    if let Some(mut w) = first("userWebOption") {
        w.set("isNetMember", true);
        doc.set("userWebOption", w);
    }
    if let Some(g) = first("userGradeStatusList") {
        doc.set("userGradeStatus", g);
    }
    if let Some(b) = first("userBossList") {
        doc.set("userBoss", b);
    }
    if let Some(e) = first("userExtend") {
        doc.set("userExtend", e);
    }
    if let Some(Json::Arr(ghosts)) = upsert.get("userGhost")
        && let Some(g) = ghosts.last()
    {
        doc.set("userGhost", g.clone());
    }
    if let Some(r) = upsert.get("userRecentRatingList") {
        doc.set("userRecentRatingList", r.clone());
    }
    if let Some(o) = first("userOption") {
        doc.set("userOption", without(&o, &["userId"]));
    }
    if let Some(r) = first("userRatingList") {
        doc.set("userRating", r);
    }
    let plays = upsert.a("userPlaylogList");
    if !plays.is_empty() {
        let mut all = doc.a("userPlaylogList").to_vec();
        all.extend(plays.iter().cloned());
        doc.set("userPlaylogList", all);
    }
    for (list, _) in LISTS {
        let entries = upsert.a(list);
        if !entries.is_empty() {
            let entries: Vec<Json> = entries
                .iter()
                .map(|e| {
                    // ARTEMiS strips the ".0" of the charges' purchase date
                    if list == "userChargeList" {
                        let mut e = e.clone();
                        let date = text(e.get("purchaseDate")).replace(".0", "");
                        e.set("purchaseDate", date);
                        e
                    } else {
                        e.clone()
                    }
                })
                .collect();
            merge(doc, list, &entries);
        }
    }
}

/// `GetUserMusicApi`: the scores grouped by song (`userMusicDetailList` each), `max` songs a page.
fn user_music(user: &Json, doc: &Json, next: i64, max: i64) -> Json {
    let mut details = doc.a("userMusicDetailList").to_vec();
    details.sort_by_key(|d| (num(d.get("musicId")), num(d.get("level"))));
    let mut groups: Vec<Vec<Json>> = Vec::new();
    for d in details {
        match groups.last_mut() {
            Some(g) if text(g[0].get("musicId")) == text(d.get("musicId")) => g.push(d),
            _ => groups.push(vec![d]),
        }
    }
    let max = if max <= 0 { 50 } else { max };
    let (songs, next) = page(&groups.into_iter().map(|g| Json::obj().with("userMusicDetailList", g)).collect::<Vec<_>>(), next, max);
    Json::obj().with("userId", user.clone()).with("nextIndex", next).with("userMusicList", songs)
}

fn answer(api: &str, req: &Json) -> Json {
    let user_json = req.get("userId").cloned().unwrap_or(Json::num(0));
    let user = text(Some(&user_json));
    let next = num(req.get("nextIndex"));
    let max = num(req.get("maxCount"));
    // a guest's play (or(0x1000000000001, place id bits)) is not kept
    let guest = user.parse::<u64>().is_ok_and(|u| u & 0x1000000000001 == 0x1000000000001);
    let keep = user.parse::<u64>().is_ok_and(|u| u > 0) && !guest;
    let _lock = LOCK.lock().unwrap();
    let mut doc = if keep { load_file(&user_path(&user)) } else { Json::obj() };
    let u = &user_json;
    let mut changed = false;
    let reply = match api {
        "GetGameSettingApi" => {
            GAME_SETTING_SENT.store(true, std::sync::atomic::Ordering::Relaxed);
            game_setting()
        }
        "GetGameRankingApi" => game_ranking(),
        "GetGameTournamentInfoApi" => Json::obj().with("length", 0i64).with("gameTournamentInfoList", Vec::<Json>::new()),
        "GetGameEventApi" => game_events(req),
        "GetGameNgMusicIdApi" => Json::obj().with("length", 0i64).with("musicIdList", Vec::<Json>::new()),
        "GetGameNgWordListApi" => Json::obj()
            .with("ngWordExactMatchLength", 0i64)
            .with("ngWordExactMatchList", Vec::<Json>::new())
            .with("ngWordPartialMatchLength", 0i64)
            .with("ngWordPartialMatchList", Vec::<Json>::new()),
        "GetGameChargeApi" => Json::obj().with("length", 0i64).with("gameChargeList", Vec::<Json>::new()),
        "UpsertClientSettingApi" | "UpsertClientUploadApi" | "UpsertClientBookkeepingApi" | "UpsertClientTestmodeApi" => ok(api),
        "GetUserPreviewApi" => user_preview(u, &doc),
        "UserLoginApi" => {
            session(&user, true);
            changed = keep;
            user_login(&mut doc, req)
        }
        "UploadUserPlaylogApi" => {
            if let Some(p) = req.get("userPlaylog") {
                let mut all = doc.a("userPlaylogList").to_vec();
                all.push(p.clone());
                doc.set("userPlaylogList", all);
                changed = keep;
            }
            ok("UploadUserPlaylogApi")
        }
        "UpsertUserChargelogApi" => {
            if let Some(c) = req.get("userCharge") {
                let mut c = c.clone();
                let date = text(c.get("purchaseDate")).replace(".0", "");
                c.set("purchaseDate", date);
                merge(&mut doc, "userChargeList", &[c]);
                changed = keep;
            }
            ok("UpsertUserChargelogApi")
        }
        "UpsertUserAllApi" => {
            if guest {
                log!("maimai server: guest play, not kept");
            } else if let Some(upsert) = req.get("upsertUserAll") {
                upsert_all(&mut doc, upsert);
                changed = keep;
            }
            ok("UpsertUserAllApi")
        }
        "UserLogoutApi" => {
            session(&user, false);
            Json::obj().with("returnCode", 1i64)
        }
        "GetUserDataApi" => match doc.get("userData") {
            Some(d) => Json::obj().with("userId", u.clone()).with("userData", d.clone()),
            None => Json::Null,
        },
        "GetUserExtendApi" => match doc.get("userExtend") {
            Some(e) => Json::obj().with("userId", u.clone()).with("userExtend", e.clone()),
            None => Json::Null,
        },
        "GetUserOptionApi" => match doc.get("userOption") {
            Some(o) => Json::obj().with("userId", u.clone()).with("userOption", o.clone()),
            None => Json::Null,
        },
        "GetUserCardApi" => {
            let (cards, next) = page(doc.a("userCardList"), next, max);
            Json::obj().with("userId", u.clone()).with("nextIndex", next).with("userCardList", cards)
        }
        "GetUserChargeApi" => {
            let charges = doc.a("userChargeList").to_vec();
            Json::obj().with("userId", u.clone()).with("length", charges.len()).with("userChargeList", charges)
        }
        "GetUserPresentApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userPresentList", Vec::<Json>::new()),
        "GetTransferFriendApi" => Json::obj(),
        "GetUserPresentEventApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userPresentEventList", Vec::<Json>::new()),
        "GetUserBossApi" => Json::obj().with("userId", u.clone()).with("userBossData", doc.get("userBoss").cloned().unwrap_or(Json::obj())),
        "GetUserItemApi" => {
            // nextIndex: the kind times 10^10 plus the index in that kind
            let (kind, index) = (next / 10_000_000_000, next % 10_000_000_000);
            let items: Vec<Json> = doc.a("userItemList").iter().filter(|i| num(i.get("itemKind")) == kind).cloned().collect();
            let (items, n) = page(&items, index, max);
            let next = if n == 0 { 0 } else { kind * 10_000_000_000 + n };
            Json::obj().with("userId", u.clone()).with("nextIndex", next).with("itemKind", kind).with("userItemList", items)
        }
        "GetUserCharacterApi" => {
            let list: Vec<Json> = doc.a("userCharacterList").iter().map(|c| without(c, &["awakening", "useCount"])).collect();
            Json::obj().with("userId", u.clone()).with("userCharacterList", list)
        }
        "GetUserFavoriteApi" => {
            let kind = text(req.get("itemKind"));
            let list: Vec<Json> = doc
                .a("userFavoriteList")
                .iter()
                .filter(|f| text(f.get("kind")) == kind)
                .map(|f| Json::obj().with("userId", u.clone()).with("itemKind", f.get("kind").cloned().unwrap_or(Json::Null)).with("itemIdList", f.get("itemIdList").cloned().unwrap_or(Json::Arr(Vec::new()))))
                .collect();
            Json::obj().with("userId", u.clone()).with("userFavoriteData", list)
        }
        "GetUserGhostApi" => match doc.get("userGhost") {
            Some(g) => Json::obj().with("userId", u.clone()).with("userGhost", g.clone()),
            None => Json::Null,
        },
        "GetUserRecentRatingApi" => match doc.get("userRecentRatingList") {
            Some(r) => Json::obj().with("userId", u.clone()).with("length", r.items().len()).with("userRecentRatingList", r.clone()),
            None => Json::Null,
        },
        "GetUserRatingApi" => match doc.get("userRating") {
            Some(r) => Json::obj().with("userId", u.clone()).with("userRating", r.clone()),
            None => Json::Null,
        },
        "GetUserActivityApi" => match req.get("kind") {
            // FiNALE asks for each kind (1-6) and reads `userActivityList` (ARTEMiS's
            // `userActivity` play / music lists are maimai DX's)
            Some(kind) => {
                let list: Vec<Json> = doc.a("userActivityList").iter().filter(|a| text(a.get("kind")) == text(Some(kind))).cloned().collect();
                Json::obj().with("userId", u.clone()).with("kind", kind.clone()).with("length", list.len()).with("userActivityList", list)
            }
            None => {
                // kind 1 the plays, 2 the songs
                let of = |kind: i64| -> Vec<Json> { doc.a("userActivityList").iter().filter(|a| num(a.get("kind")) == kind).map(|a| without(a, &["kind"])).collect() };
                Json::obj().with("userActivity", Json::obj().with("playList", of(1)).with("musicList", of(2)))
            }
        },
        "GetUserCourseApi" => Json::obj().with("userId", u.clone()).with("nextIndex", 0i64).with("userCourseList", doc.a("userCourseList").to_vec()),
        "GetUserPortraitApi" => Json::obj().with("length", 0i64).with("userPortraitList", Vec::<Json>::new()),
        "GetUserFriendSeasonRankingApi" => {
            let (list, next) = page(doc.a("userFriendSeasonRankingList"), next, max);
            Json::obj().with("userId", u.clone()).with("nextIndex", next).with("userFriendSeasonRankingList", list)
        }
        "GetUserMapApi" => {
            let (list, next) = page(doc.a("userMapList"), next, max);
            Json::obj().with("userId", u.clone()).with("nextIndex", next).with("userMapList", list)
        }
        "GetUserLoginBonusApi" => {
            let (list, next) = page(doc.a("userLoginBonusList"), next, max);
            Json::obj().with("userId", u.clone()).with("nextIndex", next).with("userLoginBonusList", list)
        }
        "GetUserRegionApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userRegionList", Vec::<Json>::new()),
        "GetUserWebOptionApi" => Json::obj().with("userId", u.clone()).with("userWebOption", doc.get("userWebOption").cloned().unwrap_or(Json::obj())),
        "GetUserSurvivalApi" => Json::obj().with("userId", u.clone()).with("length", 0i64).with("userSurvivalList", Vec::<Json>::new()),
        "GetUserGradeApi" => Json::obj()
            .with("userId", u.clone())
            .with("userGradeStatus", doc.get("userGradeStatus").cloned().unwrap_or(Json::obj()))
            .with("length", 0i64)
            .with("userGradeList", Vec::<Json>::new()),
        // ARTEMiS: nothing without a user id
        "GetUserMusicApi" => {
            if user.parse::<u64>().is_ok_and(|u| u > 0) { user_music(u, &doc, next, max) } else { Json::obj() }
        }
        "UploadUserPortraitApi" => Json::obj().with("returnCode", 0i64).with("apiName", "UploadUserPortraitApi"),
        "UploadUserPhotoApi" => Json::obj().with("returnCode", 0i64).with("apiName", "UploadUserPhotoApi"),
        "GetUserFavoriteItemApi" => Json::obj()
            .with("userId", u.clone())
            .with("kind", req.get("kind").cloned().unwrap_or(Json::num(0)))
            .with("nextIndex", 0i64)
            .with("userFavoriteItemList", Vec::<Json>::new()),
        "GetUserRecommendRateMusicApi" => Json::obj().with("userId", u.clone()).with("userRecommendRateMusicIdList", Vec::<Json>::new()),
        "GetUserRecommendSelectMusicApi" => Json::obj().with("userId", u.clone()).with("userRecommendSelectionMusicIdList", Vec::<Json>::new()),
        "GetUserScoreRankingApi" => Json::obj().with("userId", u.clone()).with("userScoreRanking", Vec::<Json>::new()),
        "Ping" | "PingApi" => Json::obj().with("returnCode", 1i64),
        _ => {
            log!("maimai server: {api} not handled");
            Json::obj().with("returnCode", 1i64)
        }
    };
    if changed {
        save(&user, &doc);
    }
    // ARTEMiS: a handler's None is {"returnCode": 1}
    if matches!(reply, Json::Null) { Json::obj().with("returnCode", 1i64) } else { reply }
}

/// A request to `.../MaimaiServlet/...`: its zlib packed answer.
pub(crate) fn title(path: &str, body: &[u8]) -> Vec<u8> {
    if !path.contains("MaimaiServlet/") {
        return old_title(path, body);
    }
    let rest = path.split_once("MaimaiServlet/").map_or("", |(_, r)| r).split('?').next().unwrap_or("");
    let pack = |text: &str| miniz_oxide::deflate::compress_to_vec_zlib(text.as_bytes(), 6);
    if rest.starts_with("old/") {
        log!("maimai server: old server {rest}");
        // a player's data of the earlier versions (`userdata180/<place>/<keychip>/<user id>`,
        // read as plain JSON): none, as the old server answers an unknown player (ARTEMiS's
        // zlib packed "{}" is read as old data: the new player then plays as a guest)
        if rest.starts_with("old/userdata") {
            let body = "User not found.";
            return format!("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes();
        }
        // ARTEMiS's old server: "ok"
        return pack("ok");
    }
    if rest.starts_with("usbdl/") {
        return b"OK".to_vec();
    }
    if rest.starts_with("deliver/") {
        return Vec::new();
    }
    if rest.starts_with("api/movie/") {
        return b"{}".to_vec();
    }
    let api = rest.rsplit('/').next().unwrap_or("");
    let text = match miniz_oxide::inflate::decompress_to_vec_zlib(body) {
        Ok(t) => String::from_utf8_lossy(&t).into_owned(),
        Err(_) => {
            log!("maimai server: {path}: request not zlib packed ({:02x?})", &body[..body.len().min(32)]);
            return pack(r#"{"stat": "0"}"#);
        }
    };
    let req = Json::parse(&text).unwrap_or(Json::obj());
    let answer = answer(api, &req);
    log!("maimai server: {api} {} -> {}", text.chars().take(600).collect::<String>(), answer.to_string().chars().take(300).collect::<String>());
    pack(&answer.to_string())
}

/// maimai PiNK's title server (the protocol before `MaimaiServlet`): `<uri>/<command>/...`.
fn old_title(path: &str, body: &[u8]) -> Vec<u8> {
    let rest = path.split_once("/SDCQ/").map_or(path, |(_, r)| r);
    let parts: Vec<&str> = rest.split(['/', '?']).filter(|p| !p.is_empty()).collect();
    let command = parts.get(1).copied().unwrap_or("");
    log!("maimai old server: {path} ({} bytes: {})", body.len(), String::from_utf8_lossy(&body[..body.len().min(400)]).replace(['\r', '\n'], " "));
    let json = |text: String| text.into_bytes();
    match command {
        "gameconfig" => {
            GAME_SETTING_SENT.store(true, std::sync::atomic::Ordering::Relaxed);
            json(r#"{"GameConfig":{"UpdateInterval":60,"Maintenance":false,"UploadLimit":0,"OpenOnlineMusic":true,"AllowMovie":false,"AllowDemoPlay":true,"AllowMovieForNew":false,"NeedReboot":false,"RebootBegin":[4,0],"RebootEnd":[5,0],"DeliverVersion":0,"MovieServer":"","DeliverServer":""}}"#.into())
        }
        "logincheck" => json(r#"{"loginStatus":"OK"}"#.into()),
        // `login/<place>/<keychip>/<user id>?serial=<play serial>`: the player's session
        // (`player_session`), the user id answered; `logout/...` carries the player's records
        // (multipart: `old_upload`)
        "login" | "logout" => {
            let user = parts.get(4).copied().unwrap_or("");
            session(user, command == "login");
            if body.starts_with(b"--") {
                old_upload(body);
            }
            if command == "login" { json(format!(r#"{{"login":{{"userId":{}}}}}"#, num(Some(&Json::Str(user.into()))))) } else { json("{}".into()) }
        }
        "userdata" => {
            let user = parts.last().copied().unwrap_or("");
            match std::fs::read(dir().join(format!("{user}.userdata"))) {
                Ok(data) => data,
                Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            }
        }
        _ if body.starts_with(b"--") => {
            old_upload(body);
            json("{}".into())
        }
        _ => json("{}".into()),
    }
}

/// A multipart/form-data body's parts: (name, content).
fn form_parts(body: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(eol) = body.windows(2).position(|w| w == b"\r\n") else { return Vec::new() };
    let boundary = [b"\r\n".as_slice(), &body[..eol]].concat();
    let mut parts = Vec::new();
    let mut at = eol + 2;
    while at < body.len() {
        let end = body[at..].windows(boundary.len()).position(|w| w == boundary.as_slice()).map_or(body.len(), |i| at + i);
        let part = &body[at..end];
        if let Some(h) = part.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&part[..h]);
            let name = head.split("name=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("").to_string();
            parts.push((name, part[h + 4..].to_vec()));
        }
        at = end + boundary.len() + 2;
    }
    parts
}

/// The player's records uploaded at the end of a credit (`UserID`, `UserData` or `UserData.z`:
/// the game's JSON, zlib packed for `.z`), kept as `<user id>.userdata` for `userdata`.
fn old_upload(body: &[u8]) {
    let parts = form_parts(body);
    let field = |name: &str| parts.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice());
    let user = String::from_utf8_lossy(field("UserID").unwrap_or_default()).trim().to_string();
    let data = match (field("UserData.z"), field("UserData")) {
        (Some(z), _) => miniz_oxide::inflate::decompress_to_vec_zlib(z).ok(),
        (None, Some(d)) => Some(d.to_vec()),
        _ => None,
    };
    log!("maimai old server: upload, parts {:?}, user {user}, user data {} bytes", parts.iter().map(|(n, v)| (n.as_str(), v.len())).collect::<Vec<_>>(), data.as_ref().map_or(0, |d| d.len()));
    if let Some(data) = data.filter(|_| user.parse::<u64>().is_ok()) {
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(dir().join(format!("{user}.userdata")), &data);
    }
}
