//! Theatrhythm's card server answers (`theatrhythm_server.rs`): the players' records, one per
//! card (`theatrhythm-server/<card id>.json`), made at the card's first login and kept up to
//! date with what the game reports after each credit.
//!
//! What the game expects (its card server client, `Siva.Network.ServerManager`):
//! * every answer HTTP 200 with `result` `OK`, or `NG` and an error `code`;
//! * `user/profile` `NG #004`: a card without Theatrhythm data (first play);
//! * `service/version` the game's version (sent with each request);
//! * `service/echo` a body that is not JSON; `event/musicscoreranking` no answer (404);
//! * the optional services (play bonus, score ranking, domination, raid boss) `NG` (no event);
//! * 3 parties of 4 characters; the song list 0, or 11 or more songs but not 20 (the client
//!   asks for the songs' records 10 at a time and divides by zero otherwise): all the songs a
//!   new player has (171, below) plus the ones unlocked; 6 levels of each song's records;
//!   only the items `item_0001` to `item_0008`.
//!
//! What is kept: `user/playupdate` (each credit: gil, rhythm points, sound medals, items and
//! crystals gained, the characters' experience, scores, parties, settings), `user/playreward`
//! (characters and songs unlocked, collection cards, medals and crystals after spending, the
//! title chosen), `user/playresult` (titles), `user/infoupdate`, `user/shopbuy`.
//!
//! `WAL_THEATRHYTHM_PLAYER_NAME`: the players' name (none: the game's, 名もなき演奏者).

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

/// The players' records' folder (in the game's folder).
const DIR: &str = "theatrhythm-server";

/// The songs of a new player: the medleys neither hidden from the music select nor secret
/// (`MedleyTable`, all delivered by 2018).
const DEFAULT_SONGS: &[&str] = &[
    "00101_1_001", "00101_0_003", "00101_0_005", "00101_0_007", "00101_0_017", "00102_0_017", "00102_0_032", "00201_1_001",
    "00201_0_002", "00201_0_005", "00201_0_007", "00201_0_011", "00301_1_001", "00301_0_003", "00301_0_007", "00301_0_022",
    "00301_0_040", "00301_0_042", "00401_1_001", "00401_0_002", "00401_0_007", "00401_0_008", "00401_0_013", "00401_0_028",
    "00401_0_029", "00501_1_001", "00501_0_003", "00501_0_007", "00501_0_025", "00501_0_031", "00501_0_037", "00501_0_049",
    "00601_1_001", "00601_0_005", "00601_0_007", "00601_0_024", "00601_0_026", "00601_0_048", "00601_0_052", "00701_1_001",
    "00701_0_010", "00701_0_020", "00701_0_024", "00701_0_037", "00701_0_042", "00701_0_045", "00701_0_070", "00704_0_051",
    "00704_0_052", "00801_1_001", "00801_0_002", "00801_0_003", "00801_0_004", "00801_0_010", "00801_0_015", "00801_0_019",
    "00801_0_049", "00801_0_064", "00801_0_068", "00901_1_001", "00901_0_007", "00901_0_018", "00901_0_025", "00901_0_037",
    "00901_0_060", "00901_0_085", "00901_0_093", "00901_0_105", "01001_1_001", "01001_0_009", "01001_0_037", "01001_0_050",
    "01001_0_078", "01001_0_085", "01001_0_087", "01002_0_007", "01101_1_001", "01101_0_004", "01101_0_005", "01101_0_009",
    "01101_0_021", "01102_0_011", "01102_0_014", "01201_1_001", "01201_0_002", "01201_0_009", "01201_0_015", "01201_0_032",
    "01201_0_079", "01301_1_001", "01301_0_005", "01301_0_020", "01301_0_035", "01301_0_072", "01302_0_017", "01302_0_023",
    "01303_0_001", "01303_0_033", "01401_1_001", "01402_0_002", "01402_0_005", "01402_0_067", "01402_0_080", "01402_0_156",
    "01402_0_163", "01403_0_057", "01403_0_084", "01403_0_113", "01501_1_001", "01501_0_007", "01501_0_034", "01501_0_039",
    "01501_0_057", "01501_0_062", "10101_0_007", "10101_0_024", "10101_0_025", "10201_1_001", "10201_0_006", "10201_0_009",
    "10201_0_045", "10301_1_001", "10301_0_004", "10301_0_024", "10301_0_047", "10401_1_001", "10401_0_002", "10703_0_031",
    "10703_0_032", "10703_0_056", "10901_0_007", "10901_0_087", "11201_0_003", "11201_0_019", "11004_0_001", "11004_0_002",
    "11004_0_004", "11004_0_005", "11004_0_006", "11004_0_007", "00000_1_001", "00000_1_002", "00000_1_003", "00000_1_004",
    "00000_1_005", "00000_1_006", "00000_1_007", "00000_1_008", "29001_1_001", "11101_0_010", "29012_0_001", "30211_0_033",
    "30212_0_023", "30221_0_052", "30301_0_015", "30302_0_006", "30302_0_019", "30311_0_006", "30311_0_048", "30401_0_002",
    "30401_0_047", "30401_0_051", "30401_0_060", "30501_0_002", "30601_0_035", "30601_0_016", "30702_0_007", "30702_0_008",
    "30702_0_012", "31101_0_028", "31101_0_034",
];

/// The game's default party (`UserData.Create`): Warrior of Light, Firion, Onion Knight, Cecil.
const DEFAULT_PARTY: [&str; 4] = ["ply00100_01a", "ply00200_01a", "ply00300_01a", "ply00400_01a"];
/// The game's default title.
const DEFAULT_TITLE: &str = "titl00000_0010";
const ITEMS: usize = 8;
const CRYSTALS: usize = 8;

/// One answer of the server.
pub(crate) enum Answer {
    Json(Json),
    /// Plain text (`service/echo`).
    Text(&'static str),
    /// HTTP 404.
    NotFound,
}

fn ok() -> Json {
    Json::obj().with("result", "OK").with("code", "").with("message", "")
}

fn ng(code: &str) -> Json {
    Json::obj().with("result", "NG").with("code", code).with("message", "")
}

/// The game's version (its requests' `GameVersion` header): the server's (`service/version`),
/// which the game checks (error 31001 otherwise).
static GAME_VERSION: Mutex<String> = Mutex::new(String::new());

pub(crate) fn set_game_version(version: &str) {
    let mut v = GAME_VERSION.lock().unwrap();
    if *v != version {
        *v = version.to_string();
    }
}

/// The records loaded (by card id), each saved after a change.
static PLAYERS: Mutex<Vec<(String, Json)>> = Mutex::new(Vec::new());

fn path(card: &str) -> String {
    let safe: String = card.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{DIR}/{safe}.json")
}

/// The player id of a card: a stable number from its id (under 2^31: the game keeps a uint).
fn player_id(card: &str) -> u64 {
    let mut h: u32 = 0x811c9dc5;
    for b in card.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    (h & 0x7fff_ffff).max(1) as u64
}

/// The record of `card`, loaded (`None`: no Theatrhythm data yet).
fn load(players: &mut Vec<(String, Json)>, card: &str) -> Option<usize> {
    if let Some(i) = players.iter().position(|(c, _)| c == card) {
        return Some(i);
    }
    let text = std::fs::read_to_string(path(card)).ok()?;
    let record = Json::parse(&text)?;
    players.push((card.to_string(), record));
    Some(players.len() - 1)
}

fn save(card: &str, record: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(card));
    if let Err(e) = std::fs::write(&tmp, record.to_string()).and_then(|_| std::fs::rename(&tmp, path(card))) {
        log!("theatrhythm: {}: {e}", path(card));
    }
}

/// A new player's record.
fn new_record(card: &str) -> Json {
    let party = Json::Arr(DEFAULT_PARTY.iter().map(|&c| Json::from(c)).collect());
    Json::obj()
        .with("card", card)
        .with("playerID", player_id(card))
        .with("degressID", DEFAULT_TITLE)
        .with("titles", vec![Json::from(DEFAULT_TITLE)])
        .with("gil", 0u64)
        .with("point", 0u64)
        .with("medal", 0u64)
        .with("items", Json::obj())
        .with("crystals", Json::obj())
        .with("lastPlayPartySlot", 0)
        .with("lastPlayMusicContentID", "")
        .with("lastPlayMusicLevel", 0)
        .with(
            "setting",
            Json::obj()
                .with("triggerSpeed", 0)
                .with("triggerTiming", 0)
                .with("volumeTriggerSE", 3)
                .with("volumeBackgroundSE", 3)
                .with("volumeMusic", 0)
                .with("triggerColor", 0)
                .with("backgroundBrightness", 0)
                .with("hideChat", 0),
        )
        .with("system", Json::obj().with("hiddenContents", 0).with("selectedStatus", 0))
        .with("party", vec![party.clone(), party.clone(), party])
        .with("charas", Json::obj())
        .with("collecas", Json::obj())
        .with("songs", Json::obj())
}

/// The card of a player id (records loaded at their login).
fn card_of(players: &[(String, Json)], id: u64) -> Option<usize> {
    players.iter().position(|(_, r)| r.u("playerID") == id)
}

/// NESYS card data: every card is a registered NESiCA (its Theatrhythm data is the card
/// server's).
pub(crate) fn card_data(card: &str) -> Json {
    Json::obj()
        .with("status", 1)
        .with("reissue", false)
        .with("game_server", "")
        .with("play_count", 0)
        .with("today_play_count", 0)
        .with("today_free_play_count", 0)
        .with("cur_timestamp", 0)
        .with("last_access_timestamp", 0)
        .with("continuous_login_days", 0)
        .with("original_card_id", card)
        .with("error_number", 0)
        .with("message", "")
}

pub(crate) fn card_insert(_card: &str) -> Json {
    Json::obj().with("status", 1).with("error_number", 0).with("message", "")
}

/// The answer to `api` (`user/info`...).
pub(crate) fn answer(api: &str, req: &Json) -> Answer {
    let reply = match api {
        "service/echo" => return Answer::Text("OK"),
        "event/musicscoreranking" => return Answer::NotFound,
        // the information panel's image (none: an empty frame)
        _ if api.starts_with('/') => return Answer::NotFound,
        "service/version" => {
            let off = || Json::obj().with("active", false).with("option", 0);
            ok().with("serviceVersion", GAME_VERSION.lock().unwrap().as_str()).with(
                "campaign",
                Json::obj()
                    .with("trackPlus", off())
                    .with("firstFree", off())
                    .with("rareItemDrop", off())
                    .with("gilBoost", Json::obj().with("active", false).with("option", 1))
                    .with("dffCampaign", off()),
            )
        }
        "cabinet/boot" | "cabinet/nesicalock" | "cabinet/nesicaunlock" | "user/accountlogout" | "user/accountshoplogout" => ok(),
        // one information panel at least: the game's information screen divides by their count
        "info/banner" => ok()
            .with("titleURL", Json::Arr(vec![]))
            .with("infoURL", vec![Json::from(format!("http://127.0.0.1:{}/info/0.png", crate::theatrhythm_server::PORT))]),
        "ranking/musicscorerankingconfig" => ng("R002"),
        "user/playbonus" => ng("U011"),
        "user/musicscorerankingresult" | "user/musicscoreranking" => ng("U013"),
        "user/dominationreward" => ng("U006")
            .with("eventInfo", Json::obj())
            .with("eventResult", Json::obj().with("crystals", Json::Arr(vec![])))
            .with("medal", 0)
            .with("selectedTeam", 0)
            .with("totalPoint", 0)
            .with("nextRewardPoint", 0),
        "user/dominationnewest" => ng("U004").with("eventInfo", Json::obj()).with("home", Json::obj()).with("priorityTeam", 0),
        "user/domination" => ng("U004")
            .with("eventInfo", Json::obj())
            .with("eventResult", Json::obj().with("crystals", Json::Arr(vec![])))
            .with("selectedTeam", 0)
            .with("totalPoint", 0)
            .with("nextRewardPoint", 0),
        "user/raidbossresult" => ng("U019"),
        "user/raidboss" => ng("U018"),
        _ => {
            let mut players = PLAYERS.lock().unwrap();
            player_answer(&mut players, api, req)
        }
    };
    Answer::Json(reply)
}

fn player_answer(players: &mut Vec<(String, Json)>, api: &str, req: &Json) -> Json {
    match api {
        "user/profile" => {
            let card = req.s("nesicaID");
            let Some(i) = load(players, card) else { return ng("#004") };
            ok().with("officialID", 0).with("profile", profile(&players[i].1))
        }
        "user/accountlogin" | "user/accountshoplogin" => {
            let card = req.s("nesicaID");
            if card.is_empty() {
                return ng("U001");
            }
            let i = match load(players, card) {
                Some(i) => i,
                None => {
                    log!("theatrhythm: card {card}: new player");
                    let record = new_record(card);
                    save(card, &record);
                    players.push((card.to_string(), record));
                    players.len() - 1
                }
            };
            ok().with("playerID", players[i].1.u("playerID"))
        }
        _ => {
            let Some(i) = card_of(players, req.u("playerID")) else {
                log!("theatrhythm: {api}: unknown player {}", req.u("playerID"));
                return ng("U001");
            };
            let (card, record) = &mut players[i];
            let (reply, changed) = record_answer(record, api, req);
            if changed {
                save(card, record);
            }
            reply
        }
    }
}

fn profile(r: &Json) -> Json {
    let name = std::env::var("WAL_THEATRHYTHM_PLAYER_NAME").unwrap_or_default();
    Json::obj()
        .with("playerID", r.u("playerID"))
        .with("playerName", name.trim())
        .with("point", r.u("point"))
        .with("degressID", r.s("degressID"))
        .with("locationID", crate::theatrhythm_server::LOCATION_ID)
        .with("locationName", "")
        .with("iconID", "")
        .with("designID", "")
}

/// `items` / `crystals` of the record as the game's list (`item_0001`...: only these).
fn counts(r: &Json, key: &str, prefix: &str, digits: usize, n: usize, field: &str) -> Json {
    let kept = r.get(key);
    Json::Arr(
        (1..=n)
            .filter_map(|k| {
                let id = format!("{prefix}{k:0digits$}");
                let count = kept.map(|o| o.u(&id)).unwrap_or(0);
                (count > 0).then(|| Json::obj().with(field, id).with("count", count))
            })
            .collect(),
    )
}

/// Adds `delta` to the count `id` of `key` (`items`, `crystals`), kept within `0..=max`.
fn add_count(r: &mut Json, key: &str, id: &str, delta: i64, max: i64) {
    if id.is_empty() {
        return;
    }
    let mut kept = r.get(key).cloned().unwrap_or_else(Json::obj);
    let count = (kept.i(id) + delta).clamp(0, max);
    kept.set(id, count);
    r.set(key, kept);
}

fn add(r: &mut Json, key: &str, delta: u64, max: u64) {
    let v = r.u(key).saturating_add(delta).min(max);
    r.set(key, v);
}

/// A song's record: best score and play count of each of the 6 levels.
fn song_record(songs: Option<&Json>, id: &str) -> Json {
    let blank = || {
        Json::obj()
            .with("score", 0)
            .with("state", 0)
            .with("criticalBar", "")
            .with("lastCriticalBar", "")
            .with("style", 0)
            .with("dateTime", "")
    };
    let kept = songs.and_then(|s| s.get(id));
    let scores = (0..6).map(|l| kept.and_then(|k| k.a("highScore").get(l).cloned()).unwrap_or_else(blank)).collect::<Vec<_>>();
    let plays = (0..6).map(|l| Json::from(kept.and_then(|k| k.a("playCount").get(l)).and_then(Json::as_u64).unwrap_or(0))).collect::<Vec<_>>();
    Json::obj()
        .with("musicContentID", id)
        .with("highScore", scores)
        .with("playCount", plays)
        .with("lockedLevel", 0)
        .with("favoriteFlag", kept.and_then(|k| k.get("favoriteFlag")).and_then(Json::as_bool).unwrap_or(false))
}

/// The answer to a request about the player `r`; whether `r` changed.
fn record_answer(r: &mut Json, api: &str, req: &Json) -> (Json, bool) {
    match api {
        "user/info" => {
            let slot = r.i("lastPlayPartySlot").clamp(0, 2);
            let info = Json::obj()
                .with("gil", r.u("gil"))
                .with("point", r.u("point"))
                .with("medal", r.u("medal"))
                .with("moogleCostumeID", "")
                .with("airshipID", "")
                .with("lastPlayPartySlot", slot)
                .with("lastPlayMusicContentID", r.s("lastPlayMusicContentID"))
                .with("lastPlayMusicLevel", r.u("lastPlayMusicLevel").min(5))
                .with("itemInfo", counts(r, "items", "item_", 4, ITEMS, "itemID"))
                .with("crystalInfo", counts(r, "crystals", "crystal_", 3, CRYSTALS, "crystalPieceID"));
            let party = Json::obj().with("partySlot", slot).with("partyChara", Json::Arr(vec![]));
            let setting = r.get("setting").cloned().unwrap_or_else(Json::obj);
            let system = r.get("system").cloned().unwrap_or_else(Json::obj);
            (ok().with("info", info).with("setting", setting).with("system", system).with("party", party), false)
        }
        "user/degresslist" => (ok().with("degressID", Json::Arr(r.a("titles").to_vec())), false),
        "user/collecalist" => {
            let list = match r.get("collecas") {
                Some(Json::Obj(fields)) => fields
                    .iter()
                    .map(|(id, c)| Json::obj().with("charaID", id.as_str()).with("rarity", c.u("rarity")).with("addLevel", c.u("addLevel")))
                    .collect(),
                _ => vec![],
            };
            (ok().with("collecaList", list).with("activeRarity", 7), false)
        }
        "user/charalist" => {
            let list = match r.get("charas") {
                Some(Json::Obj(fields)) => fields
                    .iter()
                    .map(|(id, c)| {
                        Json::obj()
                            .with("charaID", id.as_str())
                            .with("charaLevel", c.u("charaLevel").max(1))
                            .with("addLevel", c.u("addLevel").min(41))
                            .with("charaExp", c.u("charaExp"))
                            .with("playingChatCommentID", Json::Arr(vec![]))
                            .with("clearedChatCommentID", Json::Arr(vec![]))
                    })
                    .collect(),
                _ => vec![],
            };
            (ok().with("charaList", list), false)
        }
        "user/party" => {
            let parties = r.a("party");
            let list = (0..3)
                .map(|slot| {
                    let kept = parties.get(slot).map(Json::items).filter(|c| c.len() == 4 && c.iter().all(|c| c.as_str().is_some_and(|s| !s.is_empty())));
                    let charas = kept.map(<[Json]>::to_vec).unwrap_or_else(|| DEFAULT_PARTY.iter().map(|&c| Json::from(c)).collect());
                    Json::obj().with("partySlot", slot).with("charaID", charas)
                })
                .collect::<Vec<_>>();
            (ok().with("party", list), false)
        }
        "user/musiclist" => {
            let mut ids: Vec<String> = DEFAULT_SONGS.iter().map(|s| s.to_string()).collect();
            if let Some(Json::Obj(songs)) = r.get("songs") {
                for (id, _) in songs {
                    if !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
            }
            let songs = r.get("songs");
            let list = ids
                .iter()
                .map(|id| {
                    let favorite = songs.and_then(|s| s.get(id)).and_then(|s| s.get("favoriteFlag")).and_then(Json::as_bool).unwrap_or(false);
                    Json::obj().with("musicContentID", id.as_str()).with("favoriteFlag", favorite)
                })
                .collect::<Vec<_>>();
            (ok().with("musicList", list), false)
        }
        "user/musicdetail" => {
            let songs = r.get("songs");
            let list = req
                .a("musicContentID")
                .iter()
                .filter_map(Json::as_str)
                .map(|id| {
                    let mut s = song_record(songs, id);
                    if let Json::Obj(fields) = &mut s {
                        fields.retain(|(k, _)| k != "favoriteFlag");
                    }
                    s
                })
                .collect::<Vec<_>>();
            (ok().with("musicDetail", list), false)
        }
        "user/playupdate" => {
            play_update(r, req);
            (ok(), true)
        }
        "user/playresult" => {
            let mut titles = r.a("titles").to_vec();
            let mut granted = vec![];
            for t in req.a("degressID").iter().filter(|t| t.as_str().is_some_and(|s| !s.is_empty())) {
                if !titles.contains(t) {
                    titles.push(t.clone());
                    granted.push(t.clone());
                }
            }
            let changed = !granted.is_empty();
            r.set("titles", titles);
            (ok().with("presentInfo", Json::obj().with("degressID", granted)), changed)
        }
        "user/playreward" => {
            play_reward(r, req);
            (ok(), true)
        }
        "user/infoupdate" => {
            if let Some(system) = req.get("system") {
                r.set("system", system.clone());
            }
            (ok(), true)
        }
        "user/shopbuy" => {
            for item in req.a("itemInfo") {
                add_count(r, "items", item.s("itemID"), item.i("count"), 999);
            }
            (ok(), true)
        }
        _ => {
            log!("theatrhythm: {api}: not answered");
            (ng("E001"), false)
        }
    }
}

/// `user/playupdate`: a credit's gains (summed), parties, settings, scores, characters' exp.
fn play_update(r: &mut Json, req: &Json) {
    if let Some(setting) = req.get("setting") {
        r.set("setting", setting.clone());
    }
    let parties = req.a("partyInfo");
    if parties.len() == 3 {
        r.set("party", parties.iter().map(|p| Json::Arr(p.a("charaID").to_vec())).collect::<Vec<_>>());
    }
    for play in req.a("playInfo") {
        add(r, "gil", play.u("gil"), 999_999_999);
        add(r, "point", play.u("point"), 99_999_999);
        add(r, "medal", play.u("medal"), 99_999);
        for item in play.a("usedItemInfo") {
            add_count(r, "items", item.s("itemID"), -item.i("count"), 999);
        }
        for item in play.a("dropItemInfo") {
            add_count(r, "items", item.s("itemID"), item.i("count"), 999);
        }
        for crystal in play.a("crystalInfo") {
            add_count(r, "crystals", crystal.s("crystalPieceID"), crystal.i("count"), 99);
        }
        let Some(music) = play.get("musicInfo") else { continue };
        let id = music.s("musicContentID");
        if id.is_empty() {
            continue;
        }
        let level = (music.u("musicLevel") as usize).min(5);
        let mut songs = r.get("songs").cloned().unwrap_or_else(Json::obj);
        let mut song = song_record(Some(&songs), id);
        // a failed track (the party fell) is no play: a play count with state 0 is a clear
        if let Some(Json::Arr(plays)) = song.get_mut("playCount").filter(|_| play.get("clearFlag").and_then(Json::as_bool) == Some(true)) {
            let n = plays[level].as_u64().unwrap_or(0) + 1;
            plays[level] = Json::from(n);
        }
        if let (Some(score), Some(Json::Arr(best))) = (music.get("score"), song.get_mut("highScore")) {
            let old = best[level].clone();
            let state = old.u("state").max(score.u("state"));
            let mut new = if score.u("score") >= old.u("score") { score.clone() } else { old };
            new.set("state", state);
            if !score.s("criticalBar").is_empty() {
                new.set("criticalBar", score.s("criticalBar"));
            }
            best[level] = new;
        }
        songs.set(id, song);
        r.set("songs", songs);
        r.set("lastPlayMusicContentID", id);
        r.set("lastPlayMusicLevel", level as u64);
        r.set("lastPlayPartySlot", music.i("partySlot").clamp(0, 2));
    }
    let mut charas = r.get("charas").cloned().unwrap_or_else(Json::obj);
    for c in req.a("resultCharaInfo") {
        let id = c.s("charaID");
        if id.is_empty() {
            continue;
        }
        let mut kept = charas.get(id).cloned().unwrap_or_else(|| Json::obj().with("addLevel", 0));
        kept.set("charaLevel", c.u("charaLevel"));
        kept.set("charaExp", c.u("charaExp"));
        charas.set(id, kept);
    }
    r.set("charas", charas);
}

/// `user/playreward`: what the player unlocked at the result (after `playupdate`).
fn play_reward(r: &mut Json, req: &Json) {
    let mut charas = r.get("charas").cloned().unwrap_or_else(Json::obj);
    for id in req.a("charaID").iter().filter_map(Json::as_str).filter(|s| !s.is_empty()) {
        if charas.get(id).is_none() {
            charas.set(id, Json::obj().with("charaLevel", 1).with("charaExp", 0).with("addLevel", 0));
        }
    }
    let mut collecas = r.get("collecas").cloned().unwrap_or_else(Json::obj);
    for level in req.a("addLevelInfo") {
        let id = level.s("charaID");
        if id.is_empty() {
            continue;
        }
        let add_level = level.u("addLevel").min(41);
        let mut c = charas.get(id).cloned().unwrap_or_else(|| Json::obj().with("charaLevel", 1).with("charaExp", 0));
        c.set("addLevel", add_level);
        charas.set(id, c);
        let mut card = collecas.get(id).cloned().unwrap_or_else(|| Json::obj().with("rarity", 0));
        card.set("addLevel", add_level);
        collecas.set(id, card);
    }
    if !req.a("collecaID").is_empty() {
        log!("theatrhythm: collection cards {} (their rarity not kept)", Json::Arr(req.a("collecaID").to_vec()));
    }
    r.set("charas", charas);
    r.set("collecas", collecas);
    let mut songs = r.get("songs").cloned().unwrap_or_else(Json::obj);
    for id in req.a("musicContentID").iter().filter_map(Json::as_str).filter(|s| !s.is_empty()) {
        if songs.get(id).is_none() {
            songs.set(id, song_record(None, id));
        }
    }
    r.set("songs", songs);
    if req.a("crystalInfo").len() == CRYSTALS {
        let mut crystals = Json::obj();
        for c in req.a("crystalInfo") {
            crystals.set(c.s("crystalPieceID"), c.u("count").min(99));
        }
        r.set("crystals", crystals);
    }
    if req.get("medal").is_some() {
        r.set("medal", req.u("medal").min(99_999));
    }
    let title = req.s("replaceDegressID");
    if !title.is_empty() {
        r.set("degressID", title);
    }
}
