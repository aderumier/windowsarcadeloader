//! Love Live! School Idol Festival ~after school ACTIVITY~ Next Stage's title server: the game
//! logic of each `/game` request (`lovelive.rs` decrypts the body and encrypts the reply), a port
//! of LLServer (BocuD, MIT): its `GameController` and handlers, the same answers.
//!
//! A request is `{"protocol", "param", "sessionkey", "terminal"}`, its reply
//! `{"result": 200, "response": {...}}` (400 and an empty response: a request whose param or
//! session is missing or does not read as the game's types). `auth` logs a NESiCA card in (a
//! session key back, sent with every later request; a card never seen gets a new player whose
//! `status` 1 makes the game ask for a name and group, `userdata.initialize`), or a guest
//! (`guestflag` 1: LLServer's guest player, kept in memory, never saved). A second `auth` of a
//! card whose session is still active (between `gameentry` and `gameexit`, one hour at most)
//! answers `abnormal_end` 1.
//!
//! The player's whole record is one JSON document, `<D: data>\lovelive\<card id>.json`: the
//! three groups' user data, members, cards, songs and charts, travels, game histories,
//! achievements, items, unlockables, mailbox and the tutorial / popup flags string, each list in
//! the shape the game reads (the game histories carry their group in `_idol_kind`). The game
//! reports what changed at each step: `userdata.set` (options, equipped cards, the
//! nameplate / badge / honor chosen), `gameresult` (scores, histories, profile card ids),
//! `achievement` / `achievementyell`, `TravelResult` (the board game: pamphlets, items, cards to
//! the mailbox, taken with `present`), `gameexit` (what was seen, the flags).
//!
//! What LLServer answers with fixed data, kept: dummy rankings, a fixed member gacha box, three
//! gacha announcements, no events, news or travel gachas (its databases empty), no travel
//! histories (it never stores them), no other players on the travel map.
//! `WAL_LOVELIVE_SERVER_LOG=1`: every request's protocol and reply size in the log.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use wal_payload_common::json::Json;
use wal_payload_common::log;

/// A reply's response, or `Err`: answered 400.
type Reply = Result<Json, ()>;

struct Session {
    key: String,
    /// The card logged in (`None`: a guest).
    card: Option<String>,
    /// Between `gameentry` and `gameexit`, until `expire` (Unix seconds).
    active: bool,
    expire: u64,
}

struct Server {
    sessions: Vec<Session>,
    /// The guest player (LLServer's `User.GuestUser`: changed in memory, never saved).
    guest: Option<Json>,
}

/// One request at a time (sessions, players' files).
static SERVER: Mutex<Server> = Mutex::new(Server { sessions: Vec::new(), guest: None });

struct Player {
    card: Option<String>,
    state: Json,
    /// Changed: saved after the request (a card's).
    dirty: bool,
}

fn verbose() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("WAL_LOVELIVE_SERVER_LOG").is_ok_and(|v| v.trim() == "1"))
}

/// The host of the news pictures' base URL (`information`: `http://<host>/info/`), the HTTP
/// request's `Host` in LLServer: `WAL_LOVELIVE_HOST`, default 127.0.0.1.
fn host() -> String {
    std::env::var("WAL_LOVELIVE_HOST").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_else(|| "127.0.0.1".into())
}

/// One `/game` request (the decrypted JSON body: `protocol`, `param`, `sessionkey`, `terminal`),
/// answered with the whole reply object `{"result": 200, "response": {...}}`.
pub(crate) fn handle(request: &Json) -> Json {
    let protocol = request.s("protocol");
    let mut server = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    let reply = match dispatch(&mut server, protocol, request) {
        Ok(response) => Json::obj().with("result", 200).with("response", response),
        Err(()) => Json::obj().with("result", 400).with("response", Json::obj()),
    };
    if verbose() {
        log!("lovelive: {protocol} -> {} bytes", reply.to_string().len());
    }
    reply
}

fn dispatch(server: &mut Server, protocol: &str, req: &Json) -> Reply {
    let result = match protocol {
        "auth" => return auth(server, req),
        "unlock" | "checkword" | "setterminallog" => return Ok(Json::obj()),
        "gameconfig" => return Ok(gameconfig()),
        "information" => return information(req),
        "ranking" => return Ok(ranking()),
        "achievement" => player_op(server, req, true, achievement),
        "achievementyell" => player_op(server, req, true, achievement_yell),
        "gacha.member" => player_op(server, req, true, member_gacha),
        "gameentry" => player_op(server, req, true, game_entry),
        "gameentry.center" => player_op(server, req, true, game_entry_center),
        "gameresult" => player_op(server, req, true, game_result),
        "gametotalresult" => player_op(server, req, true, game_total_result),
        "gameexit" => player_op(server, req, true, game_exit),
        "getmembercard" => player_op(server, req, true, get_member_card),
        "music.unlock" => player_op(server, req, true, music_unlock),
        "present" => player_op(server, req, true, present),
        "printcard" => player_op(server, req, true, |p, _| Ok(cards_reply(&p.state, false))),
        "profile.inquiry" => player_op(server, req, true, profile_inquiry),
        "profile.print" => player_op(server, req, true, profile_print),
        "travelstamp" => player_op(server, req, true, travel_stamp),
        "TravelStart" => player_op(server, req, true, travel_start),
        "TravelResult" => player_op(server, req, true, travel_result),
        "TravelSnap.print" => player_op(server, req, true, travel_snap_print),
        "userdata.get" => player_op(server, req, false, |p, _| {
            let mut reply = userdata_reply(&p.state);
            // no name entry popup
            let mut flags = reply.s("flags").to_string();
            set_flag(&mut flags, 181);
            reply.set("flags", flags);
            Ok(reply)
        }),
        "userdata.initialize" => player_op(server, req, true, userdata_initialize),
        "userdata.set" => player_op(server, req, true, userdata_set),
        _ => {
            log!("lovelive: {protocol}: not known, answered empty");
            return Ok(Json::obj());
        }
    };
    // the session's state: active from the game's entry (`abnormal_end` at the next `auth`
    // while it is), gone at its exit (a card's: LLServer keeps a guest's)
    if result.is_ok()
        && let Some(s) = server.sessions.iter_mut().position(|s| s.key == req.s("sessionkey") && s.card.is_some())
    {
        match protocol {
            "gameentry" | "gameentry.center" => {
                server.sessions[s].active = true;
                server.sessions[s].expire = unix_now() + 3600;
            }
            "gameexit" => {
                server.sessions.remove(s);
            }
            _ => {}
        }
    }
    result
}

/// A request of a session's player (LLServer's `ParamHandler`: the param, an object, then the
/// session, else 400); a card's player saved after it when changed.
fn player_op(server: &mut Server, req: &Json, with_param: bool, op: impl FnOnce(&mut Player, &Json) -> Reply) -> Reply {
    let none = Json::obj();
    let prm = if with_param { param(req)? } else { &none };
    let key = req.s("sessionkey");
    let card = server.sessions.iter().find(|s| s.key == key).ok_or(())?.card.clone();
    let state = match &card {
        Some(card) => load(card).ok_or(())?,
        None => server.guest.get_or_insert_with(guest_user).clone(),
    };
    let mut player = Player { card, state, dirty: false };
    let reply = op(&mut player, prm)?;
    match &player.card {
        Some(card) if player.dirty => save(card, &player.state),
        Some(_) => {}
        None => server.guest = Some(player.state),
    }
    Ok(reply)
}

// ---- the request's param (System.Text.Json's reading: a wrong type is a 400)

fn param(req: &Json) -> Result<&Json, ()> {
    req.get("param").filter(|p| matches!(p, Json::Obj(_))).ok_or(())
}

/// An `int` value.
fn int_value(v: &Json) -> Result<i64, ()> {
    match v {
        Json::Num(n) => n.parse::<i32>().map(i64::from).map_err(|_| ()),
        _ => Err(()),
    }
}

/// An `int` (absent: 0).
fn int(o: &Json, k: &str) -> Result<i64, ()> {
    o.get(k).map_or(Ok(0), int_value)
}

/// A string (absent or null: "").
fn text(o: &Json, k: &str) -> Result<String, ()> {
    match o.get(k) {
        None | Some(Json::Null) => Ok(String::new()),
        Some(Json::Str(s)) => Ok(s.clone()),
        _ => Err(()),
    }
}

/// An `int[]` (absent: empty).
fn ints(o: &Json, k: &str) -> Result<Vec<i64>, ()> {
    match o.get(k) {
        None => Ok(Vec::new()),
        Some(Json::Arr(items)) => items.iter().map(int_value).collect(),
        _ => Err(()),
    }
}

/// An array of objects (absent: empty).
fn objs<'a>(o: &'a Json, k: &str) -> Result<&'a [Json], ()> {
    match o.get(k) {
        None => Ok(&[]),
        Some(Json::Arr(items)) if items.iter().all(|i| matches!(i, Json::Obj(_))) => Ok(items),
        _ => Err(()),
    }
}

/// A nullable object (absent or null: none).
fn opt_obj<'a>(o: &'a Json, k: &str) -> Result<Option<&'a Json>, ()> {
    match o.get(k) {
        None | Some(Json::Null) => Ok(None),
        Some(v @ Json::Obj(_)) => Ok(Some(v)),
        _ => Err(()),
    }
}

/// A profile card id (`Int128.Parse` of hex, written back as 32 upper case digits).
fn profile_card(s: &str) -> Result<String, ()> {
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(());
    }
    let t = t.trim_start_matches('0');
    if t.len() > 32 {
        return Err(());
    }
    Ok(format!("{:0>32}", t.to_ascii_uppercase()))
}

const NO_PROFILE_CARD: &str = "00000000000000000000000000000000";

// ---- records

fn n(v: i64) -> Json {
    Json::from(v)
}

fn rec(fields: &[(&str, Json)]) -> Json {
    Json::Obj(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

fn b(j: &Json, k: &str) -> bool {
    j.get(k).and_then(Json::as_bool).unwrap_or(false)
}

fn inc(j: &mut Json, k: &str, by: i64) {
    let v = j.i(k) + by;
    j.set(k, v);
}

fn ints_json(v: &[i64]) -> Json {
    Json::Arr(v.iter().map(|&i| n(i)).collect())
}

/// `key`'s array of an object (made when missing).
fn list<'a>(j: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    if !matches!(j.get(key), Some(Json::Arr(_))) {
        j.set(key, Json::Arr(Vec::new()));
    }
    match j.get_mut(key) {
        Some(Json::Arr(v)) => v,
        _ => unreachable!(),
    }
}

/// `key`'s object of an object (made when missing).
fn object<'a>(j: &'a mut Json, key: &str) -> &'a mut Json {
    if !matches!(j.get(key), Some(Json::Obj(_))) {
        j.set(key, Json::obj());
    }
    j.get_mut(key).unwrap()
}

/// The list's entry whose `key` is `id`, added (`make`) when there is none.
fn find_or_add<'a>(list: &'a mut Vec<Json>, key: &str, id: i64, make: impl FnOnce() -> Json) -> &'a mut Json {
    let i = match list.iter().position(|e| e.i(key) == id) {
        Some(i) => i,
        None => {
            list.push(make());
            list.len() - 1
        }
    };
    &mut list[i]
}

fn has(list: &[Json], key: &str, id: i64) -> bool {
    list.iter().any(|e| e.i(key) == id)
}

fn member(id: i64, new: bool) -> Json {
    rec(&[
        ("character_id", n(id)),
        ("m_card_member_id", n(0)),
        ("yell_point", n(0)),
        ("m_card_memorial_id", n(0)),
        ("yell_achieve_rank", n(0)),
        ("main", n(0)),
        ("camera", n(0)),
        ("stage", n(0)),
        ("select_count", n(0)),
        ("new", new.into()),
    ])
}

fn member_card(id: i64, count: i64, new: bool) -> Json {
    rec(&[("m_card_member_id", n(id)), ("count", n(count)), ("print_rest", n(1)), ("new", new.into())])
}

fn skill_card(id: i64, level: i64, new: bool) -> Json {
    rec(&[("m_card_skill_id", n(id)), ("skill_level", n(level)), ("print_rest", n(1)), ("new", new.into())])
}

fn memorial_card(id: i64, count: i64, new: bool) -> Json {
    rec(&[
        ("m_card_memorial_id", n(id)),
        ("count", n(count)),
        ("print_rest", n(1)),
        ("select_count", n(0)),
        ("talk_count", n(0)),
        ("goal_count", n(0)),
        ("new", new.into()),
    ])
}

/// A chart's record (LLServer's `PersistentLiveData`).
fn live(id: i64, new: bool) -> Json {
    let mut r = rec(&[("m_live_id", n(id)), ("select_count", n(0)), ("unlocked", true.into()), ("new", new.into())]);
    for k in [
        "full_combo", "total_hi_score", "technical_hi_score", "technical_hi_rate", "player_count_1", "trophy_count_gold", "trophy_count_silver",
        "trophy_count_bronze", "finale_count", "technical_rank",
    ] {
        r.set(k, 0);
    }
    r
}

/// A chart's record as the game reads it (`LiveData`: the counters LLServer does not keep 0).
fn live_out(l: &Json) -> Json {
    let mut r = Json::obj();
    for k in ["m_live_id", "select_count"] {
        r.set(k, l.i(k));
    }
    for k in ["unlocked", "new"] {
        r.set(k, b(l, k));
    }
    for k in [
        "full_combo", "total_hi_score", "technical_hi_score", "technical_hi_rate", "coop_total_hi_score_2", "coop_total_hi_score_3", "player_count_1",
        "player_count_2", "player_count_3", "rank_count_0", "rank_count_1", "rank_count_2", "rank_count_3", "rank_count_4", "rank_count_5",
        "rank_count_6", "trophy_count_gold", "trophy_count_silver", "trophy_count_bronze", "finale_count", "technical_rank",
    ] {
        r.set(k, l.i(k));
    }
    r
}

/// An achievement, limited / yell achievement, card frame, honor.
fn unlockable(key: &str, id: i64, unlocked: bool, new: bool) -> Json {
    rec(&[(key, n(id)), ("unlocked", unlocked.into()), ("new", new.into())])
}

/// A nameplate, badge.
fn plate(key: &str, id: i64, new: bool) -> Json {
    rec(&[(key, n(id)), ("new", new.into())])
}

fn pamphlet(id: i64, new: bool) -> Json {
    rec(&[
        ("m_travel_pamphlet_id", n(id)),
        ("round", n(0)),
        ("total_talk_count", n(0)),
        ("total_dice_count", n(0)),
        ("is_new", new.into()),
        ("travel_ex_rewards", Json::Arr(Vec::new())),
    ])
}

fn travel(slot: i64) -> Json {
    rec(&[
        ("slot", n(slot)),
        ("m_travel_pamphlet_id", n(0)),
        ("character_id", n(0)),
        ("m_card_memorial_id", n(0)),
        ("positions", ints_json(&[0, 0, 0])),
        ("last_landmark", n(0)),
        ("modified", "".into()),
    ])
}

/// LLServer's `UserDataBase` (the Aqours / Saint Snow user data).
fn userdata_base() -> Json {
    rec(&[
        ("character_id", n(0)),
        ("honor", n(0)),
        ("badge", n(0)),
        ("nameplate", n(0)),
        ("profile_card_id_1", NO_PROFILE_CARD.into()),
        ("profile_card_id_2", NO_PROFILE_CARD.into()),
    ])
}

const USERDATA_BASE_INTS: [&str; 4] = ["character_id", "honor", "badge", "nameplate"];
const USERDATA_INTS: [&str; 14] = [
    "idol_kind", "volume_bgm", "volume_se", "volume_voice", "screen_filter_level", "note_speed_level", "submonitor_type", "play_satellite",
    "play_center", "level", "total_exp", "credit_count_satellite", "credit_count_center", "play_ls4",
];

/// LLServer's `UserData` (µ's, and the player's).
fn userdata() -> Json {
    let mut u = userdata_base();
    u.set("idol_kind", 0);
    u.set("name", "");
    for k in &USERDATA_INTS[1..7] {
        u.set(k, 0);
    }
    u.set("tenpo_name", "Test");
    u.set("play_date", "");
    for k in &USERDATA_INTS[7..] {
        u.set(k, 0);
    }
    u.set("play_ls4", 1);
    u
}

/// The fields of a `NullableUserData(Base)` copied onto the record (LLServer's `ReflectionMapper`:
/// the ones sent; the profile card ids always, zeros when not sent; the main one's `play_ls4` 1
/// when not sent).
fn userdata_changes(src: &Json, main: bool) -> Result<Vec<(&'static str, Json)>, ()> {
    let mut out = Vec::new();
    let more: &[&'static str] = if main { &USERDATA_INTS } else { &[] };
    for &k in USERDATA_BASE_INTS.iter().chain(more) {
        match src.get(k) {
            None if main && k == "play_ls4" => out.push((k, n(1))),
            None | Some(Json::Null) => {}
            Some(_) => out.push((k, n(int(src, k)?))),
        }
    }
    if main {
        for k in ["name", "tenpo_name", "play_date"] {
            match src.get(k) {
                None | Some(Json::Null) => {}
                Some(Json::Str(s)) => out.push((k, s.as_str().into())),
                _ => return Err(()),
            }
        }
    }
    for k in ["profile_card_id_1", "profile_card_id_2"] {
        let id = match src.get(k) {
            None | Some(Json::Null) => NO_PROFILE_CARD.to_string(),
            Some(Json::Str(s)) => profile_card(s)?,
            _ => return Err(()),
        };
        out.push((k, id.into()));
    }
    Ok(out)
}

fn apply(dst: &mut Json, changes: Vec<(&str, Json)>) {
    for (k, v) in changes {
        dst.set(k, v);
    }
}

// ---- fixed data (LLServer's)

/// Each character's first member card, memorial card, yell rank 20 / 80 member card (by id).
const INITIAL_MEMBER_CARDS: [i64; 23] = [
    0, 10011, 20011, 30011, 40011, 50011, 60011, 70011, 80011, 90011, 0, 110901, 120901, 130901, 140901, 150901, 160901, 170901, 180901, 190901, 0,
    212000, 222000,
];
const INITIAL_MEMORIAL_CARDS: [i64; 23] =
    [0, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 0, 11100, 12100, 13100, 14100, 15100, 16100, 17100, 18100, 19100, 0, 21200, 22200];
const YELL_RANK_20_CARDS: [i64; 23] =
    [0, 10250, 20250, 30250, 40250, 50250, 60250, 70250, 80250, 90250, 0, 111310, 121310, 131310, 141310, 151310, 161310, 171310, 181310, 191310, 0, 0, 0];
const YELL_RANK_80_CARDS: [i64; 23] =
    [0, 10251, 20251, 30251, 40251, 50251, 60251, 70251, 80251, 90251, 0, 111311, 121311, 131311, 141311, 151311, 161311, 171311, 181311, 191311, 0, 0, 0];
const MEMBER_IDS: [i64; 20] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 21, 22];
const SKILL_CARDS_MUSE: [i64; 27] = [
    1100, 2100, 3100, 4100, 5100, 6100, 7100, 8100, 9100, 1200, 2200, 3200, 4200, 5200, 6200, 7200, 8200, 9200, 1300, 2300, 3300, 4300, 5300, 6300,
    7300, 8300, 9300,
];
const SKILL_CARDS_AQOURS: [i64; 27] = [
    11100, 12100, 13100, 14100, 15100, 16100, 17100, 18100, 19100, 11200, 12200, 13200, 14200, 15200, 16200, 17200, 18200, 19200, 11300, 12300,
    13300, 14300, 15300, 16300, 17300, 18300, 19300,
];
const SKILL_CARDS_SAINT_SNOW: [i64; 6] = [21100, 22100, 21200, 22200, 21300, 22300];
/// The skill cards' "got" achievements.
const SKILL_ACHIEVEMENTS_MUSE: [i64; 18] = [9001, 9002, 9008, 9009, 9015, 9016, 9022, 9023, 9029, 9030, 9036, 9037, 9043, 9044, 9050, 9051, 9057, 9058];
const SKILL_ACHIEVEMENTS_AQOURS: [i64; 18] = [
    509001, 509002, 509008, 509009, 509015, 509016, 509022, 509023, 509029, 509030, 509036, 509037, 509043, 509044, 509050, 509051, 509057, 509058,
];
const SKILL_ACHIEVEMENTS_SAINT_SNOW: [i64; 4] = [809001, 809002, 809008, 809009];

const GUEST_NAME: &str = "ゲストプレイヤー";
const GUEST_CARD: &str = "7020392000000000";

fn flags_with(set: &[usize]) -> String {
    let mut f = "0".repeat(200);
    for &i in set {
        set_flag(&mut f, i);
    }
    f
}

/// Sets a flag of the flags string (one character per flag; a string too short is padded).
fn set_flag(flags: &mut String, i: usize) {
    let mut c: Vec<char> = flags.chars().collect();
    if c.len() <= i {
        c.resize(i + 1, '0');
    }
    c[i] = '1';
    *flags = c.into_iter().collect();
}

// ---- players

/// A new player's record (LLServer's `RegisterNewUser`).
fn new_player(card: &str, user_id: u64) -> Json {
    let mut st = rec(&[
        ("user_id", Json::from(user_id)),
        ("card_id", card.into()),
        ("initialized", false.into()),
        ("flags", "".into()),
        ("next_history_id", n(1)),
        ("userdata", userdata()),
        ("userdata_aqours", userdata_base()),
        ("userdata_saintsnow", userdata_base()),
    ]);
    for k in [
        "members", "membercard", "skillcard", "memorialcard", "lives", "musics", "travels", "travel_pamphlets", "travel_talks", "game_history",
        "achievements", "yell_achievements", "record_books", "limited_achievements", "item", "specials", "card_frames", "nameplates", "badges",
        "honors", "mailbox", "profile_cards",
    ] {
        st.set(k, Json::Arr(Vec::new()));
    }
    st
}

/// LLServer's guest player: µ's, every member with their first cards, the tutorials of the
/// profile card's final screen and the travel memories skipped.
fn guest_user() -> Json {
    let mut st = new_player(GUEST_CARD, 0);
    let ud = object(&mut st, "userdata");
    ud.set("name", GUEST_NAME);
    *list(&mut st, "members") = MEMBER_IDS
        .iter()
        .map(|&x| {
            let mut m = member(x, false);
            m.set("m_card_member_id", INITIAL_MEMBER_CARDS[x as usize]);
            m.set("m_card_memorial_id", INITIAL_MEMORIAL_CARDS[x as usize]);
            m.set("yell_achieve_rank", 1);
            m
        })
        .collect();
    *list(&mut st, "membercard") = INITIAL_MEMBER_CARDS.iter().filter(|&&x| x != 0).map(|&x| member_card(x, 1, true)).collect();
    *list(&mut st, "memorialcard") = INITIAL_MEMORIAL_CARDS.iter().filter(|&&x| x != 0).map(|&x| memorial_card(x, 1, true)).collect();
    *list(&mut st, "skillcard") = SKILL_CARDS_MUSE.iter().map(|&x| skill_card(x, 1, true)).collect();
    st.set("flags", flags_with(&[2, 16, 59, 181]));
    st
}

fn dir() -> String {
    format!("{}\\lovelive", crate::drive::data_dir())
}

fn path(card: &str) -> String {
    let name: String = card.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{}\\{name}.json", dir())
}

fn load(card: &str) -> Option<Json> {
    std::fs::read_to_string(path(card)).ok().and_then(|t| Json::parse(&t)).filter(|j| matches!(j, Json::Obj(_)))
}

fn save(card: &str, state: &Json) {
    let _ = std::fs::create_dir_all(dir());
    if let Err(e) = std::fs::write(path(card), state.to_string()) {
        log!("lovelive: {}: {e}", path(card));
    }
}

/// Every saved player's record.
fn all_players() -> Vec<Json> {
    let Ok(entries) = std::fs::read_dir(dir()) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok().and_then(|t| Json::parse(&t)))
        .collect()
}

// ---- time, randomness

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The local date and time (year, month, day, hour, minute, second).
fn local_now() -> [i64; 6] {
    let t = unsafe {
        let mut t = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        t
    };
    [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond].map(i64::from)
}

/// `yyyy-MM-dd<sep>HH:mm:ss`, or `hh` (12-hour) for LLServer's strings that use it.
fn format_time(t: [i64; 6], sep: &str, twelve_hour: bool) -> String {
    let h = if twelve_hour { (t[3] + 11) % 12 + 1 } else { t[3] };
    format!("{:04}-{:02}-{:02}{sep}{:02}:{:02}:{:02}", t[0], t[1], t[2], h, t[4], t[5])
}

/// `yyyy-MM-ddHH:mm:ss`, the game's usual time.
fn now() -> String {
    format_time(local_now(), "", false)
}

/// The date `days` after `y-m-d` (proleptic Gregorian).
fn add_days(y: i64, m: i64, d: i64, days: i64) -> (i64, i64, i64) {
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let z = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy + days;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// xorshift64, seeded from the time.
fn random() -> u64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64) ^ 0x9E37_79B9_7F4A_7C15 | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x
}

/// 32 lower case hex digits (a `Guid` "N").
fn new_id() -> String {
    format!("{:016x}{:016x}", random(), random())
}

// ---- replies

/// The user data reply (`UserDataResponse`: `userdata.get`, `.initialize`, `.set`,
/// `gameentry.center`).
fn userdata_reply(st: &Json) -> Json {
    let all = |k: &str| Json::Arr(st.a(k).to_vec());
    let histories = |kind: i64| {
        Json::Arr(
            st.a("game_history")
                .iter()
                .filter(|h| h.i("_idol_kind") == kind)
                .map(|h| match h {
                    Json::Obj(f) => Json::Obj(f.iter().filter(|(k, _)| k != "_idol_kind").cloned().collect()),
                    other => other.clone(),
                })
                .collect(),
        )
    };
    let gacha_status = |id: i64| rec(&[("m_gacha_id", n(id)), ("usage_count", n(1)), ("idol_kind", n(0)), ("in_use", false.into())]);
    rec(&[
        ("userdata", st.get("userdata").cloned().unwrap_or_else(userdata)),
        ("userdata_aqours", st.get("userdata_aqours").cloned().unwrap_or_else(userdata_base)),
        ("userdata_saintsnow", st.get("userdata_saintsnow").cloned().unwrap_or_else(userdata_base)),
        ("members", all("members")),
        ("membercard", all("membercard")),
        ("skillcard", all("skillcard")),
        ("memorialcard", all("memorialcard")),
        ("item", all("item")),
        ("musics", all("musics")),
        ("lives", Json::Arr(st.a("lives").iter().map(live_out).collect())),
        ("stages", Json::Arr(Vec::new())),
        ("game_history", histories(0)),
        ("game_history_aqours", histories(1)),
        ("game_history_saintsnow", histories(2)),
        ("travel_history", Json::Arr(Vec::new())),
        ("travel_history_aqours", Json::Arr(Vec::new())),
        ("travel_history_saintsnow", Json::Arr(Vec::new())),
        ("mailbox", all("mailbox")),
        ("specials", all("specials")),
        ("flags", st.s("flags").into()),
        ("achievements", all("achievements")),
        ("record_books", all("record_books")),
        ("yell_achievements", all("yell_achievements")),
        ("limited_achievements", all("limited_achievements")),
        ("honors", all("honors")),
        (
            "scfes_profile",
            rec(&[
                ("enable", true.into()),
                ("name", "".into()),
                ("unit_id", n(1)),
                ("award_id", n(1)),
                ("rank_up_flag", n(0)),
                ("invalid_name", false.into()),
                ("live_list", ints_json(&[0, 10, 20])),
            ]),
        ),
        ("travels", all("travels")),
        ("travel_pamphlets", all("travel_pamphlets")),
        ("travel_talks", all("travel_talks")),
        ("gacha_status", Json::Arr(vec![gacha_status(1), gacha_status(2)])),
        ("card_frames", all("card_frames")),
        ("nameplates", all("nameplates")),
        ("badges", all("badges")),
        ("now", now().into()),
        ("first_login", false.into()),
        ("dice_bonus", true.into()),
        ("active_information", Json::Arr(Vec::new())),
    ])
}

/// The cards (`printcard`; `present` adds the items and mailbox).
fn cards_reply(st: &Json, present: bool) -> Json {
    let mut r = Json::obj();
    for k in ["membercard", "skillcard", "memorialcard"] {
        r.set(k, Json::Arr(st.a(k).to_vec()));
    }
    if present {
        for k in ["item", "mailbox"] {
            r.set(k, Json::Arr(st.a(k).to_vec()));
        }
    }
    r
}

fn event_status() -> Json {
    rec(&[("m_event_id", n(0)), ("event_point", n(0)), ("next_reward", n(0)), ("level", n(0)), ("rank", n(0)), ("first_play", n(0))])
}

fn event_result() -> Json {
    rec(&[("m_event_id", n(0)), ("add_point", n(0)), ("event_point", n(0)), ("update_score", n(0)), ("rewards", Json::Arr(Vec::new()))])
}

// ---- protocols

fn auth(server: &mut Server, req: &Json) -> Reply {
    let prm = param(req)?;
    let reply = |key: String, user: String, status: i64, abnormal: i64, name: &str| {
        rec(&[
            ("sessionkey", key.into()),
            ("user_id", user.into()),
            ("blockseq", n(1)),
            ("status", n(status)),
            ("abnormal_end", n(abnormal)),
            ("name", name.into()),
        ])
    };
    if int(prm, "guestflag")? == 1 {
        let key = new_id();
        server.sessions.push(Session { key: key.clone(), card: None, active: false, expire: 0 });
        // guests' sessions stay (LLServer's never end): the oldest go
        if server.sessions.len() > 256 {
            server.sessions.remove(0);
        }
        return Ok(reply(key, "1".into(), 0, 0, GUEST_NAME));
    }
    let card = match prm.get("nesicaid") {
        None => GUEST_CARD.to_string(),
        Some(_) => text(prm, "nesicaid")?,
    };
    if !matches!(req.get("terminal"), Some(Json::Obj(_))) {
        return Err(());
    }
    let state = match load(&card) {
        Some(state) => {
            log!("lovelive: card {card} logged in");
            state
        }
        None => {
            let user_id = all_players().iter().map(|p| p.u("user_id")).max().unwrap_or(0) + 1;
            let state = new_player(&card, user_id);
            save(&card, &state);
            log!("lovelive: card {card}: new player {user_id}");
            state
        }
    };
    let mut abnormal = 0;
    if let Some(i) = server.sessions.iter().position(|s| s.card.as_deref() == Some(card.as_str())) {
        let old = server.sessions.remove(i);
        if old.active && old.expire > unix_now() {
            log!("lovelive: card {card}: its last game did not end");
            abnormal = 1;
        }
    }
    let key = new_id();
    server.sessions.push(Session { key: key.clone(), card: Some(card), active: false, expire: 0 });
    let name = state.get("userdata").map_or("", |u| u.s("name")).to_string();
    Ok(reply(key, state.u("user_id").to_string(), if b(&state, "initialized") { 0 } else { 1 }, abnormal, &name))
}

fn gameconfig() -> Json {
    let demo = (random() % 10) as i64;
    rec(&[
        ("version", "2.4.1".into()),
        ("season_event_uid", "0".into()),
        ("level_cap", n(200)),
        ("skill_level_cap", n(999)),
        ("yell_rank_cap", n(999)),
        ("card_sells_value", n(100)),
        ("music_release_version", n(99999)),
        ("finale_focus_release_version", n(99999)),
        ("stage_release_version", n(99999)),
        ("member_card_release_version", n(99999)),
        ("memorial_card_release_version", n(99999)),
        ("skill_card_release_version", n(99999)),
        ("advertise_movie_status_0", true.into()),
        ("advertise_movie_status_1", false.into()),
        ("advertise_movie_status_2", false.into()),
        ("advertise_movie_status_3", false.into()),
        ("live_demo_table_id", n(demo)),
        ("exp_mag", n(1)),
        ("exp_mag_target_level", n(5)),
        ("cheer_enable", false.into()),
        ("achievement_release_version", n(99999)),
        ("travel_pamphlet_release_version", n(99999)),
        ("live_break_max", n(1)),
        ("yell_achieve_mobile_point", n(100)),
        ("yell_point_mag", n(1)),
        ("dice_bonus_credit", n(100)),
    ])
}

fn information(req: &Json) -> Reply {
    let terminal = req.get("terminal").filter(|t| matches!(t, Json::Obj(_))).ok_or(())?;
    int(terminal, "terminal_attrib")?;
    let t = local_now();
    let (y, m, d) = add_days(t[0], t[1], t[2], 3650);
    let gacha = |id: i64, kind: i64, resource: &str, image: &str| {
        rec(&[
            ("id", n(id)),
            ("idol_kind", n(0)),
            ("gacha_type", n(kind)),
            ("character_id", n(1)),
            ("priority", n(1)),
            ("draw_limit", n(0)),
            ("cracker_enable", n(1)),
            ("is_event", n(0)),
            ("first_only", n(0)),
            ("rates", ints_json(&[1, 2, 3])),
            ("resource", resource.into()),
            ("gacha_id", "1".into()),
            ("image", image.into()),
            ("image_event", image.into()),
            ("title", "test title".into()),
            ("caption", "2".into()),
            ("menu_caption", "3".into()),
            ("start_datetime", "2021-01-0112:00:00".into()),
            ("end_datetime", "2099-01-0112:00:00".into()),
            ("start_menu_datetime", "2021-01-0112:00:00".into()),
            ("end_menu_datetime", "2099-01-0112:00:00".into()),
        ])
    };
    Ok(rec(&[
        ("base_url", format!("http://{}/info/", host()).into()),
        ("information", Json::Arr(Vec::new())),
        ("event_information", Json::Arr(Vec::new())),
        ("encore_expiration_date", format_time([y, m, d, 0, 0, 0], "", true).into()),
        ("resource_information", Json::Arr(Vec::new())),
        (
            "gacha_information",
            rec(&[
                ("notes", "wtf!!!".into()),
                ("gacha_list", Json::Arr(vec![gacha(1, 1, "456", "info_503_1.jpg"), gacha(2, 2, "123", "info_770_1.jpg"), gacha(3, 3, "123", "info_440_2.jpg")])),
            ]),
        ),
        ("sif_music_information", Json::Arr(Vec::new())),
        ("ranking_information", rec(&[("yell_rankings", Json::Arr(Vec::new()))])),
    ]))
}

fn ranking() -> Json {
    let rows = |id_key: &str| Json::Arr((0..10).map(|i| rec(&[("rank", n(i + 1)), ("score", n(69420)), (id_key, n(i))])).collect());
    let yell = Json::Arr(
        (0..10)
            .map(|i| {
                rec(&[
                    ("rank", n(i + 1)),
                    ("score", n(69420)),
                    ("honor", n(0)),
                    ("name", "Test".into()),
                    ("tenpo_name", "1337".into()),
                    ("nameplate", n(0)),
                    ("badge", n(0)),
                    ("uid", "1234567890".into()),
                ])
            })
            .collect(),
    );
    rec(&[
        ("ranking", rows("music_id")),
        ("coop_ranking", rows("character_id")),
        ("music_ranking", rows("music_id")),
        ("music_total", n(10)),
        ("member_ranking", rows("character_id")),
        ("yell_ranking", yell),
    ])
}

fn userdata_initialize(p: &mut Player, prm: &Json) -> Reply {
    let ud = opt_obj(prm, "userdata")?;
    let changes = ud.map(|u| userdata_changes(u, true)).transpose()?;
    let aqours = opt_obj(prm, "userdata_aqours")?.map(|u| userdata_changes(u, false)).transpose()?;
    let saint_snow = opt_obj(prm, "userdata_saintsnow")?.map(|u| userdata_changes(u, false)).transpose()?;
    let kind = match ud.and_then(|u| u.get("idol_kind")) {
        None | Some(Json::Null) => None,
        Some(_) => Some(int(ud.unwrap(), "idol_kind")?),
    };
    if p.card.is_none() {
        return Ok(userdata_reply(&p.state));
    }
    let st = &mut p.state;
    if let Some(changes) = changes {
        apply(object(st, "userdata"), changes);
        if let Some(c) = aqours {
            apply(object(st, "userdata_aqours"), c);
        }
        if let Some(c) = saint_snow {
            apply(object(st, "userdata_saintsnow"), c);
        }
        // the chosen group's first cards and their achievements
        type Group = (fn(i64) -> bool, &'static [i64], fn(i64) -> bool, &'static [i64]);
        let group: Option<Group> = match kind {
            Some(0) => Some((|x| x < 90012, &SKILL_CARDS_MUSE, |x| x < 10000, &SKILL_ACHIEVEMENTS_MUSE)),
            Some(1) => Some((|x| x > 90011 && x < 190902, &SKILL_CARDS_AQOURS, |x| x > 10000 && x < 20000, &SKILL_ACHIEVEMENTS_AQOURS)),
            Some(2) => Some((|x| x > 190902, &SKILL_CARDS_SAINT_SNOW, |x| x > 20000, &SKILL_ACHIEVEMENTS_SAINT_SNOW)),
            _ => None,
        };
        if let Some((members, skills, memorials, achievements)) = group {
            let cards = list(st, "membercard");
            for &x in INITIAL_MEMBER_CARDS.iter().filter(|&&x| members(x) && x != 0) {
                if !has(cards, "m_card_member_id", x) {
                    cards.push(member_card(x, 1, false));
                }
            }
            list(st, "skillcard").extend(skills.iter().map(|&x| skill_card(x, 1, false)));
            let cards = list(st, "memorialcard");
            for &x in INITIAL_MEMORIAL_CARDS.iter().filter(|&&x| memorials(x) && x != 0) {
                if !has(cards, "m_card_memorial_id", x) {
                    cards.push(memorial_card(x, 1, false));
                }
            }
            list(st, "achievements").extend(achievements.iter().map(|&x| unlockable("m_achievement_id", x, true, false)));
        }
        let ud = st.get("userdata").cloned().unwrap_or_else(userdata);
        list(st, "nameplates").push(plate("m_nameplate_id", ud.i("nameplate"), false));
        list(st, "badges").push(plate("m_badge_id", ud.i("badge"), false));
        list(st, "honors").push(unlockable("m_honor_id", ud.i("honor"), true, false));
        object(st, "userdata").set("level", 1);
        // tutorials seen: controls, group selection, anniversary snap, 53, the name set
        st.set("flags", flags_with(&[0, 10, 13, 53, 181]));
    }
    st.set("initialized", true);
    p.dirty = true;
    Ok(userdata_reply(&p.state))
}

/// The nameplate / badge / honor chosen, added to the player's when new.
fn add_attributes(st: &mut Json, src: Option<&Json>) -> Result<(), ()> {
    let Some(src) = src else { return Ok(()) };
    for (field, key, list_key) in [("nameplate", "m_nameplate_id", "nameplates"), ("badge", "m_badge_id", "badges"), ("honor", "m_honor_id", "honors")] {
        if matches!(src.get(field), None | Some(Json::Null)) {
            continue;
        }
        let id = int(src, field)?;
        let l = list(st, list_key);
        if !has(l, key, id) {
            l.push(if field == "honor" { unlockable(key, id, true, true) } else { plate(key, id, true) });
        }
    }
    Ok(())
}

fn userdata_set(p: &mut Player, prm: &Json) -> Reply {
    let sources = [opt_obj(prm, "userdata")?, opt_obj(prm, "userdata_aqours")?, opt_obj(prm, "userdata_saintsnow")?];
    let mut changes = Vec::new();
    for (i, src) in sources.iter().enumerate() {
        changes.push(src.map(|u| userdata_changes(u, i == 0)).transpose()?);
    }
    let equip = match prm.get("equipskill") {
        None | Some(Json::Null) => &[][..],
        Some(_) => objs(prm, "equipskill")?,
    };
    let yells = match prm.get("member_yell") {
        None | Some(Json::Null) => &[][..],
        Some(_) => objs(prm, "member_yell")?,
    };
    let mut equipped = Vec::new();
    for e in equip {
        equipped.push([int(e, "character_id")?, int(e, "m_card_member_id")?, int(e, "m_card_memorial_id")?, int(e, "camera")?, int(e, "main")?, int(e, "stage")?]);
    }
    let mut yell_points = Vec::new();
    for y in yells {
        yell_points.push((int(y, "character_id")?, int(y, "yell_point")?));
    }
    if p.card.is_none() {
        return Ok(Json::obj());
    }
    let st = &mut p.state;
    for (key, c) in ["userdata", "userdata_aqours", "userdata_saintsnow"].into_iter().zip(changes) {
        if let Some(c) = c {
            apply(object(st, key), c);
        }
    }
    let members = list(st, "members");
    for [id, card, memorial, camera, main, stage] in equipped {
        let m = find_or_add(members, "character_id", id, || member(id, false));
        m.set("m_card_member_id", card);
        m.set("m_card_memorial_id", memorial);
        m.set("camera", camera);
        m.set("main", main);
        m.set("stage", stage);
    }
    for (id, points) in yell_points {
        find_or_add(members, "character_id", id, || member(id, true)).set("yell_point", points);
    }
    for src in sources {
        add_attributes(st, src)?;
    }
    // the difficulty explanation (after a song's selection), and the tutorials of a first play
    let mut flags = st.s("flags").to_string();
    for i in [7, 0, 10, 13, 53, 181] {
        set_flag(&mut flags, i);
    }
    st.set("flags", flags);
    p.dirty = true;
    Ok(userdata_reply(&p.state))
}

/// The game entry reply (`GameEntryResponse`).
fn game_entry_reply(st: &Json, userdata: Json, aqours: Json) -> Json {
    let all = |k: &str| Json::Arr(st.a(k).to_vec());
    rec(&[
        ("userdata", userdata),
        ("userdata_aqours", aqours),
        ("userdata_saintsnow", st.get("userdata_saintsnow").cloned().unwrap_or_else(userdata_base)),
        ("membercard", all("membercard")),
        ("skillcard", all("skillcard")),
        ("memorialcard", all("memorialcard")),
        ("item", all("item")),
        ("mailbox", all("mailbox")),
        ("specials", all("specials")),
        ("now", now().into()),
        ("first_login", false.into()),
        ("travel_pamphlets", all("travel_pamphlets")),
        ("stamp_card_rewards", Json::Arr(Vec::new())),
        ("active_information", Json::Arr(Vec::new())),
        ("event_rewards", Json::Arr(Vec::new())),
        ("members", all("members")),
    ])
}

fn game_entry(p: &mut Player, prm: &Json) -> Reply {
    let released = ints(prm, "release_pamphlet_ids")?;
    let st = &mut p.state;
    if p.card.is_none() {
        // the guest's first characters: Honoka, Chika
        let mut ud = st.get("userdata").cloned().unwrap_or_else(userdata);
        ud.set("character_id", 1);
        let mut aqours = st.get("userdata_aqours").cloned().unwrap_or_else(userdata_base);
        aqours.set("character_id", 11);
        return Ok(game_entry_reply(st, ud, aqours));
    }
    let ud = object(st, "userdata");
    inc(ud, "play_satellite", 1);
    let last_play = ud.s("play_date").to_string();
    ud.set("play_date", format_time(local_now(), " ", false));
    let pamphlets = list(st, "travel_pamphlets");
    for id in released {
        if !has(pamphlets, "m_travel_pamphlet_id", id) {
            pamphlets.push(pamphlet(id, true));
        }
    }
    // µ's memorial cards missing
    let cards = list(st, "memorialcard");
    for &x in INITIAL_MEMORIAL_CARDS.iter().filter(|&&x| x != 0 && x < 10000) {
        if !has(cards, "m_card_memorial_id", x) {
            cards.push(memorial_card(x, 1, false));
        }
    }
    p.dirty = true;
    let mut ud = p.state.get("userdata").cloned().unwrap_or_else(userdata);
    ud.set("play_date", last_play);
    let aqours = p.state.get("userdata_aqours").cloned().unwrap_or_else(userdata_base);
    Ok(game_entry_reply(&p.state, ud, aqours))
}

fn game_entry_center(p: &mut Player, _prm: &Json) -> Reply {
    let ud = object(&mut p.state, "userdata");
    inc(ud, "play_center", 1);
    ud.set("play_date", format_time(local_now(), " ", false));
    p.dirty = true;
    let mut reply = userdata_reply(&p.state);
    // no name entry or tutorials on the center
    let flags = reply.s("flags").replace('0', "1");
    reply.set("flags", flags);
    Ok(reply)
}

/// The fields of a game history copied from the game's result.
const HISTORY_INTS: [&str; 7] = ["character_id", "m_membercard_id", "used_member_card", "yell_rank", "badge", "nameplate", "honor"];
const HISTORY_ARRAYS: [&str; 10] = [
    "skill_cards_main", "skill_cards_camera", "skill_cards_stage", "skill_levels_main", "skill_levels_camera", "skill_levels_stage", "skill_status_main",
    "skill_status_camera", "skill_status_stage", "skill_arg_camera",
];
const HISTORY_SCORES: [&str; 22] = [
    "m_live_id", "m_stage_id", "event_mode", "member_count", "play_part", "max_combo", "full_combo", "note_miss_count", "note_bad_count",
    "note_good_count", "note_great_count", "note_perfect_count", "finale_point", "technical_score", "skill_score", "synchro_score", "combo_score",
    "technical_rank", "skill_rank", "synchro_rank", "combo_rank", "total_rank",
];

fn game_result(p: &mut Player, prm: &Json) -> Reply {
    let character = int(prm, "character_id")?;
    let used_card = int(prm, "used_member_card")?;
    let unlocked = match prm.get("unlock_live_id") {
        Some(Json::Arr(_)) => ints(prm, "unlock_live_id")?,
        _ => Vec::new(),
    };
    let live_id = int(prm, "m_live_id")?;
    let [total_score, rate, technical, rank, full_combo, finale] =
        [int(prm, "total_score")?, int(prm, "technical_rate")?, int(prm, "technical_score")?, int(prm, "technical_rank")?, int(prm, "full_combo")?, int(prm, "finale_point")?];
    let kind = int(prm, "idol_kind")?;
    if p.card.is_none() {
        return Ok(rec(&[
            ("musics", Json::Arr(Vec::new())),
            ("lives", Json::Arr(Vec::new())),
            ("stages", Json::Arr(Vec::new())),
            ("event_status", event_status()),
            ("event_rewards", Json::Arr(Vec::new())),
            ("event_result", event_result()),
        ]));
    }
    let mut cards = [String::new(), String::new()];
    for (i, k) in ["profile_card_id_1", "profile_card_id_2"].into_iter().enumerate() {
        cards[i] = match prm.get(k) {
            Some(Json::Str(s)) => profile_card(s)?,
            _ => return Err(()),
        };
    }
    let (total_exp, level) = (int(prm, "total_exp")?, int(prm, "d_user_level")?);
    let st = &mut p.state;
    let history_id = st.i("next_history_id").max(1);
    let mut history = rec(&[
        ("id", history_id.to_string().into()),
        ("play_place", "test".into()),
        ("created", now().into()),
        ("d_user_id", st.u("user_id").to_string().into()),
    ]);
    for k in HISTORY_INTS {
        history.set(k, int(prm, k)?);
    }
    for k in HISTORY_ARRAYS {
        history.set(k, ints_json(&ints(prm, k)?));
    }
    for k in HISTORY_SCORES {
        history.set(k, int(prm, k)?);
    }
    history.set("favorite", false);
    history.set("print_rest", 1);
    history.set("memorial_card", int(prm, "memorial_card")?);
    history.set("last_cut_focus", int(prm, "last_cut_focus")?);
    for k in ["recommend_hi_score", "recommend_first_music", "recommend_first_member", "recommend_first_skill"] {
        history.set(k, false);
    }
    history.set("_idol_kind", kind);

    let m = find_or_add(list(st, "members"), "character_id", character, || {
        let mut m = member(character, true);
        m.set("m_card_member_id", used_card);
        m
    });
    inc(m, "select_count", 1);
    let lives = list(st, "lives");
    for id in unlocked {
        if !has(lives, "m_live_id", id) {
            lives.push(live(id, true));
        }
    }
    let l = find_or_add(lives, "m_live_id", live_id, || live(live_id, false));
    for (k, v) in [("total_hi_score", total_score), ("technical_hi_rate", rate), ("technical_hi_score", technical), ("technical_rank", rank)] {
        if v > l.i(k) {
            l.set(k, v);
        }
    }
    if full_combo > 0 {
        l.set("full_combo", full_combo);
    }
    if finale > 0 {
        inc(l, "finale_count", 1);
    }
    inc(l, "select_count", 1);
    inc(l, "player_count_1", 1);
    l.set("unlocked", true);
    l.set("new", false);
    // LLServer's limit of 100 histories works on a copy: they all stay
    list(st, "game_history").push(history);
    st.set("next_history_id", history_id + 1);
    let ud_key = match kind {
        0 => Some("userdata"),
        1 => Some("userdata_aqours"),
        2 => Some("userdata_saintsnow"),
        _ => None,
    };
    if let Some(key) = ud_key {
        let ud = object(st, key);
        ud.set("profile_card_id_1", cards[0].as_str());
        ud.set("profile_card_id_2", cards[1].as_str());
    }
    let ud = object(st, "userdata");
    ud.set("total_exp", total_exp);
    ud.set("level", level);
    // the score's tutorial: once
    let mut flags = st.s("flags").to_string();
    set_flag(&mut flags, 9);
    st.set("flags", flags);
    p.dirty = true;
    log!("lovelive: live {live_id} score {total_score} technical {technical} rank {rank}");
    Ok(rec(&[
        ("musics", Json::Arr(p.state.a("musics").to_vec())),
        ("lives", Json::Arr(p.state.a("lives").iter().map(live_out).collect())),
        ("stages", Json::Arr(Vec::new())),
        ("event_status", event_status()),
        ("event_rewards", Json::Arr(Vec::new())),
        ("event_result", event_result()),
    ]))
}

fn game_total_result(p: &mut Player, _prm: &Json) -> Reply {
    let player = rec(&[
        ("m_stamp_id", n(1)),
        ("yell_rank", n(1)),
        ("character_id", n(1)),
        ("name", GUEST_NAME.into()),
        ("nameplate", n(1)),
        ("badge", n(1)),
    ]);
    Ok(rec(&[
        (
            "cheer",
            rec(&[
                ("yell_count", n(0)),
                ("cheer_rank", n(1)),
                ("reward_mobile_point", n(1)),
                ("now", now().into()),
                ("cheer_players", Json::Arr(vec![player])),
            ]),
        ),
        ("item", Json::Arr(p.state.a("item").to_vec())),
    ]))
}

fn game_exit(p: &mut Player, prm: &Json) -> Reply {
    let seen = |k: &str| ints(prm, k);
    let (achievements, badges, honors, limited, lives, member_cards, members, memorials, musics, nameplates, skills) = (
        seen("achievements")?,
        seen("badges")?,
        seen("honors")?,
        seen("limited_achievements")?,
        seen("lives")?,
        seen("membercard")?,
        seen("members")?,
        seen("memorialcard")?,
        seen("musics")?,
        seen("nameplates")?,
        seen("skillcard")?,
    );
    ints(prm, "stages")?;
    let flags = text(prm, "flags")?;
    let st = &mut p.state;
    // what was seen: no longer new (added when missing)
    let unlock = |st: &mut Json, list_key: &str, key: &str, ids: Vec<i64>| {
        let l = list(st, list_key);
        for id in ids {
            let e = find_or_add(l, key, id, || unlockable(key, id, true, false));
            e.set("new", false);
            e.set("unlocked", true);
        }
    };
    unlock(st, "achievements", "m_achievement_id", achievements);
    let l = list(st, "badges");
    for id in badges {
        find_or_add(l, "m_badge_id", id, || plate("m_badge_id", id, false)).set("new", false);
    }
    st.set("flags", flags);
    unlock(st, "honors", "m_honor_id", honors);
    unlock(st, "limited_achievements", "m_limited_achievement_id", limited);
    let l = list(st, "lives");
    for id in lives {
        let e = find_or_add(l, "m_live_id", id, || live(id, false));
        e.set("new", false);
        e.set("unlocked", true);
    }
    let l = list(st, "membercard");
    for id in member_cards {
        find_or_add(l, "m_card_member_id", id, || member_card(id, 1, false)).set("new", false);
    }
    let l = list(st, "members");
    for id in members {
        find_or_add(l, "character_id", id, || member(id, false)).set("new", false);
    }
    let l = list(st, "memorialcard");
    for id in memorials {
        find_or_add(l, "m_card_memorial_id", id, || memorial_card(id, 0, false)).set("new", false);
    }
    let l = list(st, "musics");
    for id in musics {
        find_or_add(l, "music_id", id, || rec(&[("music_id", n(id)), ("unlocked", true.into()), ("new", false.into())])).set("new", false);
    }
    let l = list(st, "nameplates");
    for id in nameplates {
        find_or_add(l, "m_nameplate_id", id, || plate("m_nameplate_id", id, false)).set("new", false);
    }
    let l = list(st, "skillcard");
    for id in skills {
        find_or_add(l, "m_card_skill_id", id, || skill_card(id, 0, false)).set("new", false);
    }
    p.dirty = true;
    Ok(Json::obj())
}

fn get_member_card(p: &mut Player, prm: &Json) -> Reply {
    let id = int(prm, "membercard_id")?;
    let l = list(&mut p.state, "membercard");
    match l.iter_mut().find(|c| c.i("m_card_member_id") == id) {
        Some(c) => inc(c, "count", 1),
        None => l.push(member_card(id, 1, true)),
    }
    p.dirty = true;
    Ok(rec(&[("membercard", Json::Arr(l.clone()))]))
}

fn music_unlock(p: &mut Player, prm: &Json) -> Reply {
    let music = rec(&[("music_id", n(int(prm, "music_id")?)), ("unlocked", true.into()), ("new", true.into())]);
    if p.card.is_none() {
        return Ok(Json::obj());
    }
    list(&mut p.state, "musics").push(music.clone());
    p.dirty = true;
    Ok(rec(&[("musics", Json::Arr(vec![music])), ("lives", Json::Arr(Vec::new()))]))
}

/// A mailbox entry taken (`id`): its card the player's (a skill card sold: nothing).
fn present(p: &mut Player, prm: &Json) -> Reply {
    let id = match prm.get("id") {
        Some(Json::Arr(items)) => match items.first() {
            Some(Json::Str(s)) => s.clone(),
            Some(Json::Null) => String::new(),
            _ => return Err(()),
        },
        Some(Json::Str(s)) => s.clone(),
        Some(Json::Null) => String::new(),
        _ => return Err(()),
    };
    let sell = int(prm, "sell")?;
    let st = &mut p.state;
    let Some(i) = st.a("mailbox").iter().position(|m| m.get("id").and_then(Json::as_str) == Some(id.as_str())) else {
        log!("lovelive: mailbox entry {id} not found");
        return Ok(Json::obj());
    };
    let entry = list(st, "mailbox").remove(i);
    let item = entry.i("item_id");
    match entry.i("category") {
        1 => {
            let l = list(st, "membercard");
            match l.iter_mut().find(|c| c.i("m_card_member_id") == item) {
                Some(c) => inc(c, "count", 1),
                None => l.push(member_card(item, 1, true)),
            }
        }
        2 if sell != 1 => {
            let l = list(st, "skillcard");
            match l.iter_mut().find(|c| c.i("m_card_skill_id") == item) {
                Some(c) => inc(c, "skill_level", 1),
                None => l.push(skill_card(item, 1, true)),
            }
        }
        6 => {
            let l = list(st, "memorialcard");
            match l.iter_mut().find(|c| c.i("m_card_memorial_id") == item) {
                Some(c) => inc(c, "count", 1),
                None => l.push(memorial_card(item, 1, true)),
            }
        }
        _ => {}
    }
    p.dirty = true;
    Ok(cards_reply(&p.state, true))
}

/// A printed profile card (game history `id`) found from its id, any player's.
fn profile_inquiry(_p: &mut Player, prm: &Json) -> Reply {
    let id = text(prm, "profile_card_id")?;
    let found = all_players().into_iter().find_map(|owner| {
        let history = owner.a("profile_cards").iter().find(|c| c.s("profile_card_id") == id)?.s("game_history_id").to_string();
        let game = owner.a("game_history").iter().find(|h| h.s("id") == history)?.clone();
        Some((owner, game))
    });
    let Some((owner, game)) = found else {
        log!("lovelive: profile card {id} not found");
        return Ok(Json::obj());
    };
    let mut game = match game {
        Json::Obj(f) => Json::Obj(f.into_iter().filter(|(k, _)| k != "_idol_kind").collect()),
        other => other,
    };
    game.set("game_version", 65535);
    game.set("profile_version", 65535);
    // `userdata` as a UserDataBase: its common fields only
    let ud = owner.get("userdata").cloned().unwrap_or_else(userdata);
    let mut base = userdata_base();
    for k in ["character_id", "honor", "badge", "nameplate", "profile_card_id_1", "profile_card_id_2"] {
        base.set(k, ud.get(k).cloned().unwrap_or(Json::Null));
    }
    Ok(rec(&[
        ("owner_id", Json::from(owner.u("user_id"))),
        ("first_relation", n(0)),
        ("userdata", base),
        ("userdata_aqours", owner.get("userdata_aqours").cloned().unwrap_or_else(userdata_base)),
        ("userdata_saintsnow", owner.get("userdata_saintsnow").cloned().unwrap_or_else(userdata_base)),
        ("game", game),
    ]))
}

fn profile_print(p: &mut Player, prm: &Json) -> Reply {
    let history = text(prm, "game_history_id")?;
    if p.card.is_none() {
        // LLServer: no player behind a guest's session
        return Err(());
    }
    let id = new_id();
    let history = if p.state.a("game_history").iter().any(|h| h.s("id") == history) { history } else { "0".into() };
    list(&mut p.state, "profile_cards").push(rec(&[("profile_card_id", id.as_str().into()), ("game_history_id", history.into())]));
    p.dirty = true;
    Ok(rec(&[("profile_card_id", id.into()), ("created", now().into())]))
}

fn travel_stamp(_p: &mut Player, prm: &Json) -> Reply {
    match prm.get("travel_history_ids") {
        None => {}
        Some(Json::Arr(ids)) if ids.iter().all(|i| matches!(i, Json::Str(_) | Json::Null)) => {}
        _ => return Err(()),
    }
    Ok(rec(&[("travel_history", Json::Arr(Vec::new()))]))
}

fn travel_start(p: &mut Player, prm: &Json) -> Reply {
    let (character, pamphlet_id, slot) = (int(prm, "character_id")?, int(prm, "m_travel_pamphlet_id")?, int(prm, "slot")?);
    let reply = rec(&[("other_players", Json::Arr(Vec::new()))]);
    if p.card.is_none() {
        return Ok(reply);
    }
    let st = &mut p.state;
    let t = find_or_add(list(st, "travels"), "slot", slot, || travel(slot));
    t.set("character_id", character);
    t.set("m_travel_pamphlet_id", pamphlet_id);
    let l = list(st, "travel_pamphlets");
    if !has(l, "m_travel_pamphlet_id", pamphlet_id) {
        l.push(pamphlet(pamphlet_id, true));
    }
    p.dirty = true;
    Ok(reply)
}

fn travel_result_empty() -> Json {
    rec(&[("get_card_datas", Json::Arr(Vec::new())), ("travel_history_ids", Json::Arr(Vec::new())), ("mailbox", Json::Arr(Vec::new()))])
}

/// A board game's end: what was won, the travel's state, the cards to the mailbox.
fn travel_result(p: &mut Player, prm: &Json) -> Reply {
    let released = ints(prm, "release_pamphlet_ids")?;
    let mut talks = Vec::new();
    for t in objs(prm, "travel_talks")? {
        talks.push([int(t, "talk_id")?, int(t, "my_character_id")?, int(t, "other_character_id")?]);
    }
    let (badges, frames, nameplates, specials) = (ints(prm, "badges")?, ints(prm, "card_frames")?, ints(prm, "nameplates")?, ints(prm, "special_ids")?);
    let mut items = Vec::new();
    for i in objs(prm, "item")? {
        items.push((int(i, "m_item_id")?, int(i, "count")?));
    }
    let (level, total_exp, dice, talk_count) = (int(prm, "level")?, int(prm, "total_exp")?, int(prm, "dice_count")?, int(prm, "talk_count")?);
    let mut yells = Vec::new();
    for y in objs(prm, "member_yell")? {
        yells.push((int(y, "character_id")?, int(y, "yell_point")?));
    }
    let none = Json::obj();
    let ut = match prm.get("user_travel") {
        None => &none,
        Some(v @ Json::Obj(_)) => v,
        _ => return Err(()),
    };
    let (ut_pamphlet, ut_slot, ut_goal) = (int(ut, "m_travel_pamphlet_id")?, int(ut, "slot")?, int(ut, "is_goal")?);
    let ut_fields = [int(ut, "character_id")?, int(ut, "m_card_memorial_id")?, int(ut, "last_landmark")?];
    let positions = match ut.get("positions") {
        None => vec![0, 0, 0],
        Some(_) => ints(ut, "positions")?,
    };
    let mut cards = Vec::new();
    for c in objs(prm, "get_memorial_cards")? {
        cards.push((6, int(c, "memorial_card_id")?, int(c, "location")?));
    }
    for c in objs(prm, "get_skill_cards")? {
        cards.push((2, int(c, "skill_card_id")?, int(c, "location")?));
    }
    for h in objs(prm, "travel_history")? {
        int(h, "create_type")?;
    }
    let gachas = objs(prm, "lot_gachas")?;
    if p.card.is_none() {
        return Ok(travel_result_empty());
    }
    let st = &mut p.state;
    let l = list(st, "travel_pamphlets");
    for id in released {
        if !has(l, "m_travel_pamphlet_id", id) {
            l.push(pamphlet(id, true));
        }
    }
    let l = list(st, "travel_talks");
    for [talk, me, other] in talks {
        if !l.iter().any(|t| t.i("talk_id") == talk && t.i("my_character_id") == me && t.i("other_character_id") == other) {
            l.push(rec(&[("talk_id", n(talk)), ("my_character_id", n(me)), ("other_character_id", n(other))]));
        }
    }
    let l = list(st, "badges");
    for id in badges {
        if !has(l, "m_badge_id", id) {
            l.push(plate("m_badge_id", id, true));
        }
    }
    let l = list(st, "card_frames");
    for id in frames {
        if !has(l, "m_card_frame_id", id) {
            l.push(unlockable("m_card_frame_id", id, false, true));
        }
    }
    let l = list(st, "nameplates");
    for id in nameplates {
        if !has(l, "m_nameplate_id", id) {
            l.push(plate("m_nameplate_id", id, true));
        }
    }
    let l = list(st, "item");
    for (id, count) in items {
        find_or_add(l, "m_item_id", id, || rec(&[("m_item_id", n(id)), ("count", n(0))])).set("count", count);
    }
    // the specials (crackers) won, the last 3 of each group kept
    let kind = st.get("userdata").map_or(0, |u| u.i("idol_kind"));
    let l = list(st, "specials");
    l.extend(specials.iter().map(|&id| rec(&[("idol_kind", n(kind)), ("special_id", n(id))])));
    for k in 0..3 {
        let mut extra = l.iter().filter(|s| s.i("idol_kind") == k).count().saturating_sub(3);
        l.retain(|s| {
            let drop = extra > 0 && s.i("idol_kind") == k;
            extra -= usize::from(drop);
            !drop
        });
    }
    let ud = object(st, "userdata");
    ud.set("level", level);
    ud.set("total_exp", total_exp);
    let l = list(st, "members");
    for (id, points) in yells {
        find_or_add(l, "character_id", id, || member(id, true)).set("yell_point", points);
    }
    let pm = find_or_add(list(st, "travel_pamphlets"), "m_travel_pamphlet_id", ut_pamphlet, || pamphlet(ut_pamphlet, false));
    inc(pm, "total_dice_count", dice);
    inc(pm, "total_talk_count", talk_count);
    pm.set("is_new", false);
    if ut_goal == 1 {
        inc(pm, "round", 1);
    }
    let t = find_or_add(list(st, "travels"), "slot", ut_slot, || travel(ut_slot));
    t.set("m_travel_pamphlet_id", ut_pamphlet);
    for (k, v) in ["character_id", "m_card_memorial_id", "last_landmark"].into_iter().zip(ut_fields) {
        t.set(k, v);
    }
    t.set("positions", ints_json(&positions));
    t.set("modified", format_time(local_now(), " ", false));
    // the cards won: to the mailbox (ids from 1)
    let mailbox = list(st, "mailbox");
    let mut got = Vec::new();
    for (category, item, location) in cards {
        let id = (mailbox.len() + 1).to_string();
        got.push(rec(&[("mailbox_id", id.as_str().into()), ("location", n(location))]));
        mailbox.push(rec(&[("id", id.into()), ("attrib", n(0)), ("category", n(category)), ("item_id", n(item)), ("count", n(1))]));
    }
    if !gachas.is_empty() {
        log!("lovelive: travel gachas {}: no cards (no gacha tables)", Json::Arr(gachas.to_vec()));
    }
    p.dirty = true;
    Ok(rec(&[
        ("get_card_datas", Json::Arr(got)),
        ("travel_history_ids", Json::Arr(Vec::new())),
        ("mailbox", Json::Arr(p.state.a("mailbox").to_vec())),
    ]))
}

fn travel_snap_print(p: &mut Player, _prm: &Json) -> Reply {
    if p.card.is_none() {
        return Ok(travel_result_empty());
    }
    Ok(rec(&[("travel_snap_id", "000102030405060708090a0b0c0d0e0f".into()), ("created", format_time(local_now(), "", true).into())]))
}

fn achievement(p: &mut Player, prm: &Json) -> Reply {
    let (achievements, limited) = (ints(prm, "achievements")?, ints(prm, "limited_achievements")?);
    let mut books = Vec::new();
    for r in objs(prm, "record_books")? {
        books.push((text(r, "type")?, ints(r, "values")?));
    }
    if p.card.is_none() {
        return Ok(Json::obj());
    }
    let st = &mut p.state;
    // a record book known: its values replaced (the game's must be as many)
    for (kind, values) in &books {
        if let Some(old) = st.a("record_books").iter().find(|r| r.s("type") == kind)
            && values.len() < old.a("values").len()
        {
            return Err(());
        }
    }
    for (list_key, key, ids) in [("achievements", "m_achievement_id", achievements), ("limited_achievements", "m_limited_achievement_id", limited)] {
        let l = list(st, list_key);
        for id in ids {
            let e = find_or_add(l, key, id, || unlockable(key, id, true, true));
            e.set("unlocked", true);
            e.set("new", true);
        }
    }
    let l = list(st, "record_books");
    for (kind, values) in books {
        match l.iter_mut().find(|r| r.s("type") == kind) {
            Some(r) => {
                let len = r.a("values").len();
                r.set("values", ints_json(&values[..len]));
            }
            None => l.push(rec(&[("type", kind.into()), ("values", ints_json(&values))])),
        }
    }
    p.dirty = true;
    let st = &p.state;
    Ok(rec(&[
        ("achievements", Json::Arr(st.a("achievements").to_vec())),
        ("record_books", Json::Arr(st.a("record_books").to_vec())),
        ("honors", Json::Arr(st.a("honors").to_vec())),
        ("stages", Json::Arr(Vec::new())),
        ("item", Json::Arr(st.a("item").to_vec())),
    ]))
}

fn achievement_yell(p: &mut Player, prm: &Json) -> Reply {
    let mut ranks = Vec::new();
    for m in objs(prm, "member_yell")? {
        ranks.push((int(m, "character_id")?, int(m, "yell_achieve_rank")?));
    }
    int(prm, "small_reward_count")?;
    let yells = ints(prm, "yell_achievements")?;
    if p.card.is_none() {
        return Ok(Json::obj());
    }
    // a yell rank's member cards (rank 20, 80), when achievements came with it
    let mut rewards = Vec::new();
    if !yells.is_empty() {
        for &(id, rank) in &ranks {
            for (min, table) in [(20, &YELL_RANK_20_CARDS), (80, &YELL_RANK_80_CARDS)] {
                if rank >= min {
                    let card = *usize::try_from(id).ok().and_then(|i| table.get(i)).ok_or(())?;
                    if card != 0 {
                        rewards.push(card);
                    }
                }
            }
        }
    }
    let st = &mut p.state;
    let l = list(st, "members");
    for (id, rank) in ranks {
        find_or_add(l, "character_id", id, || member(id, true)).set("yell_achieve_rank", rank);
    }
    let l = list(st, "yell_achievements");
    for id in yells {
        if !has(l, "m_yell_achievement_id", id) {
            l.push(unlockable("m_yell_achievement_id", id, true, true));
        }
    }
    let l = list(st, "membercard");
    for card in rewards {
        if !has(l, "m_card_member_id", card) {
            l.push(member_card(card, 1, true));
        }
    }
    p.dirty = true;
    let st = &p.state;
    Ok(rec(&[
        ("yell_achievements", Json::Arr(st.a("yell_achievements").to_vec())),
        ("membercard", Json::Arr(st.a("membercard").to_vec())),
        ("honors", Json::Arr(st.a("honors").to_vec())),
        ("stages", Json::Arr(Vec::new())),
        ("item", Json::Arr(st.a("item").to_vec())),
    ]))
}

/// The member gacha's box: LLServer's fixed nine cards.
fn member_gacha(_p: &mut Player, prm: &Json) -> Reply {
    let id: i32 = prm.get("gacha_id").and_then(Json::as_str).ok_or(())?.trim().parse().map_err(|_| ())?;
    let cards = (1..=9)
        .map(|i| rec(&[("category", n(1)), ("character_id", n(7)), ("item_id", n(40001 + 10 * i)), ("level", n(1)), ("enable", n(1))]))
        .collect();
    Ok(rec(&[("m_gacha_id", n(id.into())), ("last_crackers", Json::Arr(Vec::new())), ("box", Json::Arr(cards))]))
}
