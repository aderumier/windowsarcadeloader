//! The players' documents: one JSON object per Aime user id in `Y:\idac\<id>.json`, holding
//! what ARTEMiS keeps in its `idac_*` tables (`profile`, `avatar`, `config`, `mode_rank`,
//! `stock`, `tickets`, `cars`, `courses`, `trials`, `stories`, `challenges`, `vs_info`,
//! `theory*`, `tips`, `stamps`, `lottery`). Requests merge into it; the server's lock covers a
//! whole request (load, change, save). Also the car id helpers ARTEMiS's titles counts use.

use std::sync::Mutex;

use wal_payload_common::json::Json;

/// Held for each request.
pub(super) static LOCK: Mutex<()> = Mutex::new(());

const DIR: &str = "Y:\\idac";

fn path(user: u64) -> String {
    format!("{DIR}\\{user}.json")
}

pub(super) fn load(user: u64) -> Option<Json> {
    std::fs::read_to_string(path(user)).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_)))
}

pub(super) fn save(user: u64, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(user));
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path(user));
    }
}

/// Every player (the rankings, the theory rivals), `current` in place of its saved file.
pub(super) fn players(current: Option<(u64, &Json)>) -> Vec<(u64, Json)> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(DIR) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json").and_then(|n| n.parse::<u64>().ok()) else { continue };
            if current.is_some_and(|(c, _)| c == id) {
                continue;
            }
            if let Some(doc) = load(id) {
                out.push((id, doc));
            }
        }
    }
    if let Some((id, doc)) = current {
        out.push((id, doc.clone()));
    }
    out
}

/// `doc`'s object `key` (created empty).
pub(super) fn obj<'a>(doc: &'a mut Json, key: &str) -> &'a mut Json {
    if !matches!(doc.get(key), Some(Json::Obj(_))) {
        doc.set(key, Json::obj());
    }
    doc.get_mut(key).expect("idac: document is an object")
}

/// `doc`'s array `key` (created empty).
pub(super) fn arr<'a>(doc: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    if !matches!(doc.get(key), Some(Json::Arr(_))) {
        doc.set(key, Vec::<Json>::new());
    }
    match doc.get_mut(key) {
        Some(Json::Arr(items)) => items,
        _ => unreachable!("idac: document is an object"),
    }
}

/// Copies `src`'s fields into the object `dst` (nulls skipped, empty strings too with
/// `skip_empty`: ARTEMiS's stock never clears a list), except `except`.
pub(super) fn merge(dst: &mut Json, src: &Json, skip_empty: bool, except: &[&str]) {
    if let Json::Obj(fields) = src {
        for (k, v) in fields {
            if matches!(v, Json::Null) || (skip_empty && v.as_str() == Some("")) || except.contains(&k.as_str()) {
                continue;
            }
            dst.set(k, v.clone());
        }
    }
}

/// `src`'s `key`, or `default`.
pub(super) fn val(src: &Json, key: &str, default: impl Into<Json>) -> Json {
    match src.get(key) {
        None | Some(Json::Null) => default.into(),
        Some(v) => v.clone(),
    }
}

/// An object of `src`'s `fields` in that order (numbers, with their defaults).
pub(super) fn fields(src: &Json, fields: &[(&str, i64)]) -> Json {
    let mut out = Json::obj();
    for (k, d) in fields {
        out.set(k, val(src, k, *d));
    }
    out
}

/// An object of `src`'s `keys` that it has (no defaults: ARTEMiS's optional fields).
pub(super) fn pick(src: &Json, keys: &[&str]) -> Json {
    let mut out = Json::obj();
    for k in keys {
        if let Some(v) = src.get(k).filter(|v| !matches!(v, Json::Null)) {
            out.set(k, v.clone());
        }
    }
    out
}

/// The item of `list` whose `key` is `value`.
pub(super) fn find_mut<'a>(list: &'a mut [Json], key: &str, value: i64) -> Option<&'a mut Json> {
    list.iter_mut().find(|j| j.get(key).is_some() && j.i(key) == value)
}

/// Puts `item` in `list`, replacing the one with the same `keys` (a table's unique key).
pub(super) fn upsert(list: &mut Vec<Json>, keys: &[&str], item: Json) {
    match list.iter_mut().find(|j| keys.iter().all(|k| j.i(k) == item.i(k))) {
        Some(old) => *old = item,
        None => list.push(item),
    }
}

/// Saves `tickets` (`ticket_id`, `ticket_cnt`) over the player's.
pub(super) fn put_tickets(doc: &mut Json, tickets: &[Json]) {
    let list = arr(doc, "tickets");
    for t in tickets {
        upsert(list, &["ticket_id"], Json::obj().with("ticket_id", val(t, "ticket_id", 0)).with("ticket_cnt", val(t, "ticket_cnt", 0)));
    }
}

/// Takes `count` tickets of `id` (a full tune ticket, avatar tokens...).
pub(super) fn use_tickets(doc: &mut Json, id: i64, count: i64) {
    let list = arr(doc, "tickets");
    match find_mut(list, "ticket_id", id) {
        Some(t) => {
            let left = (t.i("ticket_cnt") - count).max(0);
            t.set("ticket_cnt", left);
        }
        None => list.push(Json::obj().with("ticket_id", id).with("ticket_cnt", 0i64)),
    }
}

/// The request's stock lists and tickets (ARTEMiS's `update_inventory`).
pub(super) fn update_inventory(doc: &mut Json, req: &Json) {
    if let Some(stock) = req.get("stock_obj") {
        merge(obj(doc, "stock"), stock, true, &[]);
    }
    put_tickets(doc, req.a("ticket_data"));
}

/// A car's numbers (ARTEMiS's `CarDto`; the use counts and mileage start at 0, the pickup and
/// purchase orders at 1).
const CAR_NUMS: [&str; 23] = [
    "car_id",
    "style_car_id",
    "color",
    "bureau",
    "kana",
    "s_no",
    "l_no",
    "car_flag",
    "tune_point",
    "tune_level",
    "tune_parts",
    "infinity_tune",
    "online_vs_win",
    "pickup_seq",
    "purchase_seq",
    "use_count",
    "story_use_count",
    "timetrial_use_count",
    "vs_use_count",
    "net_vs_use_count",
    "theory_use_count",
    "car_mileage",
    "equip_parts_count",
];

const CAR_STRS: [&str; 5] = ["color_stock_list", "color_stock_new_list", "parts_stock_list", "parts_stock_new_list", "parts_set_equip_list"];

/// A new car from a request's (`CreateAccount`'s `car_obj`, `BuyCarResult`).
pub(super) fn new_car(src: &Json) -> Json {
    let mut car = Json::obj();
    for k in CAR_NUMS {
        car.set(k, val(src, k, if matches!(k, "pickup_seq" | "purchase_seq") { 1i64 } else { 0 }));
    }
    for k in CAR_STRS {
        car.set(k, val(src, k, ""));
    }
    car.set("parts_list", val(src, "parts_list", Vec::<Json>::new()));
    car
}

/// A car for the game (`CarData`, `FactoryCarData` with `car_id`: no tune level), `id` the
/// player's.
pub(super) fn car_out(user: u64, car: &Json, with_car_id: bool) -> Json {
    let mut out = Json::obj().with("id", user);
    for k in CAR_NUMS {
        if k == "tune_level" || (k == "car_id" && !with_car_id) {
            continue;
        }
        out.set(k, val(car, k, if matches!(k, "pickup_seq" | "purchase_seq") { 1i64 } else { 0 }));
    }
    for k in CAR_STRS {
        out.set(k, val(car, k, ""));
    }
    out.set("parts_list", val(car, "parts_list", Vec::<Json>::new()));
    out
}

/// The player's car `style_car_id`.
pub(super) fn car_mut(doc: &mut Json, style_car_id: i64) -> Option<&mut Json> {
    find_mut(arr(doc, "cars"), "style_car_id", style_car_id)
}

// style_car_id: 2 bytes of tune style, maker, car

pub(super) fn car_id(style_car_id: i64) -> i64 {
    style_car_id & 0xffff
}

pub(super) fn car_maker(style_car_id: i64) -> i64 {
    (style_car_id & 0xff00) >> 8
}

/// DH 0, HC 1, AR 2 (the game's style byte has HC and AR swapped).
pub(super) fn car_style(style_car_id: i64) -> i64 {
    match (style_car_id & 0xff0000) >> 16 {
        0 => 0,
        1 => 2,
        _ => 1,
    }
}

/// The technical layout cars (ARTEMiS's `CAR_LAYOUT_MAPPING_FULL`), the others high speed.
const TECHNICAL: [i64; 28] = [
    0, 1, 2, 3, 4, 5, 9, 10, 13, 14, 258, 259, 261, 512, 513, 514, 515, 770, 771, 772, 1280, 1281, 1283, 1285, 1286, 1536, 1537, 1792,
];

pub(super) fn fulltune_count(cars: &[Json]) -> usize {
    cars.iter().filter(|c| c.i("tune_level") >= 16).count()
}

pub(super) fn car_layout_count(cars: &[Json]) -> Vec<Json> {
    let technical = cars.iter().filter(|c| TECHNICAL.contains(&car_id(c.i("style_car_id")))).count();
    vec![
        Json::obj().with("layout_id", 0u32).with("count", technical),
        Json::obj().with("layout_id", 1u32).with("count", cars.len() - technical),
    ]
}

pub(super) fn car_style_count(cars: &[Json]) -> Vec<Json> {
    (0..3i64)
        .map(|style| Json::obj().with("style_id", style).with("count", cars.iter().filter(|c| car_style(c.i("style_car_id")) == style).count()))
        .collect()
}

/// Cars per key, in first seen order.
fn count_by(cars: &[Json], name: &str, key: fn(i64) -> i64) -> Vec<Json> {
    let mut counts: Vec<(i64, usize)> = Vec::new();
    for c in cars {
        let k = key(c.i("style_car_id"));
        match counts.iter_mut().find(|(id, _)| *id == k) {
            Some(e) => e.1 += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.into_iter().map(|(k, n)| Json::obj().with(name, k).with("count", n)).collect()
}

pub(super) fn car_use_count(cars: &[Json]) -> Vec<Json> {
    count_by(cars, "car_id", car_id)
}

pub(super) fn maker_use_count(cars: &[Json]) -> Vec<Json> {
    count_by(cars, "maker_id", car_maker)
}

/// A random number (the theory rivals, the CPU avatars).
pub(super) fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(crate::clock::unix_now());
    h.finish()
}
