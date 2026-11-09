//! Chrono Regalia's game server (the Aime card players' data): the game (`HkbCom.ComIo`) posts
//! `{"commands":[{"commandId":n,"data":"<json>"}]}` to `<ALL.Net uri>/hkb/<place id hex>/
//! <keychip>/<model type>/<client version>/<unix ms>`, each command's request a JSON string, and
//! reads the answers in the same order, each command's response a JSON string too (Unity's
//! `JsonUtility`: absent fields keep their defaults). The ALL.Net uri is this server,
//! `http://{gs}` in the profile's `WAL_ALLS_TITLE_URI` (127.0.0.1:`WAL_PORT`+104: the game's
//! Mono sockets are not in the game module's imports, the router's redirection never sees
//! them).
//!
//! No server of the game was published: the answers follow the client's own use of them
//! (`Assembly-CSharp`'s `HkbCom/Com*.cs`, `Login.cs`, `PlayerParam.cs`). The players are kept in
//! `Y:\hkb\` (`user.rs`), a new player starting with the guest's characters, decks and cards
//! (the game's `guest_user.csv`); the shops' products are the game's own master files
//! (`LocalOptionData/Params/game`). No other players: rankings, replays, matching and events
//! are empty.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

use super::user;

/// The commands' names by id (`ComXxx.m_commandId`, `m_command`).
const COMMANDS: &[(i64, &str)] = &[
    (10010000, "upload_book_keep"),
    (10010100, "keep_alive"),
    (10010200, "save_machine_info"),
    (10010201, "save_package_info"),
    (10010300, "get_product_shop_card"),
    (10010303, "get_product_shop_customize"),
    (10010304, "get_product_shop_chrorega_coin"),
    (10010400, "get_stamp_info"),
    (10010500, "get_ranking_player_national"),
    (10010501, "get_ranking_player_region"),
    (10010502, "get_ranking_player_place"),
    (10019900, "get_server_info"),
    (10020000, "user_keep_alive"),
    (10020100, "aime_to_user"),
    (10020200, "simple_login"),
    (10020201, "simple_logout"),
    (10020300, "get_user_info"),
    (10020301, "get_user_chronos"),
    (10020303, "get_user_character"),
    (10020306, "get_user_party"),
    (10020307, "get_user_card_set"),
    (10020308, "get_user_card"),
    (10020309, "get_user_material"),
    (10020310, "get_user_ticket"),
    (10020311, "get_user_chrorega_coin"),
    (10020312, "get_user_purchase_shop_card"),
    (10020313, "get_user_purchase_shop_customize"),
    (10020314, "get_user_stamp"),
    (10020400, "purchase_shop_card_begin"),
    (10020401, "purchase_shop_card_commit"),
    (10020402, "purchase_shop_card_retry"),
    (10020403, "purchase_shop_card_for_free"),
    (10020404, "purchase_shop_card_for_ticket"),
    (10029000, "upload_user_play_log"),
    (10030000, "login_to_terminal"),
    (10030001, "logout_from_terminal"),
    (10040000, "login_to_satellite"),
    (10040001, "logout_from_satellite"),
    (10040100, "regist_user"),
    (10040101, "complete_tutorial"),
    (10040200, "save_user_active_party"),
    (10040201, "save_user_codename"),
    (10040202, "save_user_draw_type"),
    (10040300, "save_user_party"),
    (10040301, "save_user_card_set"),
    (10040401, "decompose_card"),
    (10040402, "purchase_shop_customize"),
    (10040403, "equip_chronos_item"),
    (10040404, "equip_character_item"),
    (10040500, "purchase_shop_chrorega_coin_begin"),
    (10040501, "purchase_shop_chrorega_coin_commit"),
    (10040502, "purchase_shop_chrorega_coin_retry"),
    (10040600, "recovery_cp_begin"),
    (10040601, "recovery_cp_commit"),
    (10040602, "recovery_cp_retry"),
    (10040603, "consume_cp"),
    (10040700, "get_user_notification"),
    (10040701, "reply_to_notification"),
    (10040703, "reply_to_add_character_notification"),
    (10040704, "reply_to_add_codename_notification"),
    (10040705, "reply_to_add_card_notification"),
    (10040706, "reply_to_add_material_notification"),
    (10040707, "reply_to_add_ticket_notification"),
    (10040709, "reply_to_release_draw_type_notification"),
    (10040800, "get_user_mission_normal"),
    (10040801, "get_user_mission_daily"),
    (10040802, "get_user_mission_event"),
    (10040803, "get_user_mission_reward"),
    (10040804, "get_user_expire_mission_reward"),
    (10040900, "push_stamp"),
    (10049000, "check_ng_word"),
    (10050000, "save_user_battle_result_tutorial"),
    (10050100, "get_user_battle_result_national"),
    (10050101, "save_user_battle_result_national"),
    (10050102, "confirm_chance_battle"),
    (10050200, "get_user_battle_result_place"),
    (10050201, "save_user_battle_result_place"),
    (10050300, "get_user_battle_result_friend"),
    (10050301, "save_user_battle_result_friend"),
    (10050400, "get_user_battle_result_com"),
    (10050401, "save_user_battle_result_com"),
    (10050500, "get_user_battle_result_story"),
    (10050501, "save_user_battle_result_story"),
    (10050600, "get_user_battle_result_event"),
    (10050601, "save_user_battle_result_event"),
    (10050700, "get_user_battle_result_place_event"),
    (10050701, "save_user_battle_result_place_event"),
    (10059000, "get_user_battle_latest_result"),
    (10059001, "get_battle_reward"),
    (10059002, "save_user_battle_mission_achievement"),
    (10059003, "save_user_battle_info"),
    (10059100, "confirm_upload_ranker_replay"),
    (10059101, "upload_replay"),
    (10059900, "upload_battle_log"),
    (11010600, "get_ranker_replay"),
    (11010601, "download_ranker_replay"),
    (11010700, "get_battle_event"),
    (11059200, "send_matching_info"),
    (12010301, "get_product_generate_card"),
    (12010800, "get_campaign_login"),
    (12019800, "get_emoney_setting"),
    (12020315, "get_user_navi_chara"),
    (12020405, "purchase_shop_card_for_select_summon"),
    (12030100, "get_user_replay"),
    (12030101, "download_user_replay"),
    (12040400, "generate_card"),
    (12040405, "save_user_active_navi_chara"),
    (12040710, "create_user_system_notification"),
    (12050800, "get_user_battle_result_explore"),
    (12050801, "save_user_battle_result_explore"),
    (13010503, "get_ranking_player_total_score"),
    (13010504, "get_ranking_player_event_high_score"),
    (13010701, "get_reward_period_total_score"),
    (13010900, "get_schedule_advertise"),
    (13010901, "get_schedule_shop"),
    (13019801, "get_forbid_card"),
    (13040711, "reply_to_reward_event_high_score"),
    (13050900, "get_user_battle_period_total_score"),
    (13050901, "save_user_battle_period_total_score"),
];

fn name(id: i64) -> &'static str {
    COMMANDS.iter().find(|(i, _)| *i == id).map_or("", |(_, n)| n)
}

/// One request's context: the client version (the URL's), the request.
pub(super) struct Ctx<'a> {
    pub(super) version: i64,
    pub(super) req: &'a Json,
}

fn empty() -> Json {
    Json::obj()
}

fn list(key: &str) -> Json {
    Json::obj().with(key, Vec::<Json>::new())
}

/// The answers that need no player.
fn system(command: &str, ctx: &Ctx) -> Option<Json> {
    Some(match command {
        "keep_alive" | "save_machine_info" | "save_package_info" | "upload_book_keep" | "upload_user_play_log" | "upload_battle_log"
        | "upload_replay" | "send_matching_info" | "save_user_battle_info" | "create_user_system_notification" => empty(),
        "get_server_info" => list("serverInfp"),
        "get_emoney_setting" => Json::obj().with("upperLimitMoney", 0u32),
        "get_forbid_card" => list("forbidCardInfo"),
        "get_battle_event" => list("eventInfo"),
        "get_campaign_login" => list("campaignInfo"),
        "get_schedule_advertise" => list("advertiseInfo"),
        "get_schedule_shop" => list("shopInfo"),
        "get_stamp_info" => list("stampInfo"),
        "get_reward_period_total_score" => list("rewardInfo"),
        "get_ranker_replay" => list("replayInfo"),
        "get_user_replay" => list("replayInfo"),
        "download_ranker_replay" | "download_user_replay" => list("replayData"),
        "confirm_upload_ranker_replay" => Json::obj().with("uploadFlag", 0u32),
        "confirm_chance_battle" => Json::obj().with("chanceBattleFlag", 0u32),
        // 1: the name accepted (0: an NG word)
        "check_ng_word" => Json::obj().with("result", 1u32),
        "get_ranking_player_national" | "get_ranking_player_region" | "get_ranking_player_place" | "get_ranking_player_total_score"
        | "get_ranking_player_event_high_score" => {
            Json::obj().with("rankingDate", ctx.req.i("rankingDate")).with("rankingInfo", Vec::<Json>::new())
        }
        "get_product_shop_card" => super::master::shop_card(ctx.req.i("updateNum")),
        "get_product_shop_chrorega_coin" => {
            Json::obj().with("updateNum", ctx.req.i("updateNum")).with("productInfo", Vec::<Json>::new())
        }
        "get_product_shop_customize" => super::master::shop_customize(ctx.req.i("updateNum")),
        "get_product_generate_card" => super::master::generate_card(ctx.req.i("updateNum")),
        _ => return None,
    })
}

/// A command not known: an empty object, logged once.
fn unknown(id: i64, command: &str) -> Json {
    static SEEN: Mutex<Option<HashSet<i64>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert(id) {
        log!("hkb: command {id} {command} not answered");
    }
    empty()
}

fn answer(id: i64, ctx: &Ctx) -> Json {
    let command = name(id);
    system(command, ctx).or_else(|| user::reply(command, ctx)).unwrap_or_else(|| unknown(id, command))
}

/// The answers to a request's commands, in order.
fn reply(path: &str, body: &str) -> String {
    // /hkb/<place>/<keychip>/<model type>/<client version>/<unix ms>
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let version = parts.iter().position(|p| *p == "hkb").and_then(|i| parts.get(i + 4)).and_then(|v| v.parse().ok()).unwrap_or(0);
    let request = Json::parse(body).unwrap_or(Json::obj());
    let _lock = user::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for command in request.a("commands") {
        let id = command.i("commandId");
        let req = Json::parse(command.s("data")).unwrap_or(Json::obj());
        let data = answer(id, &Ctx { version, req: &req }).to_string();
        log!("hkb: {} {} -> {}", name(id), command.s("data").chars().take(300).collect::<String>(), data.chars().take(300).collect::<String>());
        out.push(Json::obj().with("commandId", id).with("data", data));
    }
    Json::obj().with("commands", out).to_string()
}

fn client(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut out = stream;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut length = 0usize;
    let mut expect = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.trim().eq_ignore_ascii_case("expect") {
                expect = value.trim().eq_ignore_ascii_case("100-continue");
            }
        }
    }
    // Mono's WebClient waits for this before the body (.NET's Expect100Continue)
    if expect {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let answer = reply(&path, &String::from_utf8_lossy(&body));
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", answer.len()).as_bytes())?;
    out.write_all(answer.as_bytes())
}

fn serve() {
    let port = crate::gs::port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            log!("hkb: game server on 127.0.0.1:{port}");
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = client(stream);
                });
            }
        }
        Err(e) => log!("hkb: 127.0.0.1:{port}: {e}"),
    }
}

pub(crate) fn init() {
    std::thread::spawn(serve);
}
