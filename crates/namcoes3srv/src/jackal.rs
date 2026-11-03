//! Pokkén Tournament's game server (`jackal.protobuf`, the Graphene framework of `graphene.rs`),
//! its matching server and its admission websocket. They follow ARTEMiS's Pokkén title (WTFPL,
//! https://gitea.tendokyu.moe/Hay1tsme/artemis, `titles/pokken`): its answers, what it keeps of
//! a player and how a play is added to it.
//!
//! A player is kept as the game's own `LoadUserResponseData` (encoded with its schema) in
//! `<data>/users/<banapass_id>`, its card's chip id in `<banapass_id>.chip`, the rewards it won
//! in `<banapass_id>.rewards` (`category content type` lines).
//!
//! * RegisterPcb: the cabinet's serial must be ARTEMiS's (2747, region 1, role 3, cabinet 19:
//!   `27471319....`); the answer is the server time, the Banapassport site and the matching
//!   library's settings (matching server, STUN / TURN, admission websocket, all this server).
//! * LoadUser: the card's player, made at its first use (title, comment and support sets of
//!   ARTEMiS); a new card (no trainer name yet) has the game ask for one.
//! * SaveUser: the trainer name chosen, the rank points, wallet (Pokémon gold) and score added,
//!   the flags and counters kept, the support sets, the Pokémon played (its experience added, its
//!   battle points grown), its battles per mode, the KO counts added, the rewards kept.
//! * LoadClientSettings, LoadRanking: ARTEMiS's settings (every Pokémon open, level cap 100), an
//!   empty ranking. The others: done.
//! * Matching (`POST .../matching`, JSON `{"call": ..., "data": ...}`): `isMatching` answers the
//!   cabinet alone (no opponent), the others an empty `data`. Admission (websocket `.../admission`,
//!   JSON `{"api": ...}`): `JoinSession` gets a session id, the others an empty `data`.

use std::path::{Path, PathBuf};

use crate::graphene::{Obj, Schema, V, get, leak, now, string, uint};
use crate::log;

/// A request's result: done, refused.
const OK: u64 = 1;
const REFUSED: u64 = 0;
/// The player record's message.
const PROFILE: &str = "LoadUserResponseData";
/// The cabinet serials the server registers (ARTEMiS's: 2747, region 1, role 3, cabinet 19).
const SERIAL_PREFIX: &str = "27471319";
/// The session of a lone cabinet (matching, admission).
const SESSION_ID: u64 = 12345678;

/// SaveUser's battle modes (`battle_data.play_mode`) and result (`battle_data.result`).
const MODE_TUTORIAL: u64 = 1;
const MODE_CPU: u64 = 2;
const MODE_LAN: u64 = 3;
const MODE_WAN: u64 = 4;
const WIN: u64 = 1;

fn set(obj: &mut Obj, name: &str, value: V) {
    match obj.iter_mut().find(|(n, _)| *n == name) {
        Some((_, v)) => *v = value,
        None => obj.push((leak(name), value)),
    }
}

fn add(obj: &mut Obj, name: &str, delta: i64) {
    let v = (uint(obj, name) as i64 + delta).clamp(0, u32::MAX as i64);
    set(obj, name, V::U(v as u64));
}

/// A field of the request copied to the record when the game sent it.
fn keep(p: &mut Obj, req: &Obj, from: &str, to: &str) {
    if get(req, from).is_some() {
        set(p, to, V::U(uint(req, from)));
    }
}

fn ints(obj: &Obj, name: &str) -> Vec<u64> {
    match get(obj, name) {
        Some(V::Ints(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn message(obj: &Obj, name: &str) -> Obj {
    match get(obj, name) {
        Some(V::M(m)) => m.clone(),
        _ => Vec::new(),
    }
}

/// An `int32` field (negative values travel as 64-bit varints).
fn int32(obj: &Obj, name: &str) -> i64 {
    uint(obj, name) as i64 as i32 as i64
}

fn users(dir: &Path) -> PathBuf {
    dir.join("users")
}

fn load(schema: &Schema, dir: &Path, id: u64) -> Option<Obj> {
    let data = std::fs::read(users(dir).join(id.to_string())).ok()?;
    Some(schema.decode(PROFILE, &data))
}

fn save(schema: &Schema, dir: &Path, id: u64, profile: &Obj) {
    let path = users(dir).join(id.to_string());
    if let Err(e) = std::fs::write(&path, schema.encode(PROFILE, profile)) {
        log(&format!("jackal: {}: {e}", path.display()));
    }
}

fn ids(dir: &Path) -> Vec<u64> {
    let mut ids: Vec<u64> = std::fs::read_dir(users(dir)).map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default();
    ids.sort();
    ids
}

/// The player of the card `access_code`.
fn find(schema: &Schema, dir: &Path, access_code: &str) -> Option<(u64, Obj)> {
    if access_code.is_empty() {
        return None;
    }
    ids(dir).into_iter().find_map(|id| load(schema, dir, id).filter(|p| string(p, "access_code") == access_code).map(|p| (id, p)))
}

/// A new card's player: ARTEMiS's defaults, no trainer name yet.
fn create(id: u64, access_code: &str) -> Obj {
    vec![
        ("banapass_id", V::U(id)),
        ("access_code", V::S(access_code.to_string())),
        ("navi_newbie_flag", V::U(1)),
        ("navi_enable_flag", V::U(1)),
        ("pad_vibrate_flag", V::U(1)),
        ("title_text_id", V::U(2)),
        ("title_plate_id", V::U(31)),
        ("title_decoration_id", V::U(1)),
        ("comment_text_id", V::U(1)),
        ("comment_word_id", V::U(1)),
        ("site_register_status", V::U(1)),
        ("avatar_gender", V::U(1)),
        ("support_set_1", V::Ints(vec![587, 653])),
        ("support_set_2", V::Ints(vec![495, 131])),
        ("support_set_3", V::Ints(vec![657, 133])),
    ]
}

/// The card's chip id: its access code.
fn by_chip(schema: &Schema, dir: &Path, chip: &str) -> Option<String> {
    if chip.is_empty() {
        return None;
    }
    ids(dir).into_iter().find_map(|id| {
        let saved = std::fs::read_to_string(users(dir).join(format!("{id}.chip"))).ok()?;
        (saved.trim() == chip).then(|| load(schema, dir, id).map(|p| string(&p, "access_code")))?
    })
}

fn load_user(schema: &Schema, dir: &Path, name: &str, req: &Obj) -> Obj {
    let code = string(req, "access_code");
    let (id, mut profile) = match find(schema, dir, &code) {
        Some(found) => found,
        None => {
            let id = ids(dir).last().map_or(1, |l| l + 1);
            let p = create(id, &code);
            save(schema, dir, id, &p);
            log(&format!("jackal: new player {id} for card {code}"));
            (id, p)
        }
    };
    let chip = string(req, "chip_id");
    if !chip.is_empty() {
        let _ = std::fs::write(users(dir).join(format!("{id}.chip")), &chip);
    }
    // A card whose trainer has no name yet is registered here with the profile's name: the
    // game's own registration (new card flag) asks for it on a touch keyboard the cabinet
    // controller cannot type on, and a card left new is never saved.
    if string(&profile, "trainer_name").is_empty() {
        set(&mut profile, "trainer_name", V::S(name.to_string()));
        save(schema, dir, id, &profile);
        log(&format!("jackal: player {id} registered as {name}"));
    }
    set(&mut profile, "commidserv_result", V::U(1));
    set(&mut profile, "load_hash", V::U(1));
    set(&mut profile, "cardlock_status", V::U(0));
    set(&mut profile, "banapass_id", V::U(id));
    set(&mut profile, "new_card_flag", V::U(0));
    set(&mut profile, "precedent_release_flag", V::U(u32::MAX as u64));
    profile
}

/// SaveUser: the play added to the player, as ARTEMiS does.
fn save_user(schema: &Schema, dir: &Path, req: &Obj) {
    let id = uint(req, "banapass_id");
    let Some(mut p) = load(schema, dir, id) else {
        log(&format!("jackal: SaveUser of unknown player {id}"));
        return;
    };
    let pending = string(req, "trainer_name_pending");
    if !pending.is_empty() {
        set(&mut p, "trainer_name", V::S(pending));
        keep(&mut p, req, "avatar_gender", "avatar_gender");
    }
    for name in ["tutorial_progress_flag", "achievement_flag", "event_achievement_flag", "event_achievement_param"] {
        set(&mut p, name, V::Ints(ints(req, name)));
    }
    keep(&mut p, req, "event_state", "event_state");
    keep(&mut p, req, "last_play_event_id", "last_play_event_id");

    // the rewards won (titles, plates, decorations, ...)
    let rewards_path = users(dir).join(format!("{id}.rewards"));
    let mut rewards = std::fs::read_to_string(&rewards_path).unwrap_or_default();
    if let Some(V::List(list)) = get(req, "reward_data") {
        for r in list {
            let line = format!("{} {} {}", uint(r, "get_category_id"), uint(r, "get_content_id"), uint(r, "get_type_id"));
            if !rewards.lines().any(|l| l == line) {
                rewards.push_str(&line);
                rewards.push('\n');
            }
        }
        if let Err(e) = std::fs::write(&rewards_path, &rewards) {
            log(&format!("jackal: {}: {e}", rewards_path.display()));
        }
    }

    add(&mut p, "trainer_rank_point", int32(req, "get_trainer_rank_point"));
    add(&mut p, "wallet", uint(req, "get_money") as i64);
    add(&mut p, "score_point", uint(req, "get_score_point") as i64);
    set(&mut p, "grade_max_num", V::U(uint(req, "grade_max_num")));

    // the support sets (two Pokémon each)
    for name in ["support_set_1", "support_set_2", "support_set_3"] {
        let set_ = ints(req, name);
        if set_.len() >= 2 {
            set(&mut p, name, V::Ints(set_[..2].to_vec()));
        }
    }

    // the Pokémon played, by its illustration book number: new with its battle points + 1,
    // else grown by them; its experience added
    let mon = message(req, "pokemon_data");
    let book = uint(&mon, "illustration_book_no");
    let mut list = match get(&p, "pokemon_data") {
        Some(V::List(l)) => l.clone(),
        _ => Vec::new(),
    };
    let at = match list.iter().position(|m| uint(m, "illustration_book_no") == book) {
        Some(i) => {
            for f in ["bp_point_atk", "bp_point_res", "bp_point_def", "bp_point_sp"] {
                add(&mut list[i], f, uint(&mon, f) as i64);
            }
            i
        }
        None => {
            let mut entry: Obj = vec![("illustration_book_no", V::U(book))];
            for f in ["bp_point_atk", "bp_point_res", "bp_point_def", "bp_point_sp"] {
                set(&mut entry, f, V::U(1 + uint(&mon, f)));
            }
            list.push(entry);
            list.len() - 1
        }
    };
    set(&mut list[at], "char_id", V::U(uint(&mon, "char_id")));
    add(&mut list[at], "pokemon_exp", uint(&mon, "get_pokemon_exp") as i64);
    set(&mut p, "latest_use_pokemon", V::U(book));

    // its battles: tutorial, CPU, in-store (LAN), online (WAN)
    let battle = message(req, "battle_data");
    let results = ints(&battle, "result");
    for (i, mode) in ints(&battle, "play_mode").into_iter().enumerate() {
        let won = results.get(i) == Some(&WIN);
        let entry = &mut list[at];
        match mode {
            MODE_TUTORIAL => {
                add(entry, "battle_num_tutorial", 1);
                add(entry, "battle_all_num_tutorial", 1);
            }
            MODE_CPU => {
                add(entry, "battle_num_vs_cpu", 1);
                add(entry, "win_cpu", won as i64);
            }
            MODE_LAN => {
                add(entry, "battle_num_vs_lan", 1);
                add(entry, "win_vs_lan", won as i64);
            }
            MODE_WAN => {
                add(entry, "battle_num_vs_wan", 1);
                add(entry, "win_vs_wan", won as i64);
            }
            _ => {}
        }
    }
    set(&mut p, "pokemon_data", V::List(list));

    for f in ["ex_ko_num", "wko_num", "timeup_win_num", "cool_ko_num", "perfect_ko_num"] {
        add(&mut p, f, uint(&battle, f) as i64);
    }
    set(&mut p, "continue_num", V::U(uint(req, "continue_num")));
    set(&mut p, "extra_counter", V::U(uint(req, "extra_counter")));
    set(&mut p, "event_reward_get_flag", V::U(uint(req, "event_reward_get_flag")));
    add(&mut p, "total_play_days", uint(req, "total_play_days") as i64);
    add(&mut p, "awake_num", uint(req, "awake_num") as i64);
    add(&mut p, "use_support_num", uint(req, "use_support_num") as i64);
    set(&mut p, "beat_num", V::U(int32(req, "beat_num") as u32 as u64));
    set(&mut p, "aid_skill", V::U(uint(req, "aid_skill")));
    set(&mut p, "play_date_time", V::U(now()));
    save(schema, dir, id, &p);
    log(&format!("jackal: player {id} saved"));
}

/// The matching library's settings (RegisterPcb `biwa_setting`): everything on this server.
fn biwa_setting(addr: &str) -> String {
    let (host, port) = addr.rsplit_once(':').unwrap_or((addr, "9002"));
    format!(
        r#"{{"MatchingServer":{{"host":"https://{host}","port":{port},"url":"/pokken/matching"}},"StunServer":{{"addr":"{host}","port":3333}},"TurnServer":{{"addr":"{host}","port":3333}},"AdmissionUrl":"wss://{host}:{port}/pokken/admission","locationId":0,"logfilename":"J:\\JackalMatchingLibrary.log","biwalogfilename":"J:\\biwa_log.log"}}"#
    )
}

/// The answer to one request: (result, the answer's fields).
pub(crate) fn answer(schema: &Schema, dir: &Path, name: &str, addr: &str, kind: &str, req: &Obj) -> (u64, Obj) {
    match kind {
        "REGISTER_PCB" => {
            let pcb = string(req, "pcb_id");
            if pcb.len() != 12 || !pcb.bytes().all(|b| b.is_ascii_digit()) || !pcb.starts_with(SERIAL_PREFIX) {
                log(&format!("jackal: cabinet {pcb:?} refused (not a {SERIAL_PREFIX}xxxx serial)"));
                return (REFUSED, Vec::new());
            }
            (
                OK,
                vec![
                    ("server_time", V::U(now())),
                    ("bnp_baseuri", V::S(format!("{addr}/bna"))),
                    ("biwa_setting", V::S(biwa_setting(addr))),
                ],
            )
        }
        "CHECK_ACCESS_CODE" => {
            let code = by_chip(schema, dir, &string(req, "chip_id")).unwrap_or_default();
            (OK, vec![("commidserv_result", V::U(1)), ("access_code", V::S(code))])
        }
        "LOAD_USER" => (OK, load_user(schema, dir, name, req)),
        "SAVE_USER" => {
            save_user(schema, dir, req);
            (OK, Vec::new())
        }
        "LOAD_CLIENT_SETTINGS" => (
            OK,
            vec![
                ("money_magnification", V::U(1)),
                ("continue_bonus_exp", V::U(100)),
                ("continue_fight_money", V::U(100)),
                ("event_bonus_exp", V::U(100)),
                ("level_cap", V::U(100)),
                ("op_movie_flag", V::U(u32::MAX as u64)),
                ("lucky_bonus_rate", V::U(1)),
                ("fail_support_num", V::U(10)),
                // every Pokémon open, since long ago
                ("chara_open_flag", V::U(u32::MAX as u64)),
                ("chara_open_date", V::U(now() / 1000)),
                ("chara_pre_open_date", V::U(now() / 1000)),
                ("search_id", V::U(123)),
            ],
        ),
        "LOAD_RANKING" => (
            OK,
            vec![
                ("ranking_id", V::U(1)),
                ("ranking_start", V::U(0)),
                ("ranking_end", V::U(1)),
                ("event_end", V::U(1)),
                ("modify_date", V::U(now() / 1000)),
            ],
        ),
        "CHECK_RANKING" => (OK, vec![("ranking_id", V::U(uint(req, "ranking_id")))]),
        "SAVE_CHARGE" => (OK, vec![("charge_error_code", V::U(0)), ("charge_data_index", V::S(string(req, "charge_data_index")))]),
        _ => (OK, Vec::new()),
    }
}

/// A JSON string value: the first `"key": "value"` (the game's JSON has trailing commas, read
/// by search, not parsed).
fn json_str(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

fn millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// A matching server request (`{"call": ..., "data": {"must": {"pcb_id": ...}}}`).
pub(crate) fn matching(body: &[u8]) -> Vec<u8> {
    let json = String::from_utf8_lossy(body);
    let call = json_str(&json, "call").unwrap_or_default();
    log(&format!("jackal: matching {call}"));
    let data = match call.as_str() {
        // the cabinet alone: no opponent
        "isMatching" => {
            let pcb = json_str(&json, "pcb_id").unwrap_or_default();
            format!(r#"{{"sessionId":"{SESSION_ID}","A":{{"pcb_id":"{pcb}","gip":"127.0.0.1"}},"list":[]}}"#)
        }
        _ => "{}".to_string(),
    };
    format!(r#"{{"result":"true","data":{data},"timestamp":{}}}"#, millis()).into_bytes()
}

/// An admission websocket message (`{"api": ...}`).
pub(crate) fn admission(text: &str) -> String {
    let api = json_str(text, "api").unwrap_or_else(|| "noop".into());
    log(&format!("jackal: admission {api}"));
    let data = if api.eq_ignore_ascii_case("JoinSession") { format!(r#"{{"id":{SESSION_ID}}}"#) } else { "{}".to_string() };
    format!(r#"{{"type":"res","data":{data},"api":"{api}","result":"true"}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_answers_alone() {
        let res = String::from_utf8(matching(br#"{"call":"isMatching","data":{"must":{"pcb_id":"274713190001",},},}"#)).unwrap();
        assert!(res.contains(r#""pcb_id":"274713190001""#) && res.contains(r#""list":[]"#), "{res}");
        assert!(admission(r#"{"api":"JoinSession"}"#).contains(r#""id":12345678"#));
        assert_eq!(json_str(r#"{ "call" : "stopMatching" }"#, "call").as_deref(), Some("stopMatching"));
    }
}
