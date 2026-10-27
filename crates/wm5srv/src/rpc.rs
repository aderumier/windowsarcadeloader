//! The game's RPCs (`/method/<name>`) and resources (`/resource/<name>`), after Bayshore_5's
//! modules (`startup.ts`, `users.ts`, `cars.ts`, `game.ts`).

use crate::pb::{Message, Value, Writer};
use crate::store;

const ERR_SUCCESS: u64 = 0;
const ERR_REQUEST: u64 = 1;
const ERR_NOT_FOUND: u64 = 2;

/// The game's feature version: 100, May 2014 (as its terminal's).
fn feature_version() -> Writer {
    Writer::new().uint(1, 100).uint(2, 2014).uint(3, 5).uint(4, 0).uint(5, 0)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `m` with `field` set to `value` (its previous values replaced).
fn set(m: &mut Message, field: u32, value: Value) {
    m.merge(&Message(vec![(field, value)]));
}

/// The sub-message `field` of `m` with the fields of `update` replacing its own.
fn merge_sub(m: &mut Message, field: u32, update: &Message) {
    let mut sub = m.message(field).unwrap_or_default();
    sub.merge(update);
    set(m, field, Value::Bytes(sub.encode()));
}

pub(crate) fn method(name: &str, body: &[u8]) -> Vec<u8> {
    let req = Message::parse(body).unwrap_or_default();
    if crate::verbose() {
        crate::log(&format!("wm5srv: {name} <- {}", req.dump()));
    }
    let _lock = store::lock();
    let res = match name {
        "register_system_info" => register_system_info(&req),
        "register_system_stats" => Writer::new().uint(1, ERR_SUCCESS),
        "ping" => Writer::new().uint(1, ERR_SUCCESS).uint(2, req.uint(1).unwrap_or(1)),
        "load_user" => load_user(&req),
        "create_user" => create_user(&req),
        "update_user_lock" => Writer::new().uint(1, ERR_SUCCESS),
        "load_drive_information" => Writer::new().uint(1, ERR_SUCCESS),
        "create_car" => create_car(&req),
        "load_car" => load_car(&req),
        "update_car" => update_car(&req),
        "save_game_result" => save_game_result(&req),
        _ => {
            crate::log(&format!("wm5srv: {name}: not implemented <- {}", req.dump()));
            Writer::new().uint(1, ERR_SUCCESS)
        }
    };
    if crate::verbose() {
        crate::log(&format!("wm5srv: {name} -> {}", Message::parse(&res.0).unwrap_or_default().dump()));
    }
    res.0
}

/// RegisterSystemInfoResponse: the place and region the cabinet reported, no campaign, no team
/// suspension, no face recognition.
fn register_system_info(req: &Message) -> Writer {
    Writer::new()
        .uint(1, ERR_SUCCESS)
        .uint(2, req.uint(9).unwrap_or(1))
        .string(3, &req.string(10).unwrap_or_else(|| "JPN0123".into()))
        .uint(4, 0)
        .uint(5, 0)
        .uint(6, i32::MAX as u64)
        .uint(7, i32::MAX as u64)
        .uint(8, 0)
        .message(9, feature_version())
}

/// The user of a request's access code (`field`), created for a new card when `create`.
fn user(req: &Message, field: u32, chip_field: u32, create: bool) -> Option<(u64, Message)> {
    let code = req.string(field).filter(|c| !c.is_empty())?;
    if let Some(found) = store::find_user(&code) {
        return Some(found);
    }
    if !create {
        return None;
    }
    let id = store::new_id("users");
    let user = Message::parse(
        &Writer::new()
            .uint(1, id)
            .string(2, &req.string(chip_field).unwrap_or_default())
            .string(3, &code)
            .uint(4, 0)
            .uint(5, 0)
            .uint(6, 0)
            .uint(8, 0)
            .uint(9, 0)
            .0,
    )
    .unwrap_or_default();
    store::save("users", id, &user);
    crate::log(&format!("wm5srv: new user {id} (card {code})"));
    Some((id, user))
}

/// LoadUserResponse: the card's user and cars (a new card: a new user without car).
fn load_user(req: &Message) -> Writer {
    let Some((id, user)) = user(req, 2, 1, true) else {
        return Writer::new()
            .uint(1, ERR_REQUEST)
            .uint(7, 0)
            .uint(8, 0)
            .uint(9, 0)
            .uint(10, 0)
            .uint(13, 0)
            .uint(14, 0)
            .uint(15, 0)
            .uint(21, 0)
            .uint(22, 0);
    };
    let max = req.uint(8).unwrap_or(u64::MAX) as usize;
    let car_ids = user.uints(7);
    let mut res = Writer::new()
        .uint(1, ERR_SUCCESS)
        .string(3, &user.string(3).unwrap_or_default())
        .uint(4, 1)
        .uint(6, id)
        .uint(7, user.uint(4).unwrap_or(0))
        .uint(8, user.uint(9).unwrap_or(0))
        .uint(9, user.uint(8).unwrap_or(0))
        .uint(10, car_ids.len() as u64);
    let cars: Vec<Message> = car_ids.iter().take(max).filter_map(|c| store::load("cars", *c)).collect();
    for car in &cars {
        res = res.bytes(11, car.bytes(2).unwrap_or_default());
    }
    for _ in &cars {
        // no opponent ghost, no rename, not deleted, no event
        res = res.message(12, Writer::new().uint(1, 0).uint(3, 0).uint(4, 0).uint(5, 0));
    }
    let coupon = if car_ids.is_empty() { 0 } else { 1 };
    res.uint(13, coupon)
        .uint(14, user.uint(6).unwrap_or(0))
        .uint(15, user.uint(5).unwrap_or(0))
        .uint(21, 0)
        .uint(22, 0)
}

fn create_user(req: &Message) -> Writer {
    match user(req, 3, 2, true) {
        Some((id, user)) => Writer::new().uint(1, ERR_SUCCESS).string(2, &user.string(3).unwrap_or_default()).uint(5, id),
        None => Writer::new().uint(1, ERR_NOT_FOUND).uint(5, 0),
    }
}

/// The required fields of a car, before the game's own.
fn car_defaults() -> Message {
    let mut w = Writer::new();
    for field in [9, 10, 11, 12, 13, 14, 16, 17, 18, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31] {
        w = w.uint(field, 0);
    }
    Message::parse(&w.0).unwrap_or_default()
}

/// CreateCarResponse: a new car (the game's), its settings and counters, first in the user's
/// car order.
fn create_car(req: &Message) -> Writer {
    let found = match req.uint(1) {
        Some(id) if id != 0 => store::load("users", id).map(|u| (id, u)),
        _ => user(req, 4, 3, true),
    };
    let Some((user_id, mut user)) = found else {
        return Writer::new().uint(1, ERR_NOT_FOUND).uint(5, 0).uint(6, 0);
    };
    let car_id = store::new_id("cars");
    let mut car = car_defaults();
    car.merge(&req.message(8).unwrap_or_default());
    let fixed = Writer::new()
        .uint(1, car_id)
        .uint(2, user_id)
        .uint(3, car.uint(3).unwrap_or(1).max(1))
        .uint(36, req.uint(11).unwrap_or_else(now))
        .uint(40, car.uint(40).unwrap_or(1).max(1))
        .string(41, &car.string(41).unwrap_or_else(|| "JPN".into()));
    car.merge(&Message::parse(&fixed.0).unwrap_or_default());
    // view, transmission, no retire, meter 0, navigation map, volume 2, bgm 0, nameplate 0
    let setting = Writer::new()
        .uint(1, car_id)
        .uint(2, 1)
        .uint(3, req.uint(9).unwrap_or(0))
        .uint(4, 0)
        .uint(5, 0)
        .uint(6, 1)
        .uint(7, 2)
        .uint(8, 0)
        .uint(9, 0)
        .uint(10, 0);
    let mut record = Writer::new().bytes(2, &car.encode()).uint(3, 0).uint(4, 0).uint(5, 0).uint(6, 0).uint(7, 0).message(8, setting);
    // the versus, ghost battle and story counters
    for field in 10..=36 {
        if field != 24 {
            record = record.uint(field, 0);
        }
    }
    record = record.uint(46, 0);
    store::save("cars", car_id, &Message::parse(&record.0).unwrap_or_default());
    let mut order = vec![car_id];
    order.extend(user.uints(7));
    user.0.retain(|(f, _)| *f != 7);
    user.0.extend(order.into_iter().map(|c| (7, Value::Varint(c))));
    user.0.sort_by_key(|(f, _)| *f);
    store::save("users", user_id, &user);
    crate::log(&format!("wm5srv: user {user_id}: new car {car_id}"));
    Writer::new().uint(1, ERR_SUCCESS).string(2, &user.string(3).unwrap_or_default()).uint(5, user_id).uint(6, car_id)
}

/// LoadCarResponse: the car record as stored.
fn load_car(req: &Message) -> Writer {
    match req.uint(1).and_then(|id| store::load("cars", id)) {
        Some(record) => Writer::new().uint(1, ERR_SUCCESS).raw(&record.encode()),
        None => {
            crate::log(&format!("wm5srv: load_car: no car {:?}", req.uint(1)));
            Writer::new().uint(1, ERR_NOT_FOUND)
        }
    }
}

/// The user owning a car record.
fn car_user(record: &Message) -> Option<(u64, Message)> {
    let id = record.message(2)?.uint(2)?;
    Some((id, store::load("users", id)?))
}

/// The user's maxi gold, 600 HP cards, tutorials, shop grade from a request (`fields`: maxi
/// gold, 600 HP cards, tutorials, shop grade).
fn update_user(user: &mut Message, req: &Message, fields: [Option<u32>; 4]) {
    let [gold, hp600, tutorials, grade] = fields;
    if let Some(v) = gold.and_then(|f| req.uint(f)) {
        set(user, 4, Value::Varint(v));
        let total = user.uint(9).unwrap_or(0).max(v);
        set(user, 9, Value::Varint(total));
    }
    if let Some(v) = hp600.and_then(|f| req.uint(f)) {
        set(user, 6, Value::Varint(v));
    }
    if let Some(f) = tutorials {
        let bits = req.uints(f).into_iter().fold(user.uint(5).unwrap_or(0), |b, t| b | t);
        set(user, 5, Value::Varint(bits));
    }
    if let Some(v) = grade.and_then(|f| req.uint(f)) {
        set(user, 8, Value::Varint(v));
    }
}

/// Appends the items of `field` of `req` to the car's owned items (45).
fn add_items(record: &mut Message, req: &Message, field: u32) {
    for item in req.all(field).cloned().collect::<Vec<_>>() {
        record.0.push((45, item));
    }
    record.0.sort_by_key(|(f, _)| *f);
}

fn update_car(req: &Message) -> Writer {
    let Some(car_id) = req.uint(1) else { return Writer::new().uint(1, ERR_REQUEST) };
    let Some(mut record) = store::load("cars", car_id) else { return Writer::new().uint(1, ERR_NOT_FOUND) };
    if let Some(car) = req.message(2) {
        merge_sub(&mut record, 2, &car);
    }
    if let Some(setting) = req.message(5) {
        merge_sub(&mut record, 8, &setting);
    }
    add_items(&mut record, req, 4);
    if let Some(v) = req.uint(10) {
        set(&mut record, 46, Value::Varint(v));
    }
    if let Some((user_id, mut user)) = car_user(&record) {
        update_user(&mut user, req, [Some(8), Some(3), None, None]);
        if req.uint(9) == Some(1) {
            // deleted: out of the user's cars
            user.0.retain(|(f, v)| !(*f == 7 && *v == Value::Varint(car_id)));
            crate::log(&format!("wm5srv: user {user_id}: car {car_id} deleted"));
        }
        store::save("users", user_id, &user);
    }
    store::save("cars", car_id, &record);
    Writer::new().uint(1, ERR_SUCCESS)
}

/// SaveGameResultRequest: the car, its settings and counters after a game.
fn save_game_result(req: &Message) -> Writer {
    let Some(car_id) = req.uint(1) else { return Writer::new().uint(1, ERR_REQUEST) };
    let Some(mut record) = store::load("cars", car_id) else { return Writer::new().uint(1, ERR_NOT_FOUND) };
    let mut car = req.message(7).unwrap_or_default();
    if let Some(at) = req.uint(3) {
        set(&mut car, 36, Value::Varint(at));
    }
    merge_sub(&mut record, 2, &car);
    if let Some(setting) = req.message(8) {
        merge_sub(&mut record, 8, &setting);
    }
    // request field -> car record field
    let copy = |record: &mut Message, from: &Message, map: &[(u32, u32)]| {
        for &(src, dst) in map {
            if let Some(v) = from.get(src).cloned() {
                set(record, dst, v);
            }
        }
    };
    copy(&mut record, req, &[(4, 5), (9, 4), (11, 6)]);
    add_items(&mut record, req, 13);
    if let Some(st) = req.message(19) {
        copy(&mut record, &st, &[(1, 29), (2, 3), (3, 30), (4, 31), (5, 32), (6, 33), (7, 34), (8, 35)]);
        let wins = record.uint(35).unwrap_or(0);
        if wins > record.uint(36).unwrap_or(0) {
            set(&mut record, 36, Value::Varint(wins));
        }
    }
    if let Some(vs) = req.message(21) {
        copy(&mut record, &vs, &[(6, 10), (7, 11), (8, 12), (9, 14), (10, 15), (11, 16), (12, 17), (13, 18), (14, 19)]);
        let stars = record.uint(12).unwrap_or(0);
        if stars > record.uint(13).unwrap_or(0) {
            set(&mut record, 13, Value::Varint(stars));
        }
    }
    if let Some(rg) = req.message(22) {
        copy(&mut record, &rg, &[(7, 20), (8, 21), (9, 22), (10, 23), (11, 24), (12, 25)]);
    }
    if let Some((user_id, mut user)) = car_user(&record) {
        update_user(&mut user, req, [Some(17), Some(10), Some(12), Some(18)]);
        store::save("users", user_id, &user);
    }
    store::save("cars", car_id, &record);
    Writer::new().uint(1, ERR_SUCCESS)
}

pub(crate) fn resource(name: &str, query: &str) -> Vec<u8> {
    if crate::verbose() {
        crate::log(&format!("wm5srv: resource {name}?{query}"));
    }
    match name {
        // PlaceList: this shop
        "place_list" => Writer::new()
            .message(1, Writer::new().string(1, "JPN0123").string(2, "Arcade").uint(3, 1).string(4, "JPN"))
            .0,
        // Ranking: a list of every ranking type (time attack courses 0..23, versus stars,
        // trophies), no record yet
        "ranking" => {
            let mut w = Writer::new();
            for kind in (0..=23).chain([100, 101]) {
                w = w.message(1, Writer::new().uint(1, kind));
            }
            w.0
        }
        // CrownList: no crown holder, a placeholder crown per run area (the attract mode shows
        // them by index): Bayshore_5's areas 0..14 and 18, 18 the twelfth
        "crown_list" => {
            let mut areas: Vec<u64> = (0..=14).collect();
            areas.insert(11, 18);
            let mut w = Writer::new();
            for area in areas {
                w = w.message(1, Writer::new().uint(1, 999_999_999 - area).uint(2, area).uint(3, 0));
            }
            w.0
        }
        // FileList: no file to download (the next check in an hour)
        "file_list" => Writer::new().uint(2, 3600).0,
        _ => {
            crate::log(&format!("wm5srv: resource {name}?{query}: not implemented"));
            Vec::new()
        }
    }
}
