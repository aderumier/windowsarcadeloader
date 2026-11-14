//! KanColle Arcade's title server (`http://<router>/SDBZ/<command>`, the power-on's uri): no
//! server exists; the payload answers the commands the game needs to play offline.
//!
//! A request's and an answer's body: a 16-byte IV, then AES-128-CBC (key below, PKCS#7 padding)
//! of the zlib-compressed MessagePack map (the game's mode flags 3: compressed, encrypted).
//! Requests carry `cmd` and `req_id`; an answer gives them back with `stat` (`ok`; the game
//! refuses `warning`, `ng`, `db_close`, `maintenance`) and the command's values:
//! - `game_init`: the database closing and exercise maintenance windows (start, end: in the
//!   past), guest play
//!   not refused at closing, no retry delay, `ovi_kind` 0.
//! - `attend`: its time, the game parameters and the areas with an extra operation (the game's
//!   cache, below), no matching server (no online battles), no terminal card to reissue.
//! - The data commands (`get_*_info`, `get_*_sttng`, `ng_*`): their values from the game's own
//!   cache of its last answers, the dump's `ram/databank` (JSON; the parameters: the game's
//!   structures), their time now.
//! - Every command: the keys of its answer from the game's own tables (`WAL_SEGANU_KANCOLLE_SERVER=
//!   <commands>,<keys>,<lists>`: the RVAs of the command table, 0x30-byte entries with the name
//!   at +0x20; the key table, 0x372 entries of 72 bytes: name, flags (bit 0 a list), type (0 int,
//!   1 string, 2 time, 3 bool), minimum, maximum, default as text; the commands' key lists, a
//!   request's then an answer's, each ended by 0x372), each with its default (lists empty): the
//!   keys the data commands' cache does not hold.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::json::Json;
use wal_payload_common::log;

const KEY: [u8; 16] = [0x0c, 0x09, 0xb4, 0xc2, 0x44, 0x1d, 0x10, 0x1b, 0x58, 0xcb, 0x7c, 0xa6, 0xe5, 0x4c, 0x9b, 0xe8];

// ── MessagePack: the subset the game uses ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    Map(Vec<(String, Value)>),
}

fn read_value(b: &[u8], at: &mut usize) -> Option<Value> {
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let s = b.get(*at..*at + n)?;
        *at += n;
        Some(s)
    };
    let be = |s: &[u8]| s.iter().fold(0u64, |v, &x| v << 8 | x as u64);
    let t = *take(at, 1)?.first()?;
    Some(match t {
        0x00..=0x7f => Value::Int(t as i64),
        0xe0..=0xff => Value::Int(t as i8 as i64),
        0xc0 => Value::Nil,
        0xc2 => Value::Bool(false),
        0xc3 => Value::Bool(true),
        0xcc..=0xcf => {
            let n = 1 << (t - 0xcc);
            Value::Int(be(take(at, n)?) as i64)
        }
        0xd0..=0xd3 => {
            let n = 1 << (t - 0xd0);
            let v = be(take(at, n)?);
            let shift = 64 - 8 * n as u32;
            Value::Int(((v << shift) as i64) >> shift)
        }
        0xa0..=0xbf | 0xd9..=0xdb => {
            let n = match t {
                0xd9 => be(take(at, 1)?) as usize,
                0xda => be(take(at, 2)?) as usize,
                0xdb => be(take(at, 4)?) as usize,
                _ => (t & 0x1f) as usize,
            };
            Value::Str(String::from_utf8_lossy(take(at, n)?).into_owned())
        }
        0x90..=0x9f | 0xdc | 0xdd => {
            let n = match t {
                0xdc => be(take(at, 2)?) as usize,
                0xdd => be(take(at, 4)?) as usize,
                _ => (t & 0x0f) as usize,
            };
            Value::Array((0..n).map(|_| read_value(b, at)).collect::<Option<_>>()?)
        }
        0x80..=0x8f | 0xde | 0xdf => {
            let n = match t {
                0xde => be(take(at, 2)?) as usize,
                0xdf => be(take(at, 4)?) as usize,
                _ => (t & 0x0f) as usize,
            };
            let mut m = Vec::with_capacity(n);
            for _ in 0..n {
                let Value::Str(k) = read_value(b, at)? else { return None };
                m.push((k, read_value(b, at)?));
            }
            Value::Map(m)
        }
        _ => return None,
    })
}

fn write_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Nil => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Int(i) => {
            let i = *i;
            if (0..=0x7f).contains(&i) {
                out.push(i as u8);
            } else if (-32..0).contains(&i) {
                out.push(i as i8 as u8);
            } else if (i32::MIN as i64..=i32::MAX as i64).contains(&i) {
                out.push(0xd2);
                out.extend((i as i32).to_be_bytes());
            } else {
                out.push(0xd3);
                out.extend(i.to_be_bytes());
            }
        }
        Value::Str(s) => {
            let n = s.len();
            if n < 32 {
                out.push(0xa0 | n as u8);
            } else if n < 256 {
                out.extend([0xd9, n as u8]);
            } else {
                out.push(0xda);
                out.extend((n as u16).to_be_bytes());
            }
            out.extend(s.as_bytes());
        }
        Value::Array(a) => {
            if a.len() < 16 {
                out.push(0x90 | a.len() as u8);
            } else {
                out.push(0xdc);
                out.extend((a.len() as u16).to_be_bytes());
            }
            a.iter().for_each(|v| write_value(out, v));
        }
        Value::Map(m) => {
            if m.len() < 16 {
                out.push(0x80 | m.len() as u8);
            } else {
                out.push(0xde);
                out.extend((m.len() as u16).to_be_bytes());
            }
            for (k, v) in m {
                write_value(out, &Value::Str(k.clone()));
                write_value(out, v);
            }
        }
    }
}

// ── the body: IV, AES-128-CBC, zlib ──────────────────────────────────────────────────────

fn decode(body: &[u8]) -> Option<Value> {
    if body.len() < 32 || body.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let mut prev: [u8; 16] = body[..16].try_into().ok()?;
    let mut plain = Vec::with_capacity(body.len() - 16);
    for block in body[16..].chunks(16) {
        let mut b = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut b);
        plain.extend(b.iter().zip(prev).map(|(x, p)| x ^ p));
        prev.copy_from_slice(block);
    }
    let pad = *plain.last()? as usize;
    if (1..=16).contains(&pad) {
        plain.truncate(plain.len() - pad);
    }
    let packed = miniz_oxide::inflate::decompress_to_vec_zlib(&plain).ok()?;
    read_value(&packed, &mut 0)
}

fn encode(v: &Value) -> Vec<u8> {
    let mut packed = Vec::new();
    write_value(&mut packed, v);
    let mut plain = miniz_oxide::deflate::compress_to_vec_zlib(&packed, 6);
    let pad = 16 - plain.len() % 16;
    plain.extend(std::iter::repeat_n(pad as u8, pad));
    let mut iv = [0u8; 16];
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    iv.iter_mut().enumerate().for_each(|(i, b)| *b = (seed >> (i % 8 * 8)) as u8 ^ (i as u8).wrapping_mul(0x5b));
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let mut out = iv.to_vec();
    let mut prev = iv;
    for block in plain.chunks(16) {
        let mut b = GenericArray::clone_from_slice(&block.iter().zip(prev).map(|(x, p)| x ^ p).collect::<Vec<_>>());
        cipher.encrypt_block(&mut b);
        prev.copy_from_slice(&b);
        out.extend_from_slice(&b);
    }
    out
}

// ── the game's tables ────────────────────────────────────────────────────────────────────

struct Key {
    name: String,
    list: bool,
    kind: u64,
    min: i64,
    max: i64,
    default: String,
}

struct Tables {
    /// The commands' answer keys, by name.
    answers: Vec<(String, Vec<u32>)>,
    keys: Vec<Key>,
}

unsafe fn c_string(p: usize) -> String {
    if p < 0x10000 {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }.to_string_lossy().into_owned()
}

fn tables() -> Option<&'static Tables> {
    static TABLES: std::sync::OnceLock<Option<Tables>> = std::sync::OnceLock::new();
    TABLES
        .get_or_init(|| {
            let spec = std::env::var("WAL_SEGANU_KANCOLLE_SERVER").ok()?;
            let rvas: Vec<usize> = spec.split(',').filter_map(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()).collect();
            let [cmds, keys, lists] = rvas[..] else { return None };
            let base = wal_payload_common::iat::game_module();
            let mut names = Vec::new();
            for i in 0..256 {
                let name = unsafe { c_string(*((base + cmds + 0x30 * i + 0x20) as *const usize)) };
                if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit()) {
                    break;
                }
                names.push(name);
            }
            let keys: Vec<Key> = (0..0x372)
                .map(|i| {
                    let e = unsafe { std::slice::from_raw_parts((base + keys + 72 * i) as *const u64, 9) };
                    unsafe { Key { name: c_string(e[2] as usize), list: e[3] & 1 != 0, kind: e[4], min: e[5] as i64, max: e[6] as i64, default: c_string(e[7] as usize) } }
                })
                .collect();
            // the lists: the values before each end mark, after the zeros between them (`cmd` is 0,
            // each list's first)
            let mut found: Vec<Vec<u32>> = Vec::new();
            let words = unsafe { std::slice::from_raw_parts((base + lists) as *const u32, 0x100000 / 4) };
            let mut run: Vec<u32> = Vec::new();
            for &w in words {
                if w == 0x372 {
                    let start = run.iter().rposition(|&v| v == 0).map_or(0, |p| p + 1);
                    let mut list = vec![0];
                    list.extend(&run[start..]);
                    found.push(list);
                    run.clear();
                    if found.len() == 2 * names.len() {
                        break;
                    }
                } else {
                    run.push(w);
                }
            }
            if found.len() != 2 * names.len() {
                log!("kcserver: tables: {} commands, {} lists", names.len(), found.len());
                return None;
            }
            let answers = names.into_iter().enumerate().map(|(i, n)| (n, found[2 * i + 1].clone())).collect::<Vec<_>>();
            log!("kcserver: tables: {} commands, {} keys", answers.len(), keys.len());
            Some(Tables { answers, keys })
        })
        .as_ref()
}

/// A key's default value (a list: empty; the names ending in `_lst` / `_list` are lists too,
/// their flags 0).
fn default_value(k: &Key) -> Value {
    if k.list || k.name.ends_with("_lst") || k.name.ends_with("_list") {
        return Value::Array(Vec::new());
    }
    match k.kind {
        0 => Value::Int(k.default.trim().parse::<i64>().unwrap_or(0).clamp(k.min, k.max.max(k.min))),
        3 => Value::Bool(k.default.trim() == "1"),
        _ => Value::Str(k.default.clone()),
    }
}

// ── the dump's cached answers ────────────────────────────────────────────────────────────

/// The data commands' values from the game's own cache of its last answers
/// (`ram/databank/<name>_SDBZ_<version>_<crc>_<time>.dat`, JSON): command, its time key, then
/// each key and its file.
const DATABANK: &[(&str, &str, &[(&str, &str)])] = &[
    ("ng_word", "ngw_lst_lut", &[("ngw_lst", "NgWord")]),
    ("ng_eqpmnt", "nge_lst_lut", &[("nge_lst", "NgEquipment")]),
    ("ng_room_item", "ngri_lst_lut", &[("ngri_lst", "NgRoomItem")]),
    ("get_chr_opn_sttng", "cos_lut", &[("cos_lst_lst", "CharaOpenSetting")]),
    ("get_tc_opn_sttng", "tos_lut", &[("tos_lst_lst", "TcOpenSetting")]),
    ("get_room_item_opn_sttng", "rios_lst_lut", &[("rios_lst_lst", "RoomItemOpenSetting")]),
    ("get_strtgy_pt_dev_eqpmnt_sttng", "strtgy_pt_dev_eqpmnt_sttng_lut", &[("strtgy_pt_dev_eqpmnt_sttng_lst_lst", "StrategyPointDevEquipmentSetting")]),
    (
        "get_result_point_ranking",
        "rpr_lut",
        &[
            ("rpr_kind_list", "ResultPointRankingrpr-kind"),
            ("rpr_info_list_list", "ResultPointRankingrpr-info"),
            ("rpr_play_data_id_list_list", "ResultPointRankingrpr-play-data-id"),
            ("rpr_name_list_list", "ResultPointRankingrpr-name"),
            ("rpr_title_id_list_list", "ResultPointRankingrpr-title-id"),
            ("rpr_result_point_list_list", "ResultPointRankingrpr-result-point"),
            ("rpr_kou_medal_count_list_list", "ResultPointRankingrpr-kou-medal-count"),
        ],
    ),
    (
        "get_quest_holding_info",
        "qhi_lut",
        &[
            ("qhi_int_list_list", "QuestHoldingInfoqhi-int"),
            ("qhi_str_list_list", "QuestHoldingInfoqhi-str"),
            ("qhi_dg_list", "QuestHoldingInfoqhi-grp"),
            ("qhi_dm_list", "QuestHoldingInfoqhi-msg"),
        ],
    ),
    (
        "get_tlop_holding_info",
        "tlophi_lut",
        &[
            ("tlop_ctrl_int_lst", "TlopHoldingInfotlophi-ci"),
            ("tlop_ctrl_str_lst", "TlopHoldingInfotlophi-cs"),
            ("tlop_area_data_lst_lst", "TlopHoldingInfotlophi-ad"),
            ("tlop_rwrd_data_lst_lst", "TlopHoldingInfotlophi-rd"),
            ("tlop_open_period_id_lst", "TlopHoldingInfotlophi-opi"),
        ],
    ),
    (
        "get_cop_holding_info",
        "cophi_lut",
        &[
            ("cop_ctrl_int_lst", "CopHoldingInfocophi-ci"),
            ("cop_ctrl_str_lst", "CopHoldingInfocophi-cs"),
            ("cop_area_data_lst_lst", "CopHoldingInfocophi-ad"),
            ("cop_rwrd_indv_data_lst_lst", "CopHoldingInfocophi-rid"),
            ("cop_rwrd_indv_enty_lst_lst", "CopHoldingInfocophi-ri"),
            ("cop_srt_cnt_prize_data_lst", "CopHoldingInfocophi-scpd"),
        ],
    ),
    (
        "get_ssop_holding_info",
        "ssophi_lut",
        &[
            ("ssop_ctrl_int_lst", "SsopHoldingInfossophi-ci"),
            ("ssop_ctrl_str_lst", "SsopHoldingInfossophi-cs"),
            ("ssop_area_data_lst_lst", "SsopHoldingInfossophi-ad"),
            ("ssop_rwrd_data_lst_lst", "SsopHoldingInfossophi-rd"),
        ],
    ),
    ("get_blueprint_info", "bpi_lut", &[("blueprint_info_v2_list", "BlueprintInfo")]),
    ("get_bmbsp_info", "bmbspi_lut", &[("bmbsp_info_lst", "BlueprintMkBStampPeriodInfo")]),
    ("get_exercise_hold_info", "ehi_lut", &[("exercise_hold_info_list", "ExerciseHoldInfo_0")]),
    ("get_chr_mrrg_sttng", "cms_lut", &[("cms_lst", "CharaMarriageSetting_0")]),
    ("get_exch_dev_eqpmnt_recipe", "exch_dev_eqp_recipe_lut", &[("exch_dev_eqp_recipe_lst", "ExchDevEquipmentRecipe")]),
    ("get_epfes_holding_info", "epfeshi_lut", &[("epfes_holding_info", "EpfesHoldingInfo")]),
    ("get_mfes_holding_info", "mfeshi_lut", &[("mfes_holding_info", "MfesHoldingInfo")]),
    ("get_unit_costume_info", "uci_lut", &[("unit_costume_info_list", "UnitCostumeInfo")]),
    ("get_season_info", "si_lut", &[("season_info_list", "SeasonInfo")]),
    ("get_drop_scene_info", "dsi_lut", &[("drop_scene_info_list", "DropSceneInfo")]),
    // its area data: all empty lists in the cache (the defaults)
    ("get_area_mod_data", "amd_lut", &[]),
];

/// A cached answer's file (`ram/databank/<name>_SDBZ...`).
fn databank(name: &str) -> Option<Vec<u8>> {
    let prefix = format!("{name}_SDBZ");
    let entry = std::fs::read_dir("ram/databank").ok()?.flatten().find(|e| e.file_name().to_string_lossy().starts_with(&prefix))?;
    std::fs::read(entry.path()).ok()
}

fn from_json(j: &Json) -> Value {
    match j {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Bool(*b),
        Json::Num(n) => Value::Int(n.parse().unwrap_or(0)),
        Json::Str(s) => Value::Str(s.clone()),
        Json::Arr(a) => Value::Array(a.iter().map(from_json).collect()),
        Json::Obj(o) => Value::Map(o.iter().map(|(k, v)| (k.clone(), from_json(v))).collect()),
    }
}

fn databank_json(name: &str) -> Option<Value> {
    let data = databank(name)?;
    Json::parse(std::str::from_utf8(&data).ok()?).map(|j| from_json(&j))
}

/// An attendance parameter list from the game's cached structure (`<name>`, 32-bit integers):
/// the list's items are its fields `fields` (the order of the game's parser, 0x1414344f0).
fn attend_params(name: &str, fields: &[usize]) -> Value {
    let data = databank(name).unwrap_or_default();
    let field = |i: usize| data.get(4 * i..4 * i + 4).map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()) as i64);
    Value::Array(fields.iter().map(|&i| Value::Int(field(i))).collect())
}

fn get<'a>(m: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The time now, in the game's format (UTC).
fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The data's time (`*_lut`): the payload's start, the same in every answer (`ping` gives them
/// all; the game asks again for the data whose time changed).
fn lut() -> Value {
    static LUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    Value::Str(LUT.get_or_init(now).clone())
}

/// A closing window in the past (start, end), in the game's format: none to come.
fn past_window() -> Value {
    Value::Array(vec![Value::Str("2000-01-01T00:00:00+09:00".into()), Value::Str("2000-01-01T00:00:01+09:00".into())])
}

/// The answer to a title server request.
pub fn answer(path: &str, body: &[u8]) -> Vec<u8> {
    let Some(Value::Map(req)) = decode(body) else {
        log!("kcserver: {path}: request not decoded ({} bytes)", body.len());
        return Vec::new();
    };
    let cmd = match get(&req, "cmd") {
        Some(Value::Str(c)) => c.clone(),
        _ => String::new(),
    };
    let mut res: Vec<(String, Value)> = vec![
        ("cmd".into(), Value::Str(cmd.clone())),
        ("req_id".into(), get(&req, "req_id").cloned().unwrap_or(Value::Str(String::new()))),
        ("stat".into(), Value::Str("ok".into())),
    ];
    // the answer's other keys, their defaults (the overrides below replace them)
    if let Some(t) = tables()
        && let Some((_, ids)) = t.answers.iter().find(|(n, _)| *n == cmd)
    {
        for &id in ids {
            let Some(k) = t.keys.get(id as usize) else { continue };
            if !res.iter().any(|(n, _)| *n == k.name) {
                res.push((k.name.clone(), default_value(k)));
            }
        }
    }
    let set = |res: &mut Vec<(String, Value)>, key: &str, v: Value| match res.iter_mut().find(|(n, _)| n == key) {
        Some(e) => e.1 = v,
        None => res.push((key.into(), v)),
    };
    match cmd.as_str() {
        "game_init" => {
            set(&mut res, "db_close_timestamp", past_window());
            set(&mut res, "exercise_maintenance_timestamp", past_window());
            set(&mut res, "dny_db_cls_gst_ply", Value::Bool(false));
            set(&mut res, "retry_time", Value::Int(0));
            set(&mut res, "ovi_kind", Value::Int(0));
        }
        "ping" => {
            // the data's times (unchanged), no closing
            for (key, value) in res.iter_mut() {
                if key.ends_with("_lut") {
                    *value = lut();
                }
            }
            set(&mut res, "db_close_timestamp", past_window());
            set(&mut res, "exercise_maintenance_timestamp", past_window());
            set(&mut res, "ovi_kind", Value::Int(0));
        }
        "attend" => {
            // the attendance's time and parameters (none), no matching server (no online
            // battles), no terminal card to reissue
            set(&mut res, "atnd_lut", lut());
            // the game parameters (its parser reads each list's items without checking their
            // count), the areas with an extra operation
            let etc: Vec<usize> = (0..12).chain([13, 14, 15, 22]).chain(16..22).chain(23..28).collect();
            let balance: Vec<usize> = (0..12).chain([14, 15, 16, 12, 13]).chain(17..27).collect();
            set(&mut res, "atnd_prm1", attend_params("EtcParam", &etc));
            set(&mut res, "atnd_prm2", attend_params("DispersalParam", &[0, 1]));
            set(&mut res, "atnd_prm3", attend_params("GameBalanceParam", &balance));
            set(&mut res, "eo_enable_lst", databank_json("NormalAreaExtraOperationEnableList").unwrap_or(Value::Array(Vec::new())));
            set(&mut res, "matching_server_hostname", Value::Str(String::new()));
            set(&mut res, "matching_server_tcp_port", Value::Int(0));
            set(&mut res, "matching_server_ping_port", Value::Int(0));
            set(&mut res, "tc_reissue_a_code_lst", Value::Array(Vec::new()));
            set(&mut res, "tc_reissue_kc_sral", get(&req, "kc_serial").cloned().unwrap_or(Value::Str("A69E01A8888".into())));
        }
        _ => {
            if let Some((_, time_key, keys)) = DATABANK.iter().find(|(c, _, _)| *c == cmd) {
                set(&mut res, time_key, lut());
                for (key, name) in keys.iter() {
                    match databank_json(name) {
                        Some(v) => set(&mut res, key, v),
                        None => log!("kcserver: {cmd}: no cached {name}"),
                    }
                }
            }
        }
    }
    log!("kcserver: {cmd}: {:?}", req.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>());
    encode(&Value::Map(res))
}
