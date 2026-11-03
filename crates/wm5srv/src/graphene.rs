//! Tekken 7's game server ("graphene", `graphene.protobuf`): the cabinet's `https://<server>`
//! (the ALL.Net answer's uri), run in the game process by its payload.
//!
//! The game posts a `graphene.protobuf.Request` to `/` (field 1 its `MessageType`, then the
//! request's data in the field named after the type: `REGISTER_PCB` -> `register_pcb`) and
//! reads a `Response` (1 the type, 2 the result, 1: success, the answer in the field of the same
//! name). The messages are proto2 with required fields everywhere and their numbers differ
//! between Tekken 7 and Fated Retribution: the server reads the game's own schema (the
//! `graphene.proto` file descriptor in its executable, [`Schema`]) and builds every answer by
//! field name, the fields it does not set at their defaults (0, empty string, empty message).
//!
//! The players: one record per card in `<data>/users/<bp_amid>` ([`User`]): the card, the
//! name, the last fighter, each fighter's rank, points and record (`SaveChar`).

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::log;
use crate::pb::{Message, Value, Writer};

/// A field type of the schema.
#[derive(Clone, Debug)]
enum Kind {
    /// int32, int64, uint32, uint64, bool, enums (varints).
    Int,
    /// sint32, sint64 (zigzag).
    SInt,
    Bytes,
    /// A message, by its full name inside the package (`LoadUserResponseData.FighterInfo`).
    Message(String),
}

#[derive(Clone, Debug)]
struct Field {
    name: String,
    number: u32,
    required: bool,
    repeated: bool,
    kind: Kind,
}

/// The game's protocol: its messages' fields, the `MessageType` values.
#[derive(Default)]
pub struct Schema {
    messages: BTreeMap<String, Vec<Field>>,
    types: BTreeMap<u64, String>,
}

impl Schema {
    /// Reads a serialized `FileDescriptorProto` (the start of one; its end is found by its
    /// fields: the top-level ones are 1..12, length-delimited or varints).
    pub fn parse(data: &[u8]) -> Schema {
        let mut schema = Schema::default();
        let mut pos = 0;
        let mut fields = Vec::new();
        while let Some((field, value, next)) = top_field(data, pos) {
            fields.push((field, value));
            pos = next;
        }
        let package = fields.iter().find(|(f, _)| *f == 2).map(|(_, v)| String::from_utf8_lossy(v).into_owned()).unwrap_or_default();
        let prefix = format!(".{package}.");
        for (field, value) in &fields {
            match field {
                4 => schema.message(value, "", &prefix),
                5 => schema.enumeration(value),
                _ => {}
            }
        }
        schema
    }

    fn message(&mut self, data: &[u8], parent: &str, prefix: &str) {
        let Some(m) = Message::parse(data) else { return };
        let name = format!("{parent}{}", m.string(1).unwrap_or_default());
        let mut fields = Vec::new();
        for (f, v) in &m.0 {
            let Value::Bytes(b) = v else { continue };
            match f {
                2 => {
                    let Some(fd) = Message::parse(b) else { continue };
                    let kind = match fd.uint(5).unwrap_or(0) {
                        9 | 12 => Kind::Bytes,
                        17 | 18 => Kind::SInt,
                        11 => Kind::Message(fd.string(6).unwrap_or_default().trim_start_matches(prefix).to_string()),
                        _ => Kind::Int,
                    };
                    let label = fd.uint(4).unwrap_or(1);
                    fields.push(Field {
                        name: fd.string(1).unwrap_or_default(),
                        number: fd.uint(3).unwrap_or(0) as u32,
                        required: label == 2,
                        repeated: label == 3,
                        kind,
                    });
                }
                3 => self.message(b, &format!("{name}."), prefix),
                _ => {}
            }
        }
        self.messages.insert(name, fields);
    }

    fn enumeration(&mut self, data: &[u8]) {
        let Some(e) = Message::parse(data) else { return };
        if e.string(1).as_deref() != Some("MessageType") {
            return;
        }
        for v in e.all(2) {
            let Value::Bytes(b) = v else { continue };
            if let Some(value) = Message::parse(b) {
                self.types.insert(value.uint(2).unwrap_or(0), value.string(1).unwrap_or_default());
            }
        }
    }

    fn fields(&self, message: &str) -> &[Field] {
        self.messages.get(message).map_or(&[], |f| f.as_slice())
    }

    /// A message's type, by its field `name` in `Request` / `Response`.
    fn field_type(&self, message: &str, name: &str) -> Option<String> {
        self.fields(message).iter().find(|f| f.name == name).and_then(|f| match &f.kind {
            Kind::Message(m) => Some(m.clone()),
            _ => None,
        })
    }

    /// `values` encoded as `message`, by field name; the required fields it lacks at their
    /// defaults, the others absent.
    fn encode(&self, message: &str, values: &Obj) -> Vec<u8> {
        let mut w = Writer::new();
        for field in self.fields(message) {
            let value = values.iter().find(|(n, _)| *n == field.name).map(|(_, v)| v);
            match (value, &field.kind) {
                (Some(V::List(items)), Kind::Message(m)) => {
                    for item in items {
                        w = w.bytes(field.number, &self.encode(m, item));
                    }
                }
                (Some(V::M(obj)), Kind::Message(m)) => w = w.bytes(field.number, &self.encode(m, obj)),
                (Some(V::U(v)), Kind::Int) => w = w.uint(field.number, *v),
                (Some(V::I(v)), Kind::Int) => w = w.uint(field.number, *v as u64),
                (Some(V::I(v)), Kind::SInt) => w = w.sint(field.number, *v),
                (Some(V::U(v)), Kind::SInt) => w = w.sint(field.number, *v as i64),
                (Some(V::S(s)), Kind::Bytes) => w = w.bytes(field.number, s.as_bytes()),
                (None, kind) if field.required => {
                    w = match kind {
                        Kind::Int => w.uint(field.number, 0),
                        Kind::SInt => w.sint(field.number, 0),
                        Kind::Bytes => w.bytes(field.number, b""),
                        Kind::Message(m) => w.bytes(field.number, &self.encode(m, &Vec::new())),
                    }
                }
                (None, _) => {}
                (Some(v), _) => log(&format!("graphene: {message}.{}: value {v:?} of another type", field.name)),
            }
        }
        w.0
    }

    /// `data` as `message`, by field name (repeated fields: lists, nested messages decoded).
    fn decode(&self, message: &str, data: &[u8]) -> Obj {
        let Some(m) = Message::parse(data) else { return Vec::new() };
        let mut out: Obj = Vec::new();
        for field in self.fields(message) {
            let values: Vec<&Value> = m.all(field.number).collect();
            if values.is_empty() {
                continue;
            }
            let one = |v: &Value| match (&field.kind, v) {
                (Kind::Message(sub), Value::Bytes(b)) => V::M(self.decode(sub, b)),
                (Kind::Bytes, Value::Bytes(b)) => V::S(String::from_utf8_lossy(b).into_owned()),
                (Kind::SInt, Value::Varint(x)) => V::I(((*x >> 1) as i64) ^ -((*x & 1) as i64)),
                (_, Value::Varint(x)) => V::U(*x),
                _ => V::U(0),
            };
            if field.repeated {
                let items = values.iter().filter_map(|v| match one(v) {
                    V::M(obj) => Some(obj),
                    _ => None,
                });
                out.push((leak(&field.name), V::List(items.collect())));
            } else {
                out.push((leak(&field.name), one(values[values.len() - 1])));
            }
        }
        out
    }
}

/// The schema's field names live as long as the server.
fn leak(s: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap();
    if let Some(n) = names.iter().find(|n| **n == s) {
        return n;
    }
    let n: &'static str = Box::leak(s.to_string().into_boxed_str());
    names.push(n);
    n
}

/// One top-level field of a file descriptor at `pos`: (number, data, next position).
fn top_field(data: &[u8], pos: usize) -> Option<(u32, &[u8], usize)> {
    let mut p = pos;
    let key = varint(data, &mut p)?;
    let (field, wire) = ((key >> 3) as u32, key & 7);
    if !(1..=12).contains(&field) {
        return None;
    }
    match wire {
        0 => {
            varint(data, &mut p)?;
            Some((field, &data[pos..pos], p))
        }
        2 => {
            let len = varint(data, &mut p)? as usize;
            let value = data.get(p..p + len)?;
            Some((field, value, p + len))
        }
        _ => None,
    }
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// A field value, by name ([`Schema::encode`], [`Schema::decode`]).
#[derive(Clone, Debug)]
pub enum V {
    U(u64),
    I(i64),
    S(String),
    M(Obj),
    List(Vec<Obj>),
}

/// A message's fields by name.
pub type Obj = Vec<(&'static str, V)>;

fn get<'a>(obj: &'a Obj, name: &str) -> Option<&'a V> {
    obj.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
}

fn uint(obj: &Obj, name: &str) -> u64 {
    match get(obj, name) {
        Some(V::U(v)) => *v,
        Some(V::I(v)) => *v as u64,
        _ => 0,
    }
}

fn int(obj: &Obj, name: &str) -> i64 {
    match get(obj, name) {
        Some(V::I(v)) => *v,
        Some(V::U(v)) => *v as i64,
        _ => 0,
    }
}

fn string(obj: &Obj, name: &str) -> String {
    match get(obj, name) {
        Some(V::S(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A player: the card and its fighters.
///
/// Record (protocol buffers of this server): 1 bp_amid, 2 access code, 3 chip id, 4 name,
/// 5 last fighter, 6 last side, 7 fight money, 8 play count, 9 fighters (1 id, 2 rank, 3 rank
/// points, 4 wins, 5 losses, 6 proper rank wins - losses, 7 counter a, 8 counter b, 9 highest
/// rank), 10 treasure gauge.
#[derive(Clone, Default, Debug)]
struct User {
    bp_amid: u32,
    access_code: String,
    chip_id: String,
    name: String,
    last_fighter: u32,
    last_side: u32,
    fight_money: u32,
    plays: u32,
    treasure_gauge: u32,
    fighters: BTreeMap<u32, Fighter>,
}

#[derive(Clone, Copy, Default, Debug)]
struct Fighter {
    rank: u32,
    exp: u32,
    wins: u64,
    loses: u64,
    proper: i64,
    counter_a: u32,
    counter_b: u32,
    highest: u32,
}

impl User {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new()
            .uint(1, self.bp_amid as u64)
            .string(2, &self.access_code)
            .string(3, &self.chip_id)
            .string(4, &self.name)
            .uint(5, self.last_fighter as u64)
            .uint(6, self.last_side as u64)
            .uint(7, self.fight_money as u64)
            .uint(8, self.plays as u64)
            .uint(10, self.treasure_gauge as u64);
        for (id, f) in &self.fighters {
            let fw = Writer::new()
                .uint(1, *id as u64)
                .uint(2, f.rank as u64)
                .uint(3, f.exp as u64)
                .uint(4, f.wins)
                .uint(5, f.loses)
                .sint(6, f.proper)
                .uint(7, f.counter_a as u64)
                .uint(8, f.counter_b as u64)
                .uint(9, f.highest as u64);
            w = w.message(9, fw);
        }
        w.0
    }

    fn decode(data: &[u8]) -> Option<User> {
        let m = Message::parse(data)?;
        let mut user = User {
            bp_amid: m.uint(1)? as u32,
            access_code: m.string(2).unwrap_or_default(),
            chip_id: m.string(3).unwrap_or_default(),
            name: m.string(4).unwrap_or_default(),
            last_fighter: m.uint(5).unwrap_or(0) as u32,
            last_side: m.uint(6).unwrap_or(0) as u32,
            fight_money: m.uint(7).unwrap_or(0) as u32,
            plays: m.uint(8).unwrap_or(0) as u32,
            treasure_gauge: m.uint(10).unwrap_or(0) as u32,
            fighters: BTreeMap::new(),
        };
        for v in m.all(9) {
            let Value::Bytes(b) = v else { continue };
            let Some(f) = Message::parse(b) else { continue };
            let z = f.uint(6).unwrap_or(0);
            user.fighters.insert(
                f.uint(1).unwrap_or(0) as u32,
                Fighter {
                    rank: f.uint(2).unwrap_or(0) as u32,
                    exp: f.uint(3).unwrap_or(0) as u32,
                    wins: f.uint(4).unwrap_or(0),
                    loses: f.uint(5).unwrap_or(0),
                    proper: ((z >> 1) as i64) ^ -((z & 1) as i64),
                    counter_a: f.uint(7).unwrap_or(0) as u32,
                    counter_b: f.uint(8).unwrap_or(0) as u32,
                    highest: f.uint(9).unwrap_or(0) as u32,
                },
            );
        }
        Some(user)
    }
}

struct Server {
    schema: Schema,
    dir: PathBuf,
    /// The players' name (`WAL_NAMCOES3_PROFILE_NAME`).
    name: String,
    lock: Mutex<()>,
}

static SERVER: OnceLock<Server> = OnceLock::new();

fn users_dir() -> PathBuf {
    SERVER.get().map(|s| s.dir.join("users")).unwrap_or_default()
}

fn load_user(bp_amid: u32) -> Option<User> {
    User::decode(&std::fs::read(users_dir().join(bp_amid.to_string())).ok()?)
}

fn save_user(user: &User) {
    let path = users_dir().join(user.bp_amid.to_string());
    if let Err(e) = std::fs::write(&path, user.encode()) {
        log(&format!("graphene: {}: {e}", path.display()));
    }
}

/// The card's player, made on its first use (`new`: just made).
fn find_or_create(access_code: &str, chip_id: &str, name: &str) -> (User, bool) {
    let dir = users_dir();
    let mut ids: Vec<u32> = std::fs::read_dir(&dir).map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default();
    ids.sort();
    for &id in &ids {
        if let Some(u) = load_user(id)
            && !access_code.is_empty()
            && u.access_code == access_code
        {
            return (u, false);
        }
    }
    let user = User {
        bp_amid: ids.last().map_or(1, |l| l + 1),
        access_code: access_code.to_string(),
        chip_id: chip_id.to_string(),
        name: name.to_string(),
        ..User::default()
    };
    save_user(&user);
    log(&format!("graphene: new player {} for card {access_code}", user.bp_amid));
    (user, true)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A request's result: done (the game retries others).
const OK: u64 = 1;

/// The answer to one request: (result, the answer's fields).
fn answer(server: &Server, kind: &str, req: &Obj) -> (u64, Obj) {
    let _guard = server.lock.lock().unwrap_or_else(|e| e.into_inner());
    match kind {
        "REGISTER_PCB" => (
            OK,
            vec![
                ("server_time", V::U(now())),
                ("operate_start", V::S("00:00".into())),
                ("operate_end", V::S("23:59".into())),
            ],
        ),
        // Fated Retribution: the card (access code) and Bandai Namco ID services up
        "PING" => (OK, vec![("acid_status", V::U(1)), ("bnid_status", V::U(1))]),
        "CHECK_ACCESSCODE" => {
            let chip = string(req, "chip_id");
            let code = find_by_chip(&chip).map(|u| u.access_code).unwrap_or_default();
            (OK, vec![("access_code", V::S(code))])
        }
        "LOAD_USER" | "DRAW_DAILY_LOT" => {
            let (user, new) = find_or_create(&string(req, "access_code"), &string(req, "chip_id"), &server.name);
            let fighters: Vec<Obj> = user
                .fighters
                .iter()
                .map(|(id, f)| {
                    vec![
                        ("fighter_id", V::U(*id as u64)),
                        ("rank_id", V::U(f.rank as u64)),
                        ("rank_exp", V::U(f.exp as u64)),
                        ("char_wins_count", V::U(f.wins)),
                        ("char_lose_count", V::U(f.loses)),
                        ("properrank_winlose", V::I(f.proper)),
                        ("counter_a", V::U(f.counter_a as u64)),
                        ("counter_b", V::U(f.counter_b as u64)),
                    ]
                })
                .collect();
            let highest = user.fighters.values().map(|f| f.highest.max(f.rank)).max().unwrap_or(0);
            (
                OK,
                vec![
                    ("access_code", V::S(user.access_code.clone())),
                    ("bp_amid", V::U(user.bp_amid as u64)),
                    ("user_name", V::S(user.name.clone())),
                    ("last_fighter_id", V::U(user.last_fighter as u64)),
                    ("last_playside", V::U(user.last_side as u64)),
                    ("new_card", V::U(new as u64)),
                    ("dayplay_count", V::U(user.plays as u64)),
                    ("all_highestrank_id", V::U(highest as u64)),
                    ("fightmoney_per_all", V::U(user.fight_money as u64)),
                    ("fightmoney_per_person", V::U(user.fight_money as u64)),
                    ("treasure_gauge", V::U(user.treasure_gauge as u64)),
                    ("fighter_info", V::List(fighters)),
                ],
            )
        }
        "LOAD_CHAR" => {
            let id = uint(req, "fighter_id") as u32;
            let f = load_user(uint(req, "bp_amid") as u32).and_then(|u| u.fighters.get(&id).copied()).unwrap_or_default();
            (
                OK,
                vec![
                    ("fighter_id", V::U(id as u64)),
                    ("rank_id", V::U(f.rank as u64)),
                    ("rank_exp", V::U(f.exp as u64)),
                    ("highest_rank_id", V::U(f.highest.max(f.rank) as u64)),
                ],
            )
        }
        "SAVE_CHAR" => {
            if let Some(mut user) = load_user(uint(req, "bp_amid") as u32) {
                let id = uint(req, "fighter_id") as u32;
                let f = user.fighters.entry(id).or_default();
                f.rank = uint(req, "rank_id") as u32;
                f.exp = uint(req, "rank_exp") as u32;
                f.highest = f.highest.max(f.rank);
                f.wins += uint(req, "pvp_wins_count");
                f.loses += uint(req, "pvp_lose_count");
                f.proper = int(req, "properrank_winlose");
                f.counter_a = uint(req, "counter_a") as u32;
                f.counter_b = uint(req, "counter_b") as u32;
                user.last_fighter = id;
                // the fight money won or spent (a signed 32-bit delta)
                let delta = uint(req, "fightmoney") as u32 as i32 as i64;
                user.fight_money = (user.fight_money as i64 + delta).clamp(0, u32::MAX as i64) as u32;
                user.treasure_gauge = user.treasure_gauge.saturating_add(uint(req, "add_treasure_gauge") as u32);
                if uint(req, "last_flag") != 0 {
                    user.plays += 1;
                }
                save_user(&user);
            }
            (OK, Vec::new())
        }
        "SAVE_MRESULT" => {
            if let Some(mut user) = load_user(uint(req, "bp_amid") as u32) {
                user.last_side = uint(req, "playside") as u32;
                save_user(&user);
            }
            (OK, Vec::new())
        }
        _ => (OK, Vec::new()),
    }
}

fn find_by_chip(chip: &str) -> Option<User> {
    let dir = users_dir();
    std::fs::read_dir(dir).ok()?.flatten().filter_map(|e| User::decode(&std::fs::read(e.path()).ok()?)).find(|u| !chip.is_empty() && u.chip_id == chip)
}

fn route(_method: &str, _target: &str, body: &[u8]) -> Option<Vec<u8>> {
    let server = SERVER.get()?;
    let request = Message::parse(body)?;
    let ty = request.uint(1).unwrap_or(0);
    let kind = server.schema.types.get(&ty).cloned().unwrap_or_else(|| format!("type {ty}"));
    let field = kind.to_ascii_lowercase();
    let req = match (server.schema.field_type("Request", &field), server.schema.fields("Request").iter().find(|f| f.name == field)) {
        (Some(m), Some(f)) => request.bytes(f.number).map(|b| server.schema.decode(&m, b)).unwrap_or_default(),
        _ => Vec::new(),
    };
    let (result, fields) = answer(server, &kind, &req);
    if crate::verbose() {
        log(&format!("graphene: {kind} {req:?} -> {result} {fields:?}"));
    } else {
        log(&format!("graphene: {kind}"));
    }
    let mut response: Obj = vec![("type", V::U(ty)), ("result", V::U(result))];
    if server.schema.field_type("Response", &field).is_some() {
        response.push((leak(&field), V::M(fields)));
    }
    Some(server.schema.encode("Response", &response))
}

/// Starts the server on `addr` (`host:port`): `descriptor` the game's `graphene.proto` file
/// descriptor, the players in `data_dir`, a new player named `name`.
pub fn start(addr: &str, descriptor: &[u8], data_dir: &str, name: &str, logger: fn(&str), verbose: bool) {
    let _ = crate::LOGGER.set(logger);
    let _ = crate::VERBOSE.set(verbose);
    let schema = Schema::parse(descriptor);
    log(&format!("graphene: {} messages, {} request types", schema.messages.len(), schema.types.len()));
    let dir = PathBuf::from(data_dir);
    if let Err(e) = std::fs::create_dir_all(dir.join("users")) {
        log(&format!("graphene: {}: {e}", dir.display()));
    }
    let _ = SERVER.set(Server { schema, dir, name: name.to_string(), lock: Mutex::new(()) });
    let config = match crate::tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log(&format!("graphene: TLS: {e}"));
            return;
        }
    };
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("graphene: {addr}: {e}"));
            return;
        }
    };
    log(&format!("graphene: listening on https://{addr}, data in {data_dir}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::serve(stream, config, route, "application/x-protobuf") {
                    if crate::verbose() {
                        log(&format!("graphene: connection: {e}"));
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file descriptor: `Ping { required uint32 a = 1; required string b = 2; optional uint32
    /// c = 3; }`, `MessageType { PING = 9; }`.
    fn descriptor() -> Vec<u8> {
        let field = |name: &str, number: u64, label: u64, ty: u64| {
            Writer::new().string(1, name).uint(3, number).uint(4, label).uint(5, ty)
        };
        let ping = Writer::new().string(1, "Ping").message(2, field("a", 1, 2, 13)).message(2, field("b", 2, 2, 9)).message(2, field("c", 3, 1, 13));
        let types = Writer::new().string(1, "MessageType").message(2, Writer::new().string(1, "PING").uint(2, 9));
        Writer::new().string(1, "graphene.proto").string(2, "graphene.protobuf").message(4, ping).message(5, types).0
    }

    #[test]
    fn encodes_by_name_with_required_defaults() {
        let schema = Schema::parse(&descriptor());
        assert_eq!(schema.types.get(&9).map(String::as_str), Some("PING"));
        // a set, b required and defaulted, c optional and absent
        let data = schema.encode("Ping", &vec![("a", V::U(5))]);
        let m = Message::parse(&data).unwrap();
        assert_eq!(m.uint(1), Some(5));
        assert_eq!(m.string(2).as_deref(), Some(""));
        assert!(m.get(3).is_none());
        let back = schema.decode("Ping", &data);
        assert_eq!(uint(&back, "a"), 5);
        assert_eq!(string(&back, "b"), "");
    }

    #[test]
    fn user_record_round_trip() {
        let mut user = User { bp_amid: 3, access_code: "0123".into(), name: "P".into(), fight_money: 200, treasure_gauge: 4, ..User::default() };
        user.fighters.insert(9, Fighter { rank: 2, exp: 40, wins: 1, loses: 2, proper: -1, ..Fighter::default() });
        let back = User::decode(&user.encode()).unwrap();
        assert_eq!((back.bp_amid, back.fight_money, back.treasure_gauge), (3, 200, 4));
        let f = back.fighters[&9];
        assert_eq!((f.rank, f.exp, f.wins, f.loses, f.proper), (2, 40, 1, 2, -1));
    }
}
