//! Initial D THE ARCADE's game server (the players' data: Aime card logins, profiles, cars,
//! records): the game posts JSON to `<ALL.Net uri>initiald/<category>/<endpoint>` (the uri
//! `http://<router>/SDGT/`), its connections to the router's port 80 going to this server in its
//! own process, 127.0.0.1:`WAL_PORT`+104 (`allnet.rs`). The answers follow ARTEMiS's IDAC title
//! server (WTFPL, gitea.tendokyu.moe/Dniel97/artemis, `titles/idac`: season 2 and 3
//! controllers, every version answered as season 3); the players are kept in `Y:\idac\` (one
//! JSON file per Aime user id, `idac/store.rs`).
//!
//! Requests carry the cabinet in an `application` header (`device_version`, `a_serial`,
//! `session`: the Aime user id after `Login/CheckLock`...); every answer has `status_code` "0"
//! (OK), datetimes in Unix seconds. `idac/user.rs`: the account and the modes' results,
//! `idac/factory.rs`: the garage, `idac/records.rs`: time trial rankings and theory rivals. No
//! teams (season 3's `Team/*` answer as for a player in none), no events (rounds, stamps, battle
//! gifts, time trial events: ARTEMiS's defaults, off), the game's own time release and avatar
//! gacha data (`timerelease_no` 0).

mod factory;
mod records;
mod store;
mod user;

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

/// The store name shown with the players (ALL.Net's place name).
const STORE_NAME: &str = "WindowsArcadeLoader";

/// One request.
struct Ctx<'a> {
    /// The game version, 3 digits (`230`).
    version: &'a str,
    /// The `application` header.
    app: &'a str,
    body: &'a Json,
    /// The `session` (the Aime user id).
    user: u64,
}

impl Ctx<'_> {
    fn header(&self, key: &str) -> &str {
        header_value(self.app, key)
    }

    /// A number header (`a_country`, `a_store`), as written.
    fn header_num(&self, key: &str) -> Json {
        let v = self.header(key);
        if !v.is_empty() && v.parse::<i64>().is_ok() { Json::Num(v.to_string()) } else { Json::from(0i64) }
    }
}

/// The request's `application` header (`key="value", ...`).
fn header_value<'a>(header: &'a str, key: &str) -> &'a str {
    header
        .split(", ")
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map_or("", |(_, v)| v.trim().trim_matches('"'))
}

fn now() -> u64 {
    crate::clock::unix_now()
}

fn ok() -> Json {
    Json::obj().with("status_code", "0")
}

/// The router's address (the game server's host in the domains the game is given).
fn host() -> String {
    let r = crate::netenv::router();
    format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3])
}

/// `Boot/GetConfigData`: the server's URLs and the season's settings (ARTEMiS's season 2/3
/// defaults; time release 0 = the game's own data).
fn config_data(version: &str) -> Json {
    let api = format!("http://{}/{version}/", host());
    let mut exp = Vec::new();
    for rank in 0..64u32 {
        let (winbase, losebase) = if rank > 10 { (110, -60) } else { (0, 0) };
        let winnum = match rank {
            0 => 0,
            1..=4 => 10,
            5..=7 => 20,
            8..=10 => 30,
            11..=15 => 50,
            16..=20 => 70,
            21..=25 => 90,
            26..=31 => 110,
            32..=38 => 130,
            39..=46 => 150,
            47..=55 => 170,
            _ => 190,
        };
        exp.push(Json::obj().with("online_rank_id", rank).with("winbase", winbase).with("losebase", losebase).with("winnum", winnum));
    }
    ok().with("free_continue_enable", 1u32)
        .with("free_continue_new", 1u32)
        .with("free_continue_play", 1u32)
        .with("difference_time_to_jp", 0u32)
        .with("asset_version", "1")
        .with("optional_version", "1")
        .with("disconnect_offset", 0u32)
        .with("boost_balance_version", "0")
        .with("time_release_number", "0")
        .with("play_stamp_enable", 1u32)
        .with("play_stamp_bonus_coin", 1u32)
        .with("gacha_chara_needs", 1u32)
        .with("both_win_system_control", 0u32)
        .with("subcard_system_congrol", 0u32)
        .with("server_maintenance_start_hour", 0u32)
        .with("server_maintenance_start_minutes", 0u32)
        .with("server_maintenance_end_hour", 0u32)
        .with("server_maintenance_end_minutes", 0u32)
        .with("domain_api_game", api.clone())
        .with("domain_matching", format!("{api}initiald-matching/"))
        .with("domain_echo1", format!("{}:20001", host()))
        .with("domain_echo2", format!("{}:20002", host()))
        .with("domain_ping", host())
        .with("battle_gift_event_master", Vec::new())
        .with("round_event", Vec::new())
        .with("last_round_event", Vec::new())
        .with("last_round_event_ranking", Vec::new())
        .with("round_event_exp", exp)
        .with("stamp_info", Vec::new())
        .with("timerelease_no", 0u32)
        .with("timerelease_avatar_gacha_no", 0u32)
        .with("takeover_reward", takeover_reward())
        .with("subcard_judge", subcard_judge())
        .with("special_promote", special_promote())
        .with("matching_id", 0u32)
        .with("matching_group", matching_group())
        // season 3's (2026-04-01)
        .with("timetrial_disp_date", 1775001600u64)
        .with("buy_car_need_cash", 5000u32)
        .with("time_extension_limit", 5u32)
        .with("collabo_id", 0u32)
        .with("driver_debut_end_date", 2177452800u64)
        .with("online_battle_param1", 0u32)
        .with("online_battle_param2", 0u32)
        .with("online_battle_param3", 0u32)
        .with("online_battle_param4", 0u32)
        .with("online_battle_param5", 0u32)
        .with("online_battle_param6", 2u32)
        .with("online_battle_param7", 0u32)
        .with("online_battle_param8", 3900u32)
        .with("theory_open_version", "2.00.00")
        .with("theory_close_version", "9.99.99")
        .with("number_lottery_data", number_lottery_data())
        // ARTEMiS's league rewards are off by default (`league_reward.enable`)
        .with("league_rewards", Vec::new())
        .with("league_flag", 0u32)
}

/// `RewardCategoryEnum` values the boot settings use.
const D_COIN: u32 = 1;
const CAR_DRESSUP_TOKEN: u32 = 3;
const AVATAR_TOKEN: u32 = 5;
const AVATAR_BODY: u32 = 15;
const TITLE: u32 = 24;
const FULLTUNE_TICKET: u32 = 25;
const FT_TOKEN_PIECES: u32 = 34;

/// The takeover rewards per previous game (new player, ID Zero mileage, ID Zero level, SWDC):
/// (reward_no, category, type).
fn takeover_reward() -> Vec<Json> {
    let rewards: [&[(u32, u32, u32)]; 4] = [
        &[(1, D_COIN, 5000), (2, FULLTUNE_TICKET, 1)],
        &[
            (1, AVATAR_BODY, 82),
            (1, AVATAR_BODY, 190),
            (2, CAR_DRESSUP_TOKEN, 2),
            (2, AVATAR_TOKEN, 2),
            (3, CAR_DRESSUP_TOKEN, 3),
            (3, AVATAR_TOKEN, 3),
            (4, CAR_DRESSUP_TOKEN, 5),
            (4, AVATAR_TOKEN, 5),
            (5, AVATAR_BODY, 104),
            (5, AVATAR_BODY, 212),
        ],
        &[(1, TITLE, 4052), (2, TITLE, 4053), (3, TITLE, 4054), (4, TITLE, 4055), (5, TITLE, 4056), (6, TITLE, 4057), (7, TITLE, 4058)],
        &[(1, AVATAR_BODY, 81), (1, AVATAR_BODY, 189), (2, FULLTUNE_TICKET, 1), (2, AVATAR_BODY, 103), (2, AVATAR_BODY, 211)],
    ];
    rewards
        .iter()
        .enumerate()
        .map(|(kind, list)| {
            let data: Vec<Json> = list.iter().map(|(no, cat, ty)| Json::obj().with("reward_no", *no).with("reward_category", *cat).with("reward_type", *ty)).collect();
            Json::obj().with("takeover_reward_type", kind).with("takeover_reward_data", data)
        })
        .collect()
}

/// (condition_id, lower_rank, higher_rank, condition_start, condition_end, calc).
fn subcard_judge() -> Vec<Json> {
    const JUDGE: [(u32, u32, u32, u32, u32, i32); 10] = [
        (1, 1, 7, 17, 20, 3),
        (2, 1, 15, 10, 10, 20),
        (3, 11, 15, 80, 89, 7),
        (3, 11, 15, 90, 100, 10),
        (3, 1, 15, 0, 40, -100),
        (1, 1, 15, 21, 24, 7),
        (1, 1, 15, 25, 25, 10),
        (3, 16, 20, 70, 89, 6),
        (3, 16, 20, 90, 100, 12),
        (3, 16, 20, 0, 50, -100),
    ];
    JUDGE
        .iter()
        .map(|(id, lo, hi, start, end, calc)| {
            Json::obj().with("condition_id", *id).with("lower_rank", *lo).with("higher_rank", *hi).with("condition_start", *start).with("condition_end", *end).with("calc", *calc)
        })
        .collect()
}

fn special_promote() -> Vec<Json> {
    [(50u32, 8u32), (200, 16), (255, 21)].iter().map(|(counter, rank)| Json::obj().with("counter", *counter).with("online_rank_id", *rank)).collect()
}

fn matching_group() -> Vec<Json> {
    [(1u32, 30u32), (2, 50), (3, 100)].iter().map(|(id, pct)| Json::obj().with("group_id", *id).with("group_percent", *pct)).collect()
}

/// The license plate lottery's winning numbers (none, 1111...9999: FT token pieces).
fn number_lottery_data() -> Vec<Json> {
    (0..10u32)
        .map(|n| {
            Json::obj()
                .with("m_number_lottery_win_number_no", 10 + n)
                .with("m_number_lottery_schedule_no", 1u32)
                .with("win_number", n * 1111)
                .with("reward_category", FT_TOKEN_PIECES)
                .with("reward_type", 1u32)
                .with("end_dt", 0u32)
        })
        .collect()
}

/// `Boot/getTimereleaseData`: nothing (`timerelease_no` 0: the game's own data), every menu on.
fn timerelease_data() -> Json {
    let menu = [
        "dressup_shop",
        "avatar_gacha",
        "car_dressup",
        "car_keyholder",
        "meter_customize",
        "aura_customize",
        "papercup_customize",
        "avatar_customize",
        "chat_stamp_customize",
        "mytitle_customize",
        "start_menu_customize",
        "name_change",
    ]
    .iter()
    .fold(Json::obj(), |m, k| m.with(k, 1u32));
    let mut out = ok().with("display_timerelease_id", "");
    for k in ["car_list", "course_list", "chapter_list", "annaunce_image", "vs_balance_car", "vs_balance_param4", "vs_balance_param5", "vs_balance_param6", "vs_balance_param7"] {
        out.set(k, Vec::new());
    }
    out.with("menu_info", vec![menu]).with("parts_list", Vec::new())
}

/// `Login/<endpoint>` (ARTEMiS's `login_controller.py`): any Aime id plays.
fn login(ctx: &Ctx, endpoint: &str) -> Option<Json> {
    Some(match endpoint {
        "checklock" => {
            let user = ctx.body.u("id");
            let profile = store::load(user).and_then(|d| d.get("profile").cloned());
            // a newer game's data is refused (lock_result 2)
            let digits = |v: &str| v.replace('.', "").chars().take(7).collect::<String>().parse::<u64>().unwrap_or(0);
            let saved = profile.as_ref().map_or("1.00.00".to_string(), |p| p.s("device_version").to_string());
            let client = ctx.header("device_version");
            let valid = saved.len() >= 7 && client.len() >= 7 && digits(client) >= digits(&saved);
            ok().with("lock_result", if valid { 1u32 } else { 2 })
                .with("lock_date", now())
                .with("daily_play", 1u32)
                .with("session", user.to_string())
                .with("shared_security_key", "a")
                .with("session_procseq", 1u32)
                .with("new_player", u32::from(profile.is_none()))
                .with("server_status", 1u32)
                .with("takeover_data", Vec::new())
        }
        "unlock" => ok().with("lock_result", 1u32),
        "relock" => ok().with("lock_result", 1u32).with("lock_date", now()),
        "guestplay" => ok().with("session", "-1").with("shared_security_key", "Guest").with("session_procseq", 0u32),
        _ => return None,
    })
}

/// `Team/<endpoint>` (season 3's `team_controller.py`) for a player in no team, no team to
/// join. A created team is answered but not kept.
fn team(ctx: &Ctx, endpoint: &str) -> Option<Json> {
    let league = || Json::obj().with("league_rank_id", 0u32).with("rank", 0u32).with("team_point", 0u32).with("point", 0u32).with("top_list", Vec::new());
    Some(match endpoint {
        "createteam" => {
            let name = store::load(ctx.user).and_then(|d| d.get("profile").map(|p| p.s("username").to_string())).unwrap_or_default();
            let info = Json::obj()
                .with("team_id", 0u32)
                .with("team_name", store::val(ctx.body, "team_name", ""))
                .with("role", 1u32)
                .with("league_rank_id", 0u32)
                .with("team_sticker_id", 1u32)
                .with("daily_bonus", 0u32)
                .with("weekly_bonus", 0u32)
                .with("order", 0u32)
                .with("country", ctx.header_num("a_country"))
                .with("store_name", STORE_NAME)
                .with("team_info_list", vec![Json::obj().with("username", name).with("action_id", 2u32)]);
            ok().with("team_info", info).with("league_info", league()).with("auto_invite_flag", store::val(ctx.body, "auto_invite_flag", 0i64))
        }
        "leftteam" => ok().with("left_status", 1u32),
        // JoinStatusEnum.INVALID_PASSWORD
        "prejointeam" => ok().with("join_status", 1u32),
        "jointeam" => ok().with("join_status", 1u32).with("league_info", league()).with("team_member_cars", Vec::new()),
        "getteammanagedata" => ok().with("member_data", Vec::new()).with("password", "").with("auto_invite_flag", 0u32),
        "kickmember" => ok().with("control_result", 1u32).with("team_point", 0u32),
        "changeleader" => ok().with("control_result", 1u32),
        "changepassword" => ok().with("control_result", 1u32).with("password", ""),
        "getteaminfodata" => ok().with("member_data", Vec::new()).with("past_league_data", Vec::new()),
        "getteamhomedata" => ok().with("team_member_cars", Vec::new()),
        "getstickerstock" => ok().with("sticker_stock", format!("{:0<256}", "0111111111111")),
        "updatestickerdata" => ok().with("control_result", 1u32).with("sticker_id", 0u32),
        "getrecommendteam" => ok().with("join_status", 0u32).with("recommend_team_list", Vec::new()),
        "updateteamoption" => ok().with("control_result", 1u32).with("auto_invite_flag", 0u32),
        _ => return None,
    })
}

/// `initiald-matching/<endpoint>` (ARTEMiS's `render_matching`): matched at once, ARTEMiS's
/// room on the router's matching port.
fn matching(endpoint: &str) -> Option<Json> {
    Some(match endpoint {
        "regist" => ok(),
        "status" => ok().with("host", host()).with("port", 20003u32).with("room_name", "INDTA").with("state", 1u32),
        _ => return None,
    })
}

/// An endpoint not known: OK, logged once.
fn unknown(category: &str, endpoint: &str) -> Json {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let key = format!("{category}/{endpoint}");
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.get_or_insert_with(HashSet::new).insert(key.clone()) {
        log!("idac: unknown endpoint {key}");
    }
    ok()
}

/// The answer to one request (`category`/`endpoint` lower case).
fn reply(version: &str, category: &str, endpoint: &str, app: &str, body: &Json) -> Json {
    let _lock = store::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = Ctx { version, app, body, user: header_value(app, "session").parse().unwrap_or(0) };
    let answer = if category.is_empty() {
        // a request without its URL's folders (`/Get`: the game's server URL still empty, its
        // host then the command's first word): the endpoint's category found by its name
        CATEGORIES.iter().find_map(|c| answer(&ctx, c, endpoint))
    } else {
        answer(&ctx, category, endpoint)
    };
    answer.unwrap_or_else(|| unknown(category, endpoint))
}

/// The categories, for [`reply`]'s requests without one (every endpoint name is unique).
const CATEGORIES: [&str; 11] = ["alive", "boot", "login", "user", "factory", "timetrial", "theory", "advertise", "log", "team", "matching"];

fn answer(ctx: &Ctx, category: &str, endpoint: &str) -> Option<Json> {
    let version = ctx.version;
    match (category, endpoint) {
        // the server's forced reboot a day ago, before the cabinet's ALL.Net certification (a
        // later one makes the game reboot at the attract: its auto reboot check)
        ("alive", "get") => Some(ok().with("server_status", 1u32).with("force_reboot_time", now() - 86400)),
        ("boot", "getconfigdata") => Some(config_data(version)),
        ("boot", "bookkeep") => Some(ok()),
        // timerelease_avatar_gacha_no 0: the game's own gachas
        ("boot", "getgachadata") => Some(ok().with("avatar_gacha_data", Vec::new())),
        ("boot", "gettimereleasedata") => Some(timerelease_data()),
        ("login", e) => login(ctx, e),
        ("user", e) => user::reply(ctx, e),
        ("factory", e) => factory::reply(ctx, e),
        ("timetrial", e) => records::time_trial(ctx, e),
        ("theory", "matching") => Some(records::matching(ctx)),
        ("advertise", "getrankingdata") => Some(records::ranking_data(ctx)),
        ("log", "saveplaylog" | "saveendlog") => Some(ok()),
        ("team", e) => team(ctx, e),
        ("matching", e) => matching(e),
        _ => None,
    }
}

fn client(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut length = 0usize;
    let mut app = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            } else if name.trim().eq_ignore_ascii_case("application") {
                app = value.trim().to_string();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let text = String::from_utf8_lossy(&body);
    // /<version>/initiald/<category>/<endpoint>, /<version>/initiald-matching/<endpoint>
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let version = header_value(&app, "device_version").replace('.', "");
    let version = version.get(..3).unwrap_or("230").to_string();
    let (category, endpoint) = match parts.iter().position(|p| p.starts_with("initiald")) {
        Some(i) if parts[i] == "initiald-matching" => ("matching".to_string(), parts.get(i + 1).unwrap_or(&"").to_ascii_lowercase()),
        Some(i) => (parts.get(i + 1).unwrap_or(&"").to_ascii_lowercase(), parts.get(i + 2).unwrap_or(&"").to_ascii_lowercase()),
        None => (String::new(), parts.last().unwrap_or(&"").to_ascii_lowercase()),
    };
    let request = Json::parse(&text).unwrap_or(Json::obj());
    let answer = reply(&version, &category, &endpoint, &app, &request).to_string();
    log!("idac: {path} {} -> {}", text.chars().take(600).collect::<String>(), answer.chars().take(300).collect::<String>());
    let mut out = stream;
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", answer.len()).as_bytes())?;
    out.write_all(answer.as_bytes())
}

fn serve() {
    let port = crate::gs::port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            log!("idac: server on 127.0.0.1:{port}");
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = client(stream);
                });
            }
        }
        Err(e) => log!("idac: 127.0.0.1:{port}: {e}"),
    }
}

pub(crate) fn init() {
    let _ = std::fs::create_dir_all("Y:\\idac");
    std::thread::spawn(serve);
}
