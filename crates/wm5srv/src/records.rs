//! The time attack records, the shop rankings and the car search, after Bayshore_legacy's
//! `games/time_attack.ts`, `time_attack.ts`, `resource/ranking.ts`, `game.ts` (game history) and
//! `terminal.ts` (car summary).

use crate::pb::{Message, Value, Writer};
use crate::rpc::{now, schema, set};
use crate::store;

const ERR_SUCCESS: u64 = 0;

/// RankingType: the versus stars, the trophies (after the time attack courses).
const RANKING_VS_STAR: u64 = 100;
const RANKING_TROPHY: u64 = 101;

/// The time attack records of `course` (all when `None`), fastest first.
fn ta_records(course: Option<u64>) -> Vec<(u64, Message)> {
    let mut records: Vec<(u64, Message)> = store::all("ta").into_iter().filter(|(_, r)| course.is_none() || r.uint(3) == course).collect();
    records.sort_by_key(|(_, r)| r.uint(4).unwrap_or(u64::MAX));
    records
}

/// A finished time attack (`ta`: SaveGameResultRequest.TimeAttackResult): the car's record on
/// the course when it is its best.
pub(crate) fn save_time_attack(car_id: u64, car_record: &Message, ta: &Message) {
    let (Some(time), Some(course)) = (ta.uint(1), ta.uint(2)) else { return };
    let current = ta_records(Some(course)).into_iter().find(|(_, r)| r.uint(1) == Some(car_id));
    if let Some((_, r)) = &current {
        if r.uint(4).is_some_and(|best| best <= time) {
            return;
        }
    }
    let id = current.map_or_else(|| store::new_id("ta"), |(id, _)| id);
    let car = car_record.message(2).unwrap_or_default();
    let tune = schema().car_tune_power;
    let mut w = Writer::new().uint(1, car_id).uint(2, car.uint(6).unwrap_or(0)).uint(3, course).uint(4, time);
    for section in 0..7 {
        w = w.uint(5 + section, ta.uint(3 + section).unwrap_or(0));
    }
    w = w.uint(12, car.uint(tune).unwrap_or(0)).uint(13, car.uint(tune + 1).unwrap_or(0));
    store::save("ta", id, &Message::parse(&w.0).unwrap_or_default());
    crate::log(&format!("wm5srv: car {car_id}: time attack course {course} best {time}"));
}

/// LoadTimeAttackRecordResponse: the course's 100 best times (all, the car's model), the car's
/// best and its sections.
pub(crate) fn load_time_attack_record(req: &Message) -> Writer {
    let course = req.uint(3).unwrap_or(0);
    let model = req.uint(2);
    let records = ta_records(Some(course));
    let mut w = Writer::new().uint(1, ERR_SUCCESS);
    for (_, r) in records.iter().take(100) {
        w = w.uint(2, r.uint(4).unwrap_or(0));
    }
    for (_, r) in records.iter().filter(|(_, r)| r.uint(2) == model).take(100) {
        w = w.uint(3, r.uint(4).unwrap_or(0));
    }
    if let Some((_, best)) = records.iter().find(|(_, r)| req.uint(1).is_some() && r.uint(1) == req.uint(1)) {
        w = w.uint(4, best.uint(4).unwrap_or(0));
        for section in 0..7 {
            w = w.uint(5 + section, best.uint(5 + section).unwrap_or(0));
        }
    }
    w
}

/// LoadGameHistoryResponse: the car's time attack bests (their ranks among all cars and its
/// model's), its ghost battle counts (5DX: no stamp sheet).
pub(crate) fn load_game_history(req: &Message) -> Writer {
    let car_id = req.uint(1);
    let record = car_id.and_then(|id| store::load("cars", id)).unwrap_or_default();
    let mut w = Writer::new().uint(1, ERR_SUCCESS);
    for (_, mine) in ta_records(None).into_iter().filter(|(_, r)| r.uint(1) == car_id) {
        let course = ta_records(mine.uint(3));
        let model: Vec<&(u64, Message)> = course.iter().filter(|(_, r)| r.uint(2) == mine.uint(2)).collect();
        let rank = |list: Vec<&(u64, Message)>| list.iter().position(|(_, r)| r.uint(1) == car_id).map_or(0, |p| p as u64 + 1);
        let whole_rank = rank(course.iter().collect());
        let model_rank = rank(model.clone());
        w = w.message(
            2,
            Writer::new()
                .uint(1, mine.uint(3).unwrap_or(0))
                .uint(2, mine.uint(4).unwrap_or(0))
                .uint(3, mine.uint(12).unwrap_or(0))
                .uint(4, mine.uint(13).unwrap_or(0))
                .uint(5, whole_rank)
                .uint(6, course.len() as u64)
                .uint(7, model_rank)
                .uint(8, model.len() as u64),
        );
    }
    // the ghost battle play and win counts follow the versus counters
    let rg = schema().vs + 10;
    w = w.uint(3, now()).uint(5, record.uint(rg).unwrap_or(0)).uint(6, record.uint(rg + 1).unwrap_or(0));
    if schema().game != crate::Game::Wm5 {
        w = w.uint(7, 0);
    }
    w
}

/// A Ranking.Entry of a car: its level as the rank, `result`, its name, region, model, colors,
/// `tune` (power, handling), title, level.
fn entry(car_id: u64, car: &Message, result: u64, tune: (u64, u64)) -> Writer {
    let s = schema();
    let level = car.uint(s.car_level).unwrap_or(0);
    let mut w = Writer::new()
        .uint(1, level)
        .uint(2, result)
        .uint(3, car_id)
        .string(4, &car.string(4).unwrap_or_default())
        .uint(5, car.uint(3).unwrap_or(1).max(1))
        .uint(6, car.uint(6).unwrap_or(0))
        .uint(7, car.uint(7).unwrap_or(0))
        .uint(8, car.uint(8).unwrap_or(0))
        .uint(9, tune.0)
        .uint(10, tune.1);
    w = match s.car_title {
        Some(f) => w.string(11, &car.string(f).unwrap_or_default()),
        None => w.uint(11, car.uint(30).unwrap_or(0)),
    };
    w.uint(12, level)
}

/// Ranking: per time attack course its 10 best cars, the 20 cars with the most versus stars
/// and the most maxi coins (the trophies); an empty list without record.
pub(crate) fn ranking() -> Vec<u8> {
    let s = schema();
    let cars = store::all("cars");
    let car = |id: u64| cars.iter().find(|(c, _)| *c == id).and_then(|(_, r)| r.message(2));
    let mut w = Writer::new();
    for course in 0..schema().ta_courses {
        let mut list = Writer::new().uint(1, course);
        for (_, r) in ta_records(Some(course)).iter().take(10) {
            let car_id = r.uint(1).unwrap_or(0);
            if let Some(c) = car(car_id) {
                list = list.message(2, entry(car_id, &c, r.uint(4).unwrap_or(0), (r.uint(12).unwrap_or(0), r.uint(13).unwrap_or(0))));
            }
        }
        w = w.message(1, list);
    }
    for (kind, field) in [(RANKING_VS_STAR, s.vs + 2), (RANKING_TROPHY, s.maxi_coin)] {
        let mut best: Vec<&(u64, Message)> = cars.iter().filter(|(_, r)| r.uint(field).unwrap_or(0) > 0).collect();
        best.sort_by_key(|(_, r)| std::cmp::Reverse(r.uint(field).unwrap_or(0)));
        let mut list = Writer::new().uint(1, kind);
        for (car_id, r) in best.into_iter().take(20) {
            let c = r.message(2).unwrap_or_default();
            let tune = (c.uint(s.car_tune_power).unwrap_or(0), c.uint(s.car_tune_power + 1).unwrap_or(0));
            list = list.message(2, entry(*car_id, &c, r.uint(field).unwrap_or(0), tune));
        }
        w = w.message(1, list);
    }
    w.0
}

/// A `%XX` / `+` encoded query value.
fn url_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(c) => {
                        out.push(c);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// CarSummary: the cars whose name starts or ends with `name` (`last_played_place_id`: every
/// car, this server is one shop), at most `limit` (10).
pub(crate) fn car_summary(query: &str) -> Vec<u8> {
    let param = |key: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| url_decode(v)));
    let limit = param("limit").and_then(|l| l.parse().ok()).unwrap_or(10);
    let name = param("name");
    let by_place = param("last_played_place_id").is_some();
    let mut cars = Vec::new();
    for (car_id, record) in store::all("cars") {
        let Some(mut car) = record.message(2) else { continue };
        let car_name = car.string(4).unwrap_or_default();
        let hit = by_place || name.as_ref().is_some_and(|n| !n.is_empty() && (car_name.starts_with(n.as_str()) || car_name.ends_with(n.as_str())));
        if !hit {
            continue;
        }
        if car.uint(3).unwrap_or(0) == 0 {
            set(&mut car, 3, Value::Varint(car_id % 47 + 1));
        }
        cars.push(car);
        if cars.len() >= limit {
            break;
        }
    }
    let mut w = Writer::new().uint(1, cars.len() as u64);
    for car in &cars {
        w = w.bytes(2, &car.encode());
    }
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode() {
        assert_eq!(url_decode("%E6%B9%BE+a%2"), "湾 a%2");
    }
}
