//! Mobile Suit Gundam Extreme Vs. 2's game server ("nue", `nue.protocol.exvs`): the cabinet's
//! card and game data server, run in the game process by its payload. The protocol and the
//! answers follow EXVS2-POC's `Server-Vanilla` (asesidaa/EXVS2-POC, reference only: no license;
//! nothing of it is copied, its behaviour is written again here).
//!
//! The game posts a `nue.protocol.exvs.Request` (protocol buffers, proto2) to `/`: field 1 its
//! `MethodType` (`MTHD_LOAD_CARD`), then the request in the field named after it (`load_card`),
//! and reads a `Response` of the same type and request id, `error` `SUCCESS`, the answer in the
//! field of the same name. The messages' numbers and required fields come from the game's own
//! `game_server_nue.proto` descriptor ([`Schema`]); every answer is built by field name.
//!
//! Players ([`Player`]): one file per card in `<data>/players/<id>`, holding the game's own
//! messages: the pre-load card's user (`Response.PreLoadCard.MobileUserGroup`) and record
//! (`LoadPlayer`), the load card's pilot data (`PilotDataGroup`) and customizations
//! (`MobileUserGroup`). The battle results (versus, Triad CPU battles) update them as
//! EXVS2-POC's save commands do (`save.rs`).

mod data;
mod save;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::graphene::{Obj, Schema, V, get, leak, string, uint};
use crate::log;
use crate::pb::{Message, Writer};

struct Server {
    schema: Schema,
    /// The matchmaking server's protocol (`matchmaking_server_nue.proto`, `nue.protocol.mms`).
    mms: Schema,
    dir: PathBuf,
    /// A new player's name (`WAL_NAMCOES3_PROFILE_NAME`).
    name: String,
    /// The server's `host:port`.
    addr: String,
    lock: Mutex<()>,
}

static SERVER: OnceLock<Server> = OnceLock::new();

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// --- message fields by name ---

pub(crate) fn set(obj: &mut Obj, name: &str, value: V) {
    match obj.iter_mut().find(|(n, _)| *n == name) {
        Some((_, v)) => *v = value,
        None => obj.push((leak(name), value)),
    }
}

pub(crate) fn add(obj: &mut Obj, name: &str, delta: i64) {
    let v = (uint(obj, name) as i64).saturating_add(delta).max(0) as u64;
    set(obj, name, V::U(v));
}

pub(crate) fn flag(obj: &Obj, name: &str) -> bool {
    uint(obj, name) != 0
}

pub(crate) fn message(obj: &Obj, name: &str) -> Obj {
    match get(obj, name) {
        Some(V::M(m)) => m.clone(),
        _ => Vec::new(),
    }
}

pub(crate) fn list(obj: &Obj, name: &str) -> Vec<Obj> {
    match get(obj, name) {
        Some(V::List(l)) => l.clone(),
        Some(V::M(m)) => vec![m.clone()],
        _ => Vec::new(),
    }
}

pub(crate) fn ints(obj: &Obj, name: &str) -> Vec<u64> {
    match get(obj, name) {
        Some(V::Ints(l)) => l.clone(),
        Some(V::U(v)) => vec![*v],
        _ => Vec::new(),
    }
}

/// A signed field (sint32: `V::I`).
pub(crate) fn signed(obj: &Obj, name: &str) -> i64 {
    crate::graphene::int(obj, name)
}

// --- players ---

/// A card's player: the card, its session, and the game's messages that hold its data.
///
/// File (protocol buffers of this server): 1 id, 2 access code, 3 chip id, 4 new card (not
/// registered yet), 5 session id, 6 user (`Response.PreLoadCard.MobileUserGroup`), 7 record
/// (`Response.PreLoadCard.LoadPlayer`), 8 pilot data (`Response.LoadCard.PilotDataGroup`), 9
/// customizations (`Response.LoadCard.MobileUserGroup`), 10 last play (Unix time).
#[derive(Clone, Default)]
pub(crate) struct Player {
    pub(crate) id: u32,
    pub(crate) access_code: String,
    pub(crate) chip_id: String,
    pub(crate) new_card: bool,
    pub(crate) session: String,
    pub(crate) user: Obj,
    pub(crate) record: Obj,
    pub(crate) pilot: Obj,
    pub(crate) mobile: Obj,
    pub(crate) last_played: u64,
}

const USER: &str = "Response.PreLoadCard.MobileUserGroup";
const RECORD: &str = "Response.PreLoadCard.LoadPlayer";
const PILOT: &str = "Response.LoadCard.PilotDataGroup";
const MOBILE: &str = "Response.LoadCard.MobileUserGroup";

impl Player {
    /// A new card's player, EXVS2-POC's new card profile: 10000 GP, the record open, the skill
    /// points shown, boosts 1, the navigators' advice and notices on, the training with the
    /// first mobile suit (burst type 2, CPU level 1, damage shown), the gamepad's default keys.
    fn new(id: u32, access_code: &str, chip_id: &str, name: &str) -> Player {
        let user: Obj = vec![
            ("user_id", V::U(id as u64)),
            ("player_name", V::S(name.to_string())),
            ("open_record", V::U(1)),
            ("open_echelon", V::U(0)),
            ("open_skillpoint", V::U(1)),
            ("keyconfig_number", V::U(1)),
            ("headphone_flag", V::U(0)),
            ("gp", V::U(10000)),
            (
                "customize_group",
                V::M(vec![
                    ("gp_boost", V::U(1)),
                    ("guest_nav_boost", V::U(1)),
                    ("tag_skill_point_boost_flag", V::U(1)),
                    ("battle_nav_advise_flag", V::U(1)),
                    ("battle_nav_notify_flag", V::U(1)),
                ]),
            ),
        ];
        let record: Obj = vec![("pilot_id", V::U(id as u64))];
        let pilot: Obj = vec![(
            "training",
            V::M(vec![("mst_mobile_suit_id", V::U(1)), ("burst_type", V::U(2)), ("cpu_level", V::U(1)), ("ex_burst_gauge", V::U(0)), ("damage_display", V::U(1))]),
        )];
        let gamepad: Obj = [("x_key", 1), ("y_key", 2), ("a_key", 3), ("b_key", 4), ("lb_key", 5), ("rb_key", 6), ("lt_key", 7), ("rt_key", 8), ("lsb_key", 0), ("rsb_key", 9)]
            .into_iter()
            .map(|(k, v)| (k, V::U(v)))
            .collect();
        let mobile: Obj = vec![("customize", V::M(Vec::new())), ("gamepad", V::M(gamepad))];
        Player {
            id,
            access_code: access_code.to_string(),
            chip_id: chip_id.to_string(),
            new_card: true,
            session: String::new(),
            user,
            record,
            pilot,
            mobile,
            last_played: now(),
        }
    }

    fn encode(&self, schema: &Schema) -> Vec<u8> {
        Writer::new()
            .uint(1, self.id as u64)
            .string(2, &self.access_code)
            .string(3, &self.chip_id)
            .uint(4, self.new_card as u64)
            .string(5, &self.session)
            .bytes(6, &schema.encode(USER, &self.user))
            .bytes(7, &schema.encode(RECORD, &self.record))
            .bytes(8, &schema.encode(PILOT, &self.pilot))
            .bytes(9, &schema.encode(MOBILE, &self.mobile))
            .uint(10, self.last_played)
            .0
    }

    fn decode(schema: &Schema, data: &[u8]) -> Option<Player> {
        let m = Message::parse(data)?;
        let part = |field: u32, name: &str| m.bytes(field).map(|b| schema.decode(name, b)).unwrap_or_default();
        Some(Player {
            id: m.uint(1)? as u32,
            access_code: m.string(2).unwrap_or_default(),
            chip_id: m.string(3).unwrap_or_default(),
            new_card: m.uint(4).unwrap_or(0) != 0,
            session: m.string(5).unwrap_or_default(),
            user: part(6, USER),
            record: part(7, RECORD),
            pilot: part(8, PILOT),
            mobile: part(9, MOBILE),
            last_played: m.uint(10).unwrap_or(0),
        })
    }
}

fn players_dir(server: &Server) -> PathBuf {
    server.dir.join("players")
}

fn ids(server: &Server) -> Vec<u32> {
    let mut ids: Vec<u32> = std::fs::read_dir(players_dir(server)).map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default();
    ids.sort();
    ids
}

fn load(server: &Server, id: u32) -> Option<Player> {
    Player::decode(&server.schema, &std::fs::read(players_dir(server).join(id.to_string())).ok()?)
}

fn store(server: &Server, player: &Player) {
    let path = players_dir(server).join(player.id.to_string());
    if let Err(e) = std::fs::write(&path, player.encode(&server.schema)) {
        log(&format!("exvs2: {}: {e}", path.display()));
    }
}

fn find(server: &Server, matches: impl Fn(&Player) -> bool) -> Option<Player> {
    ids(server).into_iter().filter_map(|id| load(server, id)).find(|p| matches(p))
}

// --- requests ---

fn response(error: u64, extra: Option<(&str, Obj)>, req: &Obj) -> Obj {
    let mut r: Obj = vec![("type", V::U(uint(req, "type"))), ("request_id", V::S(string(req, "request_id"))), ("error", V::U(error))];
    if let Some((name, fields)) = extra {
        r.push((leak(name), V::M(fields)));
    }
    r
}

const SUCCESS: u64 = 0;
const ERR_SERVER: u64 = 2;
const ACID_SUCCESS: u64 = 0;
const ACID_NO_USE: u64 = 1;

/// The answer to one request: (error, the answer's fields; `None`: no answer message).
fn answer(server: &Server, method: &str, req: &Obj, body: &Obj) -> (u64, Option<Obj>) {
    let _guard = server.lock.lock().unwrap_or_else(|e| e.into_inner());
    match method {
        "register_pcb" => {
            // the matching server (one cabinet: none answers), maintenance far away
            let info: Obj = vec![("server_type", V::U(2)), ("uri", V::S(format!("{}/match", server.addr))), ("port", V::U(12345))];
            (
                SUCCESS,
                Some(vec![
                    ("server_info", V::List(vec![info])),
                    ("sram_clear", V::U(1)),
                    ("next_maintenance_start_at", V::U(2005364002)),
                    ("next_maintenance_end_at", V::U(2005364004)),
                ]),
            )
        }
        "ping" => (SUCCESS, Some(vec![("response_at", V::U(now())), ("game_server", V::U(1)), ("acid_server", V::U(1)), ("matchmaking_server", V::U(1))])),
        "check_time" => (SUCCESS, Some(vec![("at", V::U(now()))])),
        "load_game_data" => (SUCCESS, Some(data::load_game_data())),
        "load_black_list" => (
            SUCCESS,
            Some(vec![
                ("threshold_delayed_rtt", V::U(300)),
                ("max_blacklist_num", V::U(1)),
                ("delay_restrict_condition", V::U(300)),
                ("delay_permit_condition", V::U(300)),
                ("threshold_delayed_frame", V::U(5)),
            ]),
        ),
        "check_telop" => (SUCCESS, Some(vec![("telop1_id", V::U(1))])),
        "load_telop" => (SUCCESS, Some(vec![("telop_data", V::S(String::new()))])),
        "load_spot_url" => (SUCCESS, Some(vec![("url", V::S("https://example.com".into())), ("qrcode", V::S("https://example.com".into()))])),
        // no shop information
        "load_spot_info" => (SUCCESS, None),
        "save_charge" => {
            let results = list(body, "charge_data").iter().map(|_| vec![("hc2_error", V::U(0))]).collect();
            (SUCCESS, Some(vec![("save_charge_result", V::List(results))]))
        }
        "pre_load_card" => (SUCCESS, Some(pre_load_card(server, body))),
        "register_card" => register_card(server, body),
        "load_card" => load_card(server, body),
        "load_tag_info" => (SUCCESS, Some(Vec::new())),
        "load_replay_card" => {
            let (code, chip) = (string(body, "access_code"), string(body, "chip_id"));
            match find(server, |p| p.access_code == code && p.chip_id == chip) {
                Some(p) => (
                    SUCCESS,
                    Some(vec![
                        ("pilot_id", V::U(p.id as u64)),
                        ("is_new_card", V::U(p.new_card as u64)),
                        ("access_code", V::S(code)),
                        ("user", V::M(vec![("user_id", V::U(p.id as u64)), ("player_name", V::S(string(&p.user, "player_name")))])),
                        ("acid_error", V::U(ACID_SUCCESS)),
                    ]),
                ),
                None => (SUCCESS, Some(vec![("acid_error", V::U(ACID_NO_USE))])),
            }
        }
        // replays are not kept: the upload goes to a server that answers nothing
        "pre_save_replay" => (SUCCESS, Some(vec![("url", V::S(format!("http://{}/upload/uploadReplay/0/{}", server.addr, uint(body, "played_at"))))])),
        "save_vsm_result" | "save_vsc_result" => {
            save::battle(server, method == "save_vsc_result", body);
            (SUCCESS, Some(Vec::new()))
        }
        // logs, the cabinet's statistics: received
        "register_pcb_ack" | "save_inside_data" | "save_log" | "save_battle_log" | "save_user_play_research_data" | "check_resource_data" | "check_movie_release"
        | "load_ranking" => (SUCCESS, Some(Vec::new())),
        _ => {
            log(&format!("exvs2: {method} (type {}): not handled", uint(req, "type")));
            (ERR_SERVER, None)
        }
    }
}

/// The card: an existing player (registered: its data), else a new card (made now, registered
/// by `register_card`), each with a new session.
fn pre_load_card(server: &Server, body: &Obj) -> Obj {
    let (code, chip) = (string(body, "access_code"), string(body, "chip_id"));
    let session = format!("{code}{}", now());
    let mut player = match find(server, |p| p.access_code == code && p.chip_id == chip) {
        Some(p) => p,
        None => {
            let id = ids(server).last().map_or(1, |l| l + 1);
            log(&format!("exvs2: new player {id} for card {code}"));
            Player::new(id, &code, &chip, &server.name)
        }
    };
    player.session = session.clone();
    store(server, &player);
    let mut out: Obj = vec![("session_id", V::S(session)), ("acid_error", V::U(ACID_SUCCESS))];
    if player.new_card {
        out.push(("is_new_card", V::U(1)));
        return out;
    }
    // the favorite mobile suits' use counts from the pilot's mobile suits
    let mut user = player.user.clone();
    let used: Vec<(u64, u64)> = list(&player.pilot, "ms_skills").iter().map(|m| (uint(m, "mst_mobile_suit_id"), uint(m, "ms_used_num"))).collect();
    let favorites: Vec<Obj> = list(&user, "favorite_mobile_suits")
        .into_iter()
        .map(|mut f| {
            let n = used.iter().find(|(id, _)| *id == uint(&f, "mst_mobile_suit_id")).map_or(0, |(_, n)| *n);
            set(&mut f, "ms_used_num", V::U(n));
            f
        })
        .collect();
    if !favorites.is_empty() {
        set(&mut user, "favorite_mobile_suits", V::List(favorites));
    }
    let mut record = player.record.clone();
    set(&mut record, "pilot_id", V::U(player.id as u64));
    set(&mut record, "last_played_at", V::U(player.last_played));
    out.extend([("am_id", V::U(1)), ("is_new_card", V::U(0)), ("user", V::M(user)), ("load_player", V::M(record))]);
    out
}

fn register_card(server: &Server, body: &Obj) -> (u64, Option<Obj>) {
    let (code, session) = (string(body, "access_code"), string(body, "session_id"));
    let Some(mut player) = find(server, |p| p.access_code == code && p.session == session) else {
        return (SUCCESS, None);
    };
    player.new_card = false;
    store(server, &player);
    log(&format!("exvs2: card {code} registered, pilot {}", player.id));
    (
        SUCCESS,
        Some(vec![
            ("session_id", V::S(session)),
            ("access_code", V::S(code)),
            ("acid_error", V::U(ACID_NO_USE)),
            ("is_registered", V::U(1)),
            ("pilot_id", V::U(player.id as u64)),
        ]),
    )
}

fn load_card(server: &Server, body: &Obj) -> (u64, Option<Obj>) {
    let session = string(body, "session_id");
    let Some(player) = find(server, |p| !p.session.is_empty() && p.session == session) else {
        return (ERR_SERVER, None);
    };
    let mut pilot = player.pilot.clone();
    // the tag teams' skill point boost (EXVS2-POC: 50, divided by 5 in the game)
    let teams: Vec<Obj> = list(&pilot, "tag_teams")
        .into_iter()
        .map(|mut t| {
            set(&mut t, "skill_point_boost", V::U(50));
            t
        })
        .collect();
    if !teams.is_empty() {
        set(&mut pilot, "tag_teams", V::List(teams));
    }
    (SUCCESS, Some(vec![("pilot_data_group", V::M(pilot)), ("mobile_user_group", V::M(player.mobile.clone()))]))
}

/// One request: its type's name (`MTHD_LOAD_CARD` -> `load_card`), the request's fields.
fn route(_method: &str, target: &str, body: &[u8]) -> Option<Vec<u8>> {
    let server = SERVER.get()?;
    if target.starts_with("/match") {
        return Some(matching(server, body));
    }
    if !target.starts_with('/') || target.starts_with("/upload") {
        log(&format!("exvs2: {target} ({} bytes)", body.len()));
        return Some(Vec::new());
    }
    let req = server.schema.decode("Request", body);
    let ty = uint(&req, "type");
    let kind = server.schema.types.get(&ty).cloned().unwrap_or_else(|| format!("MTHD_{ty}"));
    let method = kind.trim_start_matches("MTHD_").to_ascii_lowercase();
    let fields = match get(&req, &method) {
        Some(V::M(m)) => m.clone(),
        _ => Vec::new(),
    };
    let (error, out) = answer(server, &method, &req, &fields);
    if crate::verbose() {
        log(&format!("exvs2: {method} {fields:?} -> {error} {out:?}"));
    } else {
        log(&format!("exvs2: {method}"));
    }
    let out = out.map(|o| (method.as_str(), o));
    Some(server.schema.encode("Response", &response(error, out, &req)))
}

/// The matchmaking server (`/match`), EXVS2-POC's: its ping, the node id issued (1); one cabinet
/// is matched with nobody (the matching requests fail).
fn matching(server: &Server, body: &[u8]) -> Vec<u8> {
    let req = server.mms.decode("Request", body);
    let ty = uint(&req, "type");
    let method = server.mms.types.get(&ty).cloned().unwrap_or_default().to_ascii_lowercase();
    let (code, out): (u64, Option<Obj>) = match method.as_str() {
        "ping" => (0, Some(Vec::new())),
        "issue_node_id" => (0, Some(vec![("node_id", V::Ints(vec![1]))])),
        // ERR_SERVER
        _ => (101, None),
    };
    if crate::verbose() || code != 0 {
        log(&format!("exvs2: match {method} -> {code}"));
    }
    let mut r: Obj = vec![("type", V::U(ty)), ("request_id", V::S(string(&req, "request_id"))), ("code", V::U(code))];
    if let Some(out) = out {
        r.push((leak(&method), V::M(out)));
    }
    server.mms.encode("Response", &r)
}

/// Starts the server on `addr` (`host:port`, plain HTTP): `descriptor` the game's
/// `game_server_nue.proto` file descriptor (`mms`: its `matchmaking_server_nue.proto`), the players
/// in `data_dir`, a new player named `name`.
pub fn start(addr: &str, descriptor: &[u8], mms: &[u8], data_dir: &str, name: &str, logger: fn(&str), verbose: bool) {
    let _ = crate::LOGGER.set(logger);
    let _ = crate::VERBOSE.set(verbose);
    let schema = Schema::parse(descriptor);
    let mms = Schema::parse(mms);
    log(&format!("exvs2: {} request types ({})", schema.types.len(), schema.package));
    let dir = PathBuf::from(data_dir);
    if let Err(e) = std::fs::create_dir_all(dir.join("players")) {
        log(&format!("exvs2: {}: {e}", dir.display()));
    }
    let _ = SERVER.set(Server { schema, mms, dir, name: name.chars().take(12).collect(), addr: addr.to_string(), lock: Mutex::new(()) });
    let config = match crate::tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log(&format!("exvs2: TLS: {e}"));
            return;
        }
    };
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("exvs2: {addr}: {e}"));
            return;
        }
    };
    log(&format!("exvs2: listening on http://{addr}, data in {data_dir}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::serve(stream, config, route, "application/protobuf") {
                    if crate::verbose() {
                        log(&format!("exvs2: connection: {e}"));
                    }
                }
            });
        }
    });
}
