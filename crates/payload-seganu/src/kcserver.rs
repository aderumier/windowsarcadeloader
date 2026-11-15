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

use std::sync::Mutex;

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
    /// A list's minimum length.
    count: usize,
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
                    unsafe { Key { name: c_string(e[2] as usize), list: e[3] & 1 != 0, count: (e[3] >> 32 & 0xffff) as usize, kind: e[4], min: e[5] as i64, max: e[6] as i64, default: c_string(e[7] as usize) } }
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
            // WAL_SEGANU_KANCOLLE_SERVER_LOG=tables: every command's request keys
            if std::env::var("WAL_SEGANU_KANCOLLE_SERVER_LOG").is_ok_and(|v| v.split(',').any(|c| c.trim() == "tables")) {
                for (i, n) in names.iter().enumerate() {
                    let request: Vec<&str> = found[2 * i].iter().filter_map(|&k| keys.get(k as usize)).map(|k| k.name.as_str()).collect();
                    log!("kcserver: table {n}: {request:?}");
                }
            }
            let answers = names.into_iter().enumerate().map(|(i, n)| (n, found[2 * i + 1].clone())).collect::<Vec<_>>();
            log!("kcserver: tables: {} commands, {} keys", answers.len(), keys.len());
            Some(Tables { answers, keys })
        })
        .as_ref()
}

/// A key's default value (a list: its minimum length of default items; the names
/// ending in `_lst` / `_list` are lists too, their flags 0; a string at least its minimum
/// length, padded with `0`).
fn default_value(k: &Key) -> Value {
    if k.list && k.count > 0 {
        let item = Key { name: String::new(), list: false, count: 0, kind: k.kind, min: k.min, max: k.max, default: k.default.clone() };
        return Value::Array(vec![default_value(&item); k.count]);
    }
    if k.list || k.name.ends_with("_lst") || k.name.ends_with("_list") {
        return Value::Array(Vec::new());
    }
    match k.kind {
        // a command's result (`*_result`, `*_rslt`, -999..1): 1, done (0 and below the game's
        // errors)
        0 if k.max == 1 && k.min < 0 && (k.name.ends_with("result") || k.name.ends_with("rslt")) => Value::Int(1),
        0 => Value::Int(k.default.trim().parse::<i64>().unwrap_or(0).clamp(k.min, k.max.max(k.min))),
        3 => Value::Bool(k.default.trim() == "1"),
        1 if k.default.len() < k.min as usize => Value::Str(format!("{:0<1$}", k.default, k.min as usize)),
        _ => Value::Str(k.default.clone()),
    }
}

// ── the game's own data ──────────────────────────────────────────────────────────────────

/// A value of the game's data lists (`rom/<name>.farc`: a gzip member of `kind.id.key=value`
/// lines), as a number.
fn game_value(name: &str, key: &str) -> Option<i64> {
    static LISTS: Mutex<Vec<(String, std::collections::HashMap<String, String>)>> = Mutex::new(Vec::new());
    let mut lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    if !lists.iter().any(|(n, _)| n == name) {
        let map = game_list(name).unwrap_or_default();
        log!("kcserver: rom\\{name}.farc: {} values", map.len());
        lists.push((name.to_string(), map));
    }
    let (_, map) = lists.iter().find(|(n, _)| n == name)?;
    map.get(key)?.trim().parse().ok()
}

fn game_list(name: &str) -> Option<std::collections::HashMap<String, String>> {
    let farc = std::fs::read(format!("rom\\{name}.farc")).ok()?;
    let gz = farc.windows(3).position(|w| w == [0x1f, 0x8b, 8])?;
    let flags = *farc.get(gz + 3)?;
    let mut at = gz + 10;
    if flags & 4 != 0 {
        at += 2 + u16::from_le_bytes([*farc.get(at)?, *farc.get(at + 1)?]) as usize;
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            at += farc.get(at..)?.iter().position(|&b| b == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    let text = miniz_oxide::inflate::decompress_to_vec(farc.get(at..)?).ok()?;
    Some(String::from_utf8_lossy(&text).lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect())
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

/// An event's progress, none (ids -1, counts 0, lists empty, times past).
fn event_progress(key: &str) -> Option<Value> {
    let fields: &[&str] = match key {
        "cop_progress_info" => &["cop_id", "total_cp", "sortie_cnt_total", "trc_frame_num", "use_trc_frame_num", "ttrl_flg_array", "lst_thrgh_slctr_tm", "area_id", "area_sub_id_lst", "area_max_clr_rnk_lst", "area_sortie_cnt_lst", "area_aurid_flg_lst", "prize_exch_inf_lst"],
        "epfes_progress_info" => &["epfes_id", "tutorial_flag", "item_get_num", "item_use_num", "prize_exchange_info_list"],
        "mfes_progress_info" => &["tutorial_flag", "item_get_num", "item_use_num", "mfes_id", "clear_mission_id", "mfes_mission_info_list"],
        _ => return None,
    };
    Some(Value::Map(
        fields
            .iter()
            .map(|&f| {
                let v = if f.ends_with("_lst") || f.ends_with("_list") || f.ends_with("_array") || f == "tutorial_flag" {
                    Value::Array(Vec::new())
                } else if f.ends_with("_tm") {
                    Value::Str("2005-01-01T00:00:00+09:00".into())
                } else if f.ends_with("_id") {
                    Value::Int(-1)
                } else {
                    Value::Int(0)
                };
                (f.to_string(), v)
            })
            .collect(),
    ))
}

/// The admirals registered on this cabinet: one file per card (its Aime id), the name.
const ADMIRALS: &str = "kcserver";

/// The name of the admiral registered for a card.
fn admiral(aime_id: i64) -> Option<String> {
    std::fs::read_to_string(format!("{ADMIRALS}\\{aime_id}.txt")).ok().filter(|n| !n.is_empty())
}

/// The admiral's saved play (`kcserver\<id>.msgpack`): the game's last `area_result` or `end`
/// request, the whole state it uploads.
fn saved_play(id: i64) -> Option<Vec<(String, Value)>> {
    let bytes = std::fs::read(format!("{ADMIRALS}\\{id}.msgpack")).ok()?;
    match read_value(&bytes, &mut 0)? {
        Value::Map(m) => Some(m),
        _ => None,
    }
}

fn save_play(req: &[(String, Value)]) {
    let Some(id) = get(req, "pd_id").and_then(int).filter(|&id| id >= 0) else { return };
    let mut out = Vec::new();
    write_value(&mut out, &Value::Map(req.to_vec()));
    let saved = std::fs::create_dir_all(ADMIRALS).and_then(|_| std::fs::write(format!("{ADMIRALS}\\{id}.msgpack"), out));
    log!("kcserver: play of admiral {id} saved: {saved:?}");
}

/// Two values of the same shape (a saved value replaces a default only then).
fn same_kind(a: &Value, b: &Value) -> bool {
    matches!((a, b), (Value::Int(_), Value::Int(_)) | (Value::Bool(_), Value::Bool(_)) | (Value::Str(_), Value::Str(_)) | (Value::Array(_), Value::Array(_)) | (Value::Map(_), Value::Map(_)))
}

/// The admiral's values of an answer from its saved play: each key `x` from the upload's
/// `x_lst` (its first item: the uploads list one value per admiral).
fn restore(res: &mut [(String, Value)], play: &[(String, Value)]) {
    for (key, value) in res.iter_mut() {
        if let Some(Value::Array(a)) = get(play, &format!("{key}_lst"))
            && let Some(v) = a.first()
            && same_kind(v, value)
        {
            *value = v.clone();
        }
    }
}

/// A sortie's drop: a card of the game's list (gm_trc_list, the ones not ignored), at random.
fn drop_card() -> Option<i64> {
    let list = game_list("gm_trc_list")?;
    let mut ids: Vec<i64> = list
        .iter()
        .filter(|(k, v)| k.ends_with(".ignore") && v.trim() == "0")
        .filter_map(|(k, _)| k.split('.').nth(1)?.parse().ok())
        .filter(|id| list.get(&format!("trc.{id}.chara_id")).and_then(|c| c.trim().parse::<i64>().ok()).is_some_and(|c| c > 0))
        .collect();
    ids.sort();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as usize);
    ids.get(seed.wrapping_mul(2654435761) % ids.len().max(1)).copied()
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
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
    // the events' progress (objects the game reads key by key: 0x140f63dd2 cop, 0x140f67087
    // epfes, 0x140f68f20 mfes): none
    for (key, value) in res.iter_mut() {
        if let Some(progress) = event_progress(key) {
            *value = progress;
        }
    }
    // the sortie's scene: its GPU fault traced from here (WAL_NVGL_SYNC)
    if cmd == "start_sortie" {
        wal_payload_common::nvgl::sync_trace();
    }
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
        "pre_start" => {
            // the card's admiral to confirm, or none: the game's registration of a new one (its
            // switch at 0x140f887d0: 1 an admiral, -3 the scene asking to register, 0 its
            // error 92)
            match get(&req, "aime_id").and_then(int).and_then(|id| admiral(id).map(|name| (id, name))) {
                Some((id, name)) => {
                    set(&mut res, "ps_result", Value::Int(1));
                    set(&mut res, "pd_id", Value::Int(id));
                    set(&mut res, "admrl_name", Value::Str(name));
                    // its saved play: experience, flagship, tutorials seen (0 / 1 each)...
                    if let Some(play) = saved_play(id) {
                        restore(&mut res, &play);
                    }
                }
                None => set(&mut res, "ps_result", Value::Int(-3)),
            }
        }
        "pd_registration" => {
            // the new admiral, kept for the card (1: registered)
            if let (Some(id), Some(Value::Str(name))) = (get(&req, "aime_id").and_then(int), get(&req, "admrl_name")) {
                let saved = std::fs::create_dir_all(ADMIRALS).and_then(|_| std::fs::write(format!("{ADMIRALS}\\{id}.txt"), name));
                log!("kcserver: admiral {name} registered for Aime id {id}: {saved:?}");
                set(&mut res, "cd_adm_result", Value::Int(if saved.is_ok() { 1 } else { 0 }));
            }
        }
        "call_up" => {
            // the ships of the cards laid on the deck, one per card (cu_rslt 1, 0x140f892a7):
            // none known yet (-1), the game's own values of the card's ship
            let cards = match get(&req, "tc_tid_lst") {
                Some(Value::Array(a)) => a.len(),
                _ => 0,
            };
            set(&mut res, "cu_rslt", Value::Int(1));
            let tc_ids: Vec<i64> = match get(&req, "tc_id_lst") {
                Some(Value::Array(a)) => a.iter().map(|v| int(v).unwrap_or(-1)).collect(),
                _ => Vec::new(),
            };
            // a new ship: the card's (gm_trc_list chara_id), its HP, fuel and ammo full
            // (gm_unit_list i_life, fuel_max, amm_max)
            let ship = |i: usize, key: &str| -> Option<i64> {
                let chara = game_value("gm_trc_list", &format!("trc.{}.chara_id", tc_ids.get(i)?))?;
                let field = match key {
                    "chr_id_lst" => return Some(chara),
                    // the HP as a ratio of the full HP, 10000 full (its maximum; a card play: i_life,
                    // 15, showed 1/15, 0 a sunk ship)
                    "chr_hp_lst" => return Some(10000),
                    "chr_fuel_lst" => "fuel_max",
                    "chr_ammo_lst" => "amm_max",
                    _ => return None,
                };
                game_value("gm_unit_list", &format!("unit.{chara}.{field}"))
            };
            // its 5 slots (the game reads each card's list of them, 0x140f896d2): its equipment
            // (gm_unit_list eq00.., eq_num of them, the others -1), the planes on them (cap00..)
            let slots = |i: usize, key: &str| -> Value {
                let chara = tc_ids.get(i).and_then(|tc| game_value("gm_trc_list", &format!("trc.{tc}.chara_id")));
                let unit = |field: String| chara.and_then(|c| game_value("gm_unit_list", &format!("unit.{c}.{field}")));
                let equipped = unit("eq_num".into()).unwrap_or(0);
                Value::Array(
                    (0..5)
                        .map(|slot| match key {
                            "chr_slt_eqpmnt_lst" if slot < equipped => Value::Int(unit(format!("eq{slot:02}")).unwrap_or(-1)),
                            "chr_slt_eqpmnt_lst" => Value::Int(-1),
                            "chr_slt_amount_lst" => Value::Int(unit(format!("cap{slot:02}")).unwrap_or(0)),
                            _ => Value::Int(0),
                        })
                        .collect(),
                )
            };
            // a ship already played: its values in the admiral's saved play (each `chr_x_lst`
            // from `chr_x_lst_lst`, the play's ships' lists; the ship found by its id)
            let play = get(&req, "pd_id").and_then(int).and_then(saved_play);
            let played = |i: usize, key: &str| -> Option<Value> {
                let play = play.as_ref()?;
                let chara = game_value("gm_trc_list", &format!("trc.{}.chara_id", tc_ids.get(i)?))?;
                let first = |k: &str| match get(play, k) {
                    Some(Value::Array(a)) => match a.first() {
                        Some(Value::Array(ships)) => Some(ships.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                let j = first("chr_id_lst_lst")?.iter().position(|v| int(v) == Some(chara))?;
                first(&format!("{key}_lst"))?.get(j).cloned()
            };
            for (key, value) in res.iter_mut() {
                let Some(k) = tables().and_then(|t| t.keys.iter().find(|k| k.name == *key)) else { continue };
                if !key.starts_with("chr_") || !matches!(value, Value::Array(_)) {
                    continue;
                }
                let items = (0..cards).map(|i| match key.as_str() {
                    "chr_slt_eqpmnt_lst" | "chr_slt_eqpmnt_imprv_lv_lst" | "chr_slt_amount_lst" => slots(i, key),
                    // the other lists of each card (blueprints, equipment history): none
                    _ if !k.list => Value::Array(Vec::new()),
                    "chr_is_lv_cp_rlsd_lst" => Value::Bool(false),
                    "chr_strt_idx_lst" | "chr_pd_sync_idx_lst" | "chr_mrg_sts_lst" => Value::Int(-1),
                    k => Value::Int(ship(i, k).unwrap_or(if k == "chr_id_lst" { -1 } else { 0 })),
                });
                let items = items.enumerate().map(|(i, new)| match played(i, key) {
                    Some(v) if same_kind(&v, &new) && key != "chr_id_lst" => v,
                    _ => new,
                });
                *value = Value::Array(items.collect());
            }
        }
        "area_result" => {
            // the sortie's result: the play saved, the admiral's values as uploaded
            save_play(&req);
            restore(&mut res, &req);
        }
        "end" => save_play(&req),
        "sortie_drop" | "start_sortie" => {
            // the admiral's values (else 0: its resources emptied), a card dropped
            if let Some(play) = get(&req, "pd_id").and_then(int).and_then(saved_play) {
                restore(&mut res, &play);
            }
            // the card a sortie can drop: chosen at its start (the game writes it on the card
            // in the printer then, the start_sortie answer's tc_id), the same at its drop (the
            // card's picture)
            static DROP: Mutex<Option<i64>> = Mutex::new(None);
            let mut chosen = DROP.lock().unwrap_or_else(|e| e.into_inner());
            if cmd == "start_sortie" {
                *chosen = drop_card();
            }
            if let Some(tc) = *chosen {
                if cmd == "sortie_drop" {
                    log!("kcserver: card {tc} dropped");
                }
                set(&mut res, "tc_id", Value::Int(tc));
            }
        }
        "start" => {
            // the play started (0x140f88938: 1 goes on, 0 / -1 give the card back), the
            // card's admiral
            set(&mut res, "start_result", Value::Int(1));
            if let Some(id) = get(&req, "pd_id").and_then(int).filter(|&id| id >= 0) {
                set(&mut res, "pd_id", Value::Int(id));
                if let Some(name) = admiral(id) {
                    set(&mut res, "admrl_name", Value::Str(name));
                }
                // its saved play: resources, experience, flagship...
                if let Some(play) = saved_play(id) {
                    restore(&mut res, &play);
                }
                // the play's card flag (WAL_SEGANU_KANCOLLE_CARD_FLAG, the RVA of the play data's
                // byte 1: 0x142382420 + 1), never set by the game with this server; clear, the play
                // is a play without a card: no credit spent, no area_result, its end no_card_end
                // (0x140a9ac59), nothing saved
                if let Some(rva) = std::env::var("WAL_SEGANU_KANCOLLE_CARD_FLAG").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) {
                    unsafe { std::ptr::write_volatile((wal_payload_common::iat::game_module() + rva) as *mut u8, 1) };
                    log!("kcserver: card flag set");
                }
            }
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
    // the whole request and answer of the commands named in WAL_SEGANU_KANCOLLE_SERVER_LOG
    if std::env::var("WAL_SEGANU_KANCOLLE_SERVER_LOG").is_ok_and(|v| v.split(',').any(|c| c.trim() == cmd || c.trim() == "all")) {
        log!("kcserver: {cmd} request {req:?}");
        log!("kcserver: {cmd} answer {res:?}");
    }
    encode(&Value::Map(res))
}
