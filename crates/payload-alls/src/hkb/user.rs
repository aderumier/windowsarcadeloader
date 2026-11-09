//! The players: one JSON document per user id in `Y:\hkb\<id>.json` (the user id is the card's
//! Aime id). A new player (`regist_user`) starts as the game's guest (`guest_user.csv`: the
//! Chronos's look, the eight characters, their card sets, the cards), the tutorial still to play;
//! the five parties each hold the eight card sets. The server's lock covers a whole request.

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

use super::master;
use super::server::Ctx;

/// Held for each request.
pub(super) static LOCK: Mutex<()> = Mutex::new(());

const DIR: &str = "Y:\\hkb";

/// The parties a player has (the client shows a tab for each).
const PARTIES: i64 = 5;

/// The CP a finished tutorial gives ("初回プレイ特典", 100 CP).
const TUTORIAL_CP: i64 = 100;

fn path(user: i64) -> String {
    format!("{DIR}\\{user}.json")
}

fn load(user: i64) -> Option<Json> {
    std::fs::read_to_string(path(user)).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_)))
}

fn save(user: i64, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(user));
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path(user));
    }
}

/// `doc`'s array `key` (created empty).
fn arr<'a>(doc: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    if !matches!(doc.get(key), Some(Json::Arr(_))) {
        doc.set(key, Vec::<Json>::new());
    }
    match doc.get_mut(key) {
        Some(Json::Arr(items)) => items,
        _ => unreachable!("hkb: document is an object"),
    }
}

/// `doc`'s object `key` (created empty).
fn obj<'a>(doc: &'a mut Json, key: &str) -> &'a mut Json {
    if !matches!(doc.get(key), Some(Json::Obj(_))) {
        doc.set(key, Json::obj());
    }
    doc.get_mut(key).expect("hkb: document is an object")
}

fn add(doc: &mut Json, key: &str, n: i64) {
    let v = doc.i(key) + n;
    doc.set(key, v);
}

/// A pseudo-random number (the rewards' cards).
fn random() -> u64 {
    static STATE: Mutex<u64> = Mutex::new(0);
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if *s == 0 {
        *s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9e37_79b9, |d| d.as_nanos() as u64) | 1;
    }
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn today() -> i64 {
    (crate::clock::unix_now() / 86400) as i64
}

/// A new player: the guest's profile (`guest_user.csv`, format 2: the format, the user id, the
/// name, the party; the Chronos's costume, skin, chair, flag, light station, ornament; the stage
/// (8); the character count and each `id,costume,skin,beast god costume`; the party's four card
/// set ids; each card set's name, id, character, cards and their options; then `card,option`).
fn new_player(aime_id: i64, access_code: &str, name: &str, version: i64) -> Json {
    let text = master::read("GuestUser/GuestUserData/guest_user.csv").unwrap_or_default();
    let mut q = text.lines().map(str::trim).collect::<std::collections::VecDeque<&str>>();
    let mut next = || q.pop_front().unwrap_or("");
    let num = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
    let nums = |s: &str| s.split(',').map(|v| v.trim().parse::<i64>().unwrap_or(0)).collect::<Vec<i64>>();
    let (_format, _user, _name, _party) = (next(), next(), next(), next());
    let look: Vec<i64> = (0..6).map(|_| num(next())).collect();
    for _ in 0..8 {
        next();
    }
    let characters: Vec<Json> = (0..num(next()))
        .map(|_| {
            let c = nums(next());
            let at = |i: usize| c.get(i).copied().unwrap_or(0);
            Json::obj().with("characterId", at(0)).with("costumeId", at(1)).with("skinId", at(2)).with("beastGodCostumeId", at(3))
        })
        .collect();
    let members = nums(next());
    let card_sets: Vec<Json> = (0..characters.len())
        .map(|_| {
            let (name, id, character) = (next().to_string(), num(next()), num(next()));
            let (cards, options) = (nums(next()), nums(next()));
            let detail: Vec<Json> =
                cards.iter().enumerate().map(|(i, c)| Json::obj().with("cardId", *c).with("optionId", options.get(i).copied().unwrap_or(0))).collect();
            Json::obj().with("cardSetId", id).with("cardSetName", name).with("ngWordFlag", 0u32).with("characterId", character).with("cardSetDetail", detail)
        })
        .collect();
    let mut cards: Vec<Json> = Vec::new();
    for line in q {
        let c = nums(line);
        if c.first().copied().unwrap_or(0) > 0 {
            cards.push(Json::obj().with("cardId", c[0]).with("optionId", c.get(1).copied().unwrap_or(0)).with("cardNum", 1u32).with("newFlag", 0u32));
        }
    }
    let parties: Vec<Json> = (1..=PARTIES)
        .map(|id| {
            let detail: Vec<Json> = members.iter().map(|m| Json::obj().with("cardSetId", *m)).collect();
            Json::obj()
                .with("partyId", id)
                .with("partyName", format!("PARTY {id}"))
                .with("ngWordFlag", 0u32)
                .with("partyDetail", detail)
                .with("cardSetInfo", card_sets.clone())
        })
        .collect();
    let at = |i: usize| look.get(i).copied().unwrap_or(0);
    let chronos = Json::obj()
        .with("costumeId", at(0))
        .with("skinId", at(1))
        .with("chairId", at(2))
        .with("flagId", at(3))
        .with("lightStationLeftId", at(4))
        .with("lightStationRightId", at(4))
        .with("ornamentLeftId", at(5))
        .with("ornamentRightId", at(5));
    // the navigators (`NaviCharaList.json`), each in its default costume
    let navi_master = master::read("Params/game/NaviCharaList.json").and_then(|t| Json::parse(&t)).unwrap_or(Json::obj());
    let mut navi: Vec<Json> =
        navi_master.a("NaviCharaList").iter().map(|n| Json::obj().with("naviCharaId", n.i("id")).with("costumeId", n.i("defalutCostumeID"))).collect();
    if navi.is_empty() {
        navi.push(Json::obj().with("naviCharaId", 1u32).with("costumeId", 100u32));
    }
    Json::obj()
        .with("aimeId", aime_id)
        .with("accessCode", access_code)
        .with("userName", name)
        .with("ngWordFlag", 0u32)
        .with("chronosRank", 1u32)
        .with("rankPoint", 0u32)
        .with("totalScore", 0u32)
        .with("changeDrawTypeFlag", 1u32)
        .with("drawType", 0u32)
        .with("paidCp", 0u32)
        .with("freeCp", 0u32)
        .with("codenameId", 1001001u32)
        .with("partyId", 1u32)
        .with("naviCharaId", navi[0].i("naviCharaId"))
        .with("earphoneVolume", 10u32)
        .with("completeTutorialFlag", 0u32)
        .with("lastVersion", version)
        .with("chronos", chronos)
        .with("characters", characters)
        .with("parties", parties)
        .with("cards", cards)
        .with("navi", navi)
}

/// The user whose card this is (its Aime id), 0 for none.
fn aime_to_user(ctx: &Ctx) -> Json {
    let id = ctx.req.i("aimeId");
    let user = if id > 0 && id <= i32::MAX as i64 && load(id).is_some() { id } else { 0 };
    Json::obj().with("userId", user)
}

fn regist_user(ctx: &Ctx) -> Json {
    let id = ctx.req.i("aimeId");
    if id <= 0 || id > i32::MAX as i64 {
        log!("hkb: aime id {id} is no user id");
        return Json::obj().with("userId", 0u32);
    }
    let doc = new_player(id, ctx.req.s("accessCode"), ctx.req.s("userName"), ctx.req.i("clientVersion"));
    save(id, &doc);
    log!("hkb: new player {id} {}", ctx.req.s("userName"));
    Json::obj().with("userId", id)
}

/// A battle result record: matches, wins, losses, draws, streaks, high score (and stars).
fn count_result(rec: &mut Json, result: i64, score: i64, stars: Option<i64>) {
    add(rec, "matchNum", 1);
    match result {
        1 => {
            add(rec, "winNum", 1);
            add(rec, "winningStreakNum", 1);
            rec.set("losingStreakNum", 0u32);
            if rec.i("winningStreakNum") > rec.i("maxWinningStreakNum") {
                let v = rec.i("winningStreakNum");
                rec.set("maxWinningStreakNum", v);
            }
        }
        2 => {
            add(rec, "loseNum", 1);
            add(rec, "losingStreakNum", 1);
            rec.set("winningStreakNum", 0u32);
        }
        _ => add(rec, "drawNum", 1),
    }
    if score > rec.i("highScore") {
        rec.set("highScore", score);
    }
    if let Some(stars) = stars
        && stars > rec.i("starNum")
    {
        rec.set("starNum", stars);
    }
}

const RESULT_FIELDS: [&str; 8] = ["matchNum", "winNum", "winningStreakNum", "maxWinningStreakNum", "loseNum", "losingStreakNum", "drawNum", "highScore"];

fn result_record(doc: &Json, mode: &str, extra: &[&str]) -> Json {
    let rec = doc.get("results").and_then(|r| r.get(mode)).cloned().unwrap_or(Json::obj());
    let mut out = Json::obj();
    for k in RESULT_FIELDS.iter().chain(extra) {
        out.set(k, rec.i(k));
    }
    out
}

/// A battle's common results: the rank, the score, the latest results.
fn save_battle(doc: &mut Json, req: &Json) {
    if req.i("chronosRank") > 0 {
        doc.set("chronosRank", req.i("chronosRank"));
    }
    if req.get("rankPoint").is_some() {
        doc.set("rankPoint", req.i("rankPoint"));
    }
    add(doc, "totalScore", req.i("score").max(0));
    count_result(obj(doc, "latest"), req.i("battleResult"), 0, None);
}

/// A story or explore episode's record.
fn save_episode(doc: &mut Json, key: &str, req: &Json) {
    let list = arr(doc, key);
    let (chapter, episode) = (req.i("chapterId"), req.i("episodeId"));
    if !list.iter().any(|e| e.i("chapterId") == chapter && e.i("episodeId") == episode) {
        list.push(Json::obj().with("chapterId", chapter).with("episodeId", episode));
    }
    let rec = list.iter_mut().find(|e| e.i("chapterId") == chapter && e.i("episodeId") == episode).expect("hkb: episode just added");
    count_result(rec, req.i("battleResult"), req.i("score"), Some(req.i("starNum")));
}

fn episodes(doc: &Json, key: &str) -> Vec<Json> {
    doc.a(key)
        .iter()
        .map(|e| {
            let mut out = Json::obj();
            for k in ["chapterId", "episodeId", "matchNum", "winNum", "loseNum", "drawNum", "highScore", "starNum"] {
                out.set(k, e.i(k));
            }
            out
        })
        .collect()
}

/// Adds `n` of a card (a new card flagged new).
fn add_card(doc: &mut Json, card: i64, option: i64, n: i64) {
    let cards = arr(doc, "cards");
    match cards.iter_mut().find(|c| c.i("cardId") == card && c.i("optionId") == option) {
        Some(c) => add(c, "cardNum", n),
        None => cards.push(Json::obj().with("cardId", card).with("optionId", option).with("cardNum", n).with("newFlag", 1u32)),
    }
}

/// Adds (or takes, `n` negative) a material: its new count.
fn add_material(doc: &mut Json, material: i64, n: i64) -> i64 {
    let list = arr(doc, "materials");
    if !list.iter().any(|m| m.i("materialId") == material) {
        list.push(Json::obj().with("materialId", material).with("materialNum", 0u32));
    }
    let m = list.iter_mut().find(|m| m.i("materialId") == material).expect("hkb: material just added");
    let v = (m.i("materialNum") + n).max(0);
    m.set("materialNum", v);
    v
}

/// Adds a ticket: its new count.
fn add_ticket(doc: &mut Json, ticket: i64, n: i64) -> i64 {
    let list = arr(doc, "tickets");
    if !list.iter().any(|t| t.i("ticketId") == ticket) {
        list.push(Json::obj().with("ticketId", ticket).with("ticketNum", 0u32));
    }
    let t = list.iter_mut().find(|t| t.i("ticketId") == ticket).expect("hkb: ticket just added");
    let v = (t.i("ticketNum") + n).max(0);
    t.set("ticketNum", v);
    v
}

fn no_ticket() -> Json {
    Json::obj().with("ticketId", 0u32).with("ticketNum", 0u32)
}

/// The summon assist tickets the player has (the answers' `summonTicketInfo`: the client sets its
/// count).
fn summon_tickets(doc: &mut Json) -> Json {
    let n = add_ticket(doc, master::SUMMON_TICKET, 0);
    Json::obj().with("ticketId", master::SUMMON_TICKET).with("ticketNum", n)
}

/// A card summon's answer: its cards (and the product's purchase counts).
fn summoned(doc: &mut Json, product: i64, n: i64) -> Json {
    let cards = publish_cards(doc, n);
    let counts = obj(doc, "purchases");
    let key = product.to_string();
    let total = counts.i(&key) + 1;
    counts.set(&key, total);
    Json::obj()
        .with("purchaseDayNum", 0u32)
        .with("purchaseTotalNum", total)
        .with("assuranceRarityInfo", Vec::<Json>::new())
        .with("cardInfo", cards)
        .with("summonTicketInfo", summon_tickets(doc))
}

/// The copies of a card (an enchantment of it) a player keeps; more make summon assist tickets
/// (the summon shop's rule).
const CARD_LIMIT: i64 = 9;

/// `n` random cards, given: their `PublishCardInfo` (`ticketInfo` the ticket a card beyond the
/// limit made: the client then does not count the card).
fn publish_cards(doc: &mut Json, n: i64) -> Vec<Json> {
    let pool = master::cards();
    if pool.is_empty() {
        return Vec::new();
    }
    (0..n.clamp(0, 30))
        .map(|_| {
            let card = pool[(random() % pool.len() as u64) as usize];
            let have = doc.a("cards").iter().find(|c| c.i("cardId") == card && c.i("optionId") == 0).map_or(0, |c| c.i("cardNum"));
            let ticket = if have >= CARD_LIMIT {
                add_ticket(doc, master::SUMMON_TICKET, 1);
                Json::obj().with("ticketId", master::SUMMON_TICKET).with("ticketNum", 1u32)
            } else {
                add_card(doc, card, 0, 1);
                no_ticket()
            };
            Json::obj().with("cardId", card).with("optionId", 0u32).with("cardNum", 1u32).with("ticketInfo", ticket)
        })
        .collect()
}

/// The materials an explore win gives: the time sands and dimension fragments the generation
/// shop and the customize shop spend, more with stars and the episode's multiplier.
fn explore_materials(doc: &mut Json, req: &Json) -> Vec<Json> {
    if req.i("battleResult") != 1 {
        return Vec::new();
    }
    let n = 5 * (1 + req.i("starNum").max(0)) * req.i("materialMultiply").max(1);
    [401, 402, 403, 404]
        .iter()
        .map(|&m| {
            let total = add_material(doc, m, n);
            Json::obj().with("materialId", m).with("addMaterialNum", n).with("materialNum", total)
        })
        .collect()
}

fn consume_material(doc: &mut Json, material: i64, n: i64) -> Json {
    let total = add_material(doc, material, -n);
    Json::obj().with("materialId", material).with("consumeMaterialNum", n).with("materialNum", total)
}

fn cp(doc: &Json) -> Json {
    Json::obj().with("paidCp", doc.i("paidCp")).with("freeCp", doc.i("freeCp"))
}

/// The answers about a player (`userId` in the request).
fn player(command: &str, ctx: &Ctx, doc: &mut Json) -> Option<Json> {
    let req = ctx.req;
    Some(match command {
        "simple_login" => Json::obj()
            .with("loginStatus", 0u32)
            .with("message", "")
            .with("userName", doc.s("userName"))
            .with("ngWordFlag", doc.i("ngWordFlag"))
            .with("chronosRank", doc.i("chronosRank"))
            .with("rankPoint", doc.i("rankPoint"))
            .with("codenameId", doc.i("codenameId"))
            .with("completeTutorialFlag", doc.i("completeTutorialFlag"))
            // this client's version: no "newer version" dialog
            .with("terminalLastPlayVersion", 0u32)
            .with("satelliteLastPlayVersion", ctx.version),
        "login_to_satellite" | "login_to_terminal" => {
            let first = doc.i("lastLoginDay") != today();
            doc.set("lastLoginDay", today());
            doc.set("lastVersion", ctx.version);
            Json::obj()
                .with("loginStatus", 0u32)
                .with("message", "")
                .with("todayFirstLoginFlag", i64::from(first))
                .with("campaignLoginFlag", 0u32)
                .with("campaignAddCp", 0u32)
        }
        "logout_from_satellite" => {
            if req.get("earphoneVolume").is_some() {
                doc.set("earphoneVolume", req.i("earphoneVolume"));
            }
            // the play's CP left made summon assist tickets (the client's CP / CP_Conversion_Rate)
            if req.i("convertTicketFlag") != 0 {
                doc.set("paidCp", 0u32);
                doc.set("freeCp", 0u32);
                add_ticket(doc, master::SUMMON_TICKET, req.i("convertTicketNum").max(0));
            }
            Json::obj()
        }
        "simple_logout" | "logout_from_terminal" | "user_keep_alive" => Json::obj(),
        "get_user_info" => {
            let mut out = Json::obj();
            out.set("userName", doc.s("userName"));
            for k in [
                "ngWordFlag", "chronosRank", "rankPoint", "rankNational", "totalScore", "changeDrawTypeFlag", "drawType", "paidCp", "freeCp", "codenameId",
                "partyId", "naviCharaId", "earphoneVolume", "completeTutorialFlag",
            ] {
                out.set(k, doc.i(k));
            }
            out.with("dotNetMemberFlag", 0u32).with("developmentUserFlag", 0u32).with("testUserFlag", 0u32)
        }
        "get_user_chronos" => doc.get("chronos").cloned().unwrap_or(Json::obj()),
        "get_user_character" => Json::obj().with("characterInfo", doc.a("characters").to_vec()),
        "get_user_party" => {
            let parties = doc
                .a("parties")
                .iter()
                .map(|p| {
                    Json::obj()
                        .with("partyId", p.i("partyId"))
                        .with("partyName", p.s("partyName"))
                        .with("ngWordFlag", p.i("ngWordFlag"))
                        .with("partyDetail", p.a("partyDetail").to_vec())
                })
                .collect::<Vec<_>>();
            Json::obj().with("partyInfo", parties)
        }
        "get_user_card_set" => {
            let parties = doc
                .a("parties")
                .iter()
                .map(|p| Json::obj().with("partyId", p.i("partyId")).with("cardSetInfo", p.a("cardSetInfo").to_vec()))
                .collect::<Vec<_>>();
            Json::obj().with("partyInfo", parties)
        }
        "get_user_card" => {
            let cards = doc.a("cards").to_vec();
            // seen once sent
            for c in arr(doc, "cards") {
                c.set("newFlag", 0u32);
            }
            Json::obj().with("cardInfo", cards)
        }
        "get_user_material" => Json::obj().with("materialInfo", doc.a("materials").to_vec()),
        "get_user_ticket" => {
            let tickets = doc
                .a("tickets")
                .iter()
                .filter(|t| t.i("ticketNum") > 0)
                .map(|t| t.clone().with("ticketDetail", Vec::<Json>::new()))
                .collect::<Vec<_>>();
            Json::obj().with("ticketInfo", tickets)
        }
        "get_user_chrorega_coin" => Json::obj().with("chroregaCoinNum", doc.i("chroregaCoin")).with("chroregaCoinDetail", Vec::<Json>::new()),
        "get_user_purchase_shop_card" => Json::obj().with("productInfo", Vec::<Json>::new()),
        "get_user_purchase_shop_customize" => {
            let owned = doc.a("customize").iter().map(|p| Json::obj().with("productId", p.as_i64().unwrap_or(0))).collect::<Vec<_>>();
            Json::obj().with("productInfo", owned)
        }
        "get_user_stamp" => Json::obj().with("stampInfo", Vec::<Json>::new()),
        "get_user_navi_chara" => Json::obj().with("naviCharaInfo", doc.a("navi").to_vec()),
        "get_user_notification" => Json::obj().with("notificationInfo", Vec::<Json>::new()),
        "reply_to_notification" | "reply_to_release_draw_type_notification" => Json::obj(),
        "get_user_mission_normal" => Json::obj().with("achievementInfo", Vec::<Json>::new()).with("receiptInfo", Vec::<Json>::new()),
        "get_user_mission_daily" => Json::obj().with("missionInfo", Vec::<Json>::new()),
        "get_user_mission_event" => Json::obj().with("eventId", 0u32).with("missionInfo", Vec::<Json>::new()),
        "get_user_mission_reward" | "get_user_expire_mission_reward" => Json::obj().with("missionRewardInfo", Vec::<Json>::new()),
        "save_user_battle_mission_achievement" => Json::obj(),
        "push_stamp" => Json::obj().with("stampInfo", Vec::<Json>::new()),
        "get_user_battle_result_national" => result_record(doc, "national", &[]),
        "get_user_battle_result_place" => result_record(doc, "place", &[]),
        "get_user_battle_result_friend" => result_record(doc, "friend", &[]),
        "get_user_battle_result_com" => result_record(doc, "com", &[]),
        "get_user_battle_result_place_event" => result_record(doc, "placeEvent", &[]),
        "get_user_battle_result_event" => result_record(doc, &format!("event{}", req.i("eventId")), &["starNum"]),
        "get_user_battle_latest_result" => {
            let l = doc.get("latest").cloned().unwrap_or(Json::obj());
            Json::obj().with("matchNum", l.i("matchNum")).with("winNum", l.i("winNum")).with("loseNum", l.i("loseNum")).with("drawNum", l.i("drawNum"))
        }
        "get_user_battle_result_story" => Json::obj().with("storyInfo", episodes(doc, "story")),
        "get_user_battle_result_explore" => Json::obj().with("exploreInfo", episodes(doc, "explore")),
        "get_user_battle_period_total_score" => Json::obj().with("totalScore", doc.get("periodScore").and_then(|p| p.get(&req.i("eventId").to_string())).and_then(Json::as_i64).unwrap_or(0)),
        "save_user_battle_period_total_score" => {
            let key = req.i("eventId").to_string();
            let scores = obj(doc, "periodScore");
            let v = scores.i(&key) + req.i("score").max(0);
            scores.set(&key, v);
            Json::obj().with("totalScoreRewardInfo", Vec::<Json>::new())
        }
        "save_user_battle_result_national" | "save_user_battle_result_place" | "save_user_battle_result_friend" | "save_user_battle_result_com"
        | "save_user_battle_result_place_event" | "save_user_battle_result_event" => {
            let mode = match command {
                "save_user_battle_result_national" => "national".to_string(),
                "save_user_battle_result_place" => "place".to_string(),
                "save_user_battle_result_friend" => "friend".to_string(),
                "save_user_battle_result_com" => "com".to_string(),
                "save_user_battle_result_place_event" => "placeEvent".to_string(),
                _ => format!("event{}", req.i("eventId")),
            };
            save_battle(doc, req);
            let stars = (command == "save_user_battle_result_event").then(|| req.i("starNum"));
            count_result(obj(obj(doc, "results"), &mode), req.i("battleResult"), req.i("score"), stars);
            Json::obj()
        }
        "save_user_battle_result_story" => {
            save_battle(doc, req);
            save_episode(doc, "story", req);
            Json::obj()
        }
        "save_user_battle_result_explore" => {
            save_battle(doc, req);
            save_episode(doc, "explore", req);
            Json::obj().with("materialInfo", explore_materials(doc, req))
        }
        "save_user_battle_result_tutorial" => {
            for s in req.a("storyInfo") {
                let ep = Json::obj().with("chapterId", s.i("chapterId")).with("episodeId", s.i("episodeId")).with("battleResult", 1u32);
                save_episode(doc, "story", &ep);
            }
            Json::obj()
        }
        "complete_tutorial" => {
            if doc.i("completeTutorialFlag") == 0 {
                doc.set("completeTutorialFlag", 1u32);
                add(doc, "freeCp", TUTORIAL_CP);
            }
            cp(doc)
        }
        "get_battle_reward" => {
            let cards = publish_cards(doc, req.i("publishNum"));
            Json::obj()
                .with("cardInfo", cards)
                .with("summonTicketInfo", summon_tickets(doc))
                .with("materialInfo", Json::obj().with("materialId", 0u32).with("addMaterialNum", 0u32).with("materialNum", 0u32))
        }
        "consume_cp" => {
            // the free CP first
            let mut n = req.i("consumeCp").max(0);
            let free = doc.i("freeCp").min(n);
            add(doc, "freeCp", -free);
            n -= free;
            let paid = doc.i("paidCp").min(n);
            add(doc, "paidCp", -paid);
            cp(doc)
        }
        "recovery_cp_begin" => {
            let id = random() as i64 & 0x7fff_ffff;
            doc.set("assurance", Json::obj().with("id", id).with("cp", req.i("recoveryCp").max(0)));
            Json::obj().with("assuranceId", id)
        }
        "recovery_cp_commit" => {
            if let Some(a) = doc.get("assurance").cloned()
                && a.i("id") == req.i("assuranceId")
            {
                add(doc, "paidCp", a.i("cp"));
                doc.set("assurance", Json::Null);
            }
            let tickets = summon_tickets(doc);
            cp(doc).with("ticketInfo", no_ticket()).with("summonTicketInfo", tickets)
        }
        "recovery_cp_retry" | "purchase_shop_card_retry" | "purchase_shop_chrorega_coin_retry" => Json::obj().with("result", 0u32),
        "purchase_shop_card_begin" => {
            let id = random() as i64 & 0x7fff_ffff;
            doc.set("assurance", Json::obj().with("id", id).with("product", req.i("productId")).with("publish", req.i("publishNum")));
            Json::obj().with("assuranceId", id)
        }
        "purchase_shop_card_commit" => match doc.get("assurance").cloned().filter(|a| a.i("id") == req.i("assuranceId")) {
            Some(a) => {
                doc.set("assurance", Json::Null);
                summoned(doc, a.i("product"), a.i("publish"))
            }
            None => summoned(doc, 0, 0),
        },
        "purchase_shop_card_for_free" => summoned(doc, req.i("productId"), req.i("publishNum")),
        "purchase_shop_card_for_ticket" => {
            let t = req.get("ticketInfo").cloned().unwrap_or(Json::obj());
            let have = add_ticket(doc, t.i("ticketId"), 0);
            if have < t.i("ticketNum") {
                log!("hkb: {} tickets {} wanted, {have} kept", t.i("ticketId"), t.i("ticketNum"));
                return Some(summoned(doc, req.i("productId"), 0));
            }
            let left = add_ticket(doc, t.i("ticketId"), -t.i("ticketNum").max(0));
            summoned(doc, req.i("productId"), req.i("publishNum")).with("consumeTicketInfo", Json::obj().with("ticketId", t.i("ticketId")).with("ticketNum", left))
        }
        "save_user_active_party" => {
            doc.set("partyId", req.i("partyId"));
            Json::obj()
        }
        "save_user_codename" => {
            doc.set("codenameId", req.i("codenameId"));
            Json::obj()
        }
        "save_user_draw_type" => {
            doc.set("drawType", req.i("drawType"));
            doc.set("changeDrawTypeFlag", 1u32);
            Json::obj()
        }
        "save_user_active_navi_chara" => {
            doc.set("naviCharaId", req.i("naviCharaId"));
            Json::obj()
        }
        "save_user_party" => {
            if let Some(p) = arr(doc, "parties").iter_mut().find(|p| p.i("partyId") == req.i("partyId")) {
                p.set("partyDetail", req.a("partyDetail").to_vec());
            }
            Json::obj()
        }
        "save_user_card_set" => {
            let set = Json::obj()
                .with("cardSetId", req.i("cardSetId"))
                .with("characterId", req.i("characterId"))
                .with("cardSetDetail", req.a("cardSetDetail").to_vec());
            if let Some(p) = arr(doc, "parties").iter_mut().find(|p| p.i("partyId") == req.i("partyId")) {
                let sets = arr(p, "cardSetInfo");
                match sets.iter_mut().find(|s| s.i("cardSetId") == req.i("cardSetId")) {
                    Some(s) => {
                        s.set("characterId", req.i("characterId"));
                        s.set("cardSetDetail", req.a("cardSetDetail").to_vec());
                    }
                    None => sets.push(set.with("cardSetName", "").with("ngWordFlag", 0u32)),
                }
            }
            Json::obj()
        }
        "generate_card" => {
            let (card, option) = (req.i("cardId"), req.i("optionId"));
            let m = req.get("materialInfo").cloned().unwrap_or(Json::obj());
            let (material, n) = master::generate_cost(card, option).unwrap_or((m.i("materialId"), m.i("materialNum")));
            let consumed = consume_material(doc, material, n);
            add_card(doc, card, option, 1);
            Json::obj()
                .with("cardInfo", Json::obj().with("cardId", card).with("optionId", option).with("cardNum", 1u32).with("ticketInfo", no_ticket()))
                .with("consumeMaterialInfo", consumed)
        }
        "purchase_shop_customize" | "equip_chronos_item" | "equip_character_item" => {
            let product = req.i("productId");
            let m = req.get("materialInfo").cloned().unwrap_or(Json::obj());
            let cost = master::customize_cost(product).map(|(buy, equip)| if command == "purchase_shop_customize" { buy } else { equip });
            let (material, n) = cost.unwrap_or((m.i("materialId"), m.i("materialNum")));
            let consumed = consume_material(doc, material, n);
            if command == "purchase_shop_customize" {
                let owned = arr(doc, "customize");
                if !owned.iter().any(|p| p.as_i64() == Some(product)) {
                    owned.push(Json::from(product));
                }
            }
            Json::obj().with("consumeMaterialInfo", consumed)
        }
        "decompose_card" => {
            // each card taken, a summon assist ticket (101) given for it
            let mut n = 0;
            for c in req.a("cardInfo") {
                let k = c.i("cardNum").max(1);
                add_card(doc, c.i("cardId"), c.i("optionId"), -k);
                n += k;
            }
            arr(doc, "cards").retain(|c| c.i("cardNum") > 0);
            let total = add_ticket(doc, master::SUMMON_TICKET, n);
            let tickets = Json::obj().with("ticketId", master::SUMMON_TICKET).with("ticketNum", total);
            Json::obj().with("decomposeTicketInfo", tickets.clone()).with("summonTicketInfo", tickets)
        }
        _ => return None,
    })
}

/// The answer to `command` if it is about a player (`None`: not one of these).
pub(super) fn reply(command: &str, ctx: &Ctx) -> Option<Json> {
    match command {
        "aime_to_user" => return Some(aime_to_user(ctx)),
        "regist_user" => return Some(regist_user(ctx)),
        _ => {}
    }
    let user = ctx.req.i("userId");
    let mut doc = load(user)?;
    let answer = player(command, ctx, &mut doc)?;
    save(user, &doc);
    Some(answer)
}
