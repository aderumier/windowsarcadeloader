//! The game's RPCs (`/method/<name>`) and resources (`/resource/<name>`), after Bayshore_5's
//! modules (`startup.ts`, `users.ts`, `cars.ts`, `game.ts`). 5DX renumbered fields of its
//! messages ([`Schema`]: the field numbers of the game's own `service.proto`).

use crate::pb::{Message, Value, Writer};
use crate::store;

const ERR_SUCCESS: u64 = 0;
const ERR_REQUEST: u64 = 1;
const ERR_NOT_FOUND: u64 = 2;

/// GameMode
const MODE_TIME_ATTACK: u64 = 2;

/// The game served.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Game {
    Wm5,
    Wm5Dx,
    Wm5DxPlus,
    Wm6,
    Wm6R,
    Wm6RR,
}

/// The field numbers that differ between the versions: the car record (`LoadCarResponse`),
/// `Car`, `SaveGameResultRequest`.
pub(crate) struct Schema {
    pub(crate) game: Game,
    /// RankingType: the time attack courses.
    pub(crate) ta_courses: u64,
    /// LoadCarResponse: its required counters (before the game's own), the setting, odometer,
    /// play count, custom color, the first versus / ghost battle / story counters, the owned
    /// items, the aura motif auto change.
    pub(crate) record_required: &'static [u32],
    pub(crate) setting: u32,
    pub(crate) odometer: u32,
    pub(crate) play_count: u32,
    pub(crate) custom_color: u32,
    pub(crate) vs: u32,
    pub(crate) st: u32,
    pub(crate) items: u32,
    pub(crate) aura_auto: u32,
    /// The ghost battle result's fields -> the record's.
    pub(crate) rg_map: &'static [(u32, u32)],
    /// Car: its required fields (title a string in 5DX), last played time, ghost level, country.
    pub(crate) car_required: &'static [u32],
    pub(crate) car_title: Option<u32>,
    pub(crate) car_played_at: u32,
    pub(crate) car_ghost_level: u32,
    pub(crate) car_country: u32,
    /// Car: tune power, handling, level (the title: `car_title`, else uint 30 in 5).
    pub(crate) car_tune_power: u32,
    pub(crate) car_level: u32,
    /// LoadCarResponse: the maxi coins (the trophy ranking).
    pub(crate) maxi_coin: u32,
    /// SaveGameResultRequest: the time attack result.
    pub(crate) save_ta: u32,
    /// SaveGameResultRequest: maxi gold, shop grade (5DX: in the ghost battle result), story,
    /// versus and ghost battle results.
    pub(crate) save_gold: u32,
    pub(crate) save_grade: Option<u32>,
    pub(crate) save_st: u32,
    pub(crate) save_vs: u32,
    pub(crate) save_rg: u32,
    /// LoadCarResponse: the ghost battle stamp (1 for a new car, as Bayshore's).
    pub(crate) rg_stamp: Option<u32>,
    /// TutorialType: ids (6, the user's tutorials a list of flags), else bits.
    pub(crate) tutorial_ids: bool,
}

const WM5: Schema = Schema {
    game: Game::Wm5,
    ta_courses: 24,
    record_required: &[3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 46],
    setting: 8,
    odometer: 4,
    play_count: 5,
    custom_color: 6,
    vs: 10,
    st: 29,
    items: 45,
    aura_auto: 46,
    rg_map: &[(7, 20), (8, 21), (9, 22), (10, 23), (11, 24), (12, 25)],
    car_required: &[9, 10, 11, 12, 13, 14, 16, 17, 18, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31],
    car_title: None,
    car_played_at: 36,
    car_ghost_level: 40,
    car_country: 41,
    car_tune_power: 28,
    car_level: 31,
    maxi_coin: 22,
    save_ta: 20,
    save_gold: 17,
    save_grade: Some(18),
    save_st: 19,
    save_vs: 21,
    save_rg: 22,
    rg_stamp: None,
    tutorial_ids: false,
};

/// 5DX: shop points and grade per car (4, 5), the ghost battle stamp (54, 1 as Bayshore's),
/// past cleared regions (56), screenshots (53).
const WM5DX: Schema = Schema {
    game: Game::Wm5Dx,
    ta_courses: 24,
    record_required: &[3, 4, 5, 6, 7, 8, 9, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27, 28, 29, 30, 34, 35, 36, 37, 38, 39, 40, 41, 51, 53, 56],
    setting: 10,
    odometer: 6,
    play_count: 7,
    custom_color: 8,
    vs: 12,
    st: 34,
    items: 50,
    aura_auto: 51,
    rg_map: &[(8, 22), (9, 23), (10, 24), (11, 4), (12, 5), (13, 25), (14, 26), (15, 27), (23, 54)],
    car_required: &[9, 10, 11, 12, 13, 14, 16, 17, 18, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 34],
    car_title: Some(33),
    car_played_at: 45,
    car_ghost_level: 49,
    car_country: 50,
    car_tune_power: 31,
    car_level: 34,
    maxi_coin: 24,
    save_ta: 23,
    save_gold: 18,
    save_grade: None,
    save_st: 22,
    save_vs: 24,
    save_rg: 25,
    rg_stamp: Some(54),
    tutorial_ids: false,
};

/// 5DX+: 5DX's fields, a 25th time attack course.
const WM5DXP: Schema = Schema { game: Game::Wm5DxPlus, ta_courses: 25, ..WM5DX };

/// 6: 5DX+'s field numbers, many fields gone: no maxi gold, shop or maxi coins (24: the ghost
/// battle trophies), the ghost battle score, regions and dress-up level instead.
const WM6: Schema = Schema {
    game: Game::Wm6,
    record_required: &[3, 6, 7, 8, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 28, 29, 34, 35, 36, 37, 38, 40, 41, 51, 53, 59, 60, 63, 64],
    rg_map: &[(8, 22), (10, 28), (11, 29), (21, 59), (23, 27)],
    car_required: &[9, 10, 11, 12, 13, 14, 16, 24, 25, 26, 27, 28, 31, 32, 34],
    rg_stamp: Some(27),
    tutorial_ids: true,
    ..WM5DXP
};

/// 6R: 6's, the ghost battle's highway and 2-player versus counters (67..71), no transfer flag.
const WM6R: Schema = Schema {
    game: Game::Wm6R,
    record_required: &[3, 6, 7, 8, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 28, 29, 34, 35, 36, 37, 38, 40, 41, 51, 53, 59, 63, 64, 67, 68, 69, 70, 71],
    rg_map: &[(8, 22), (10, 28), (11, 29), (21, 59), (23, 27), (27, 24)],
    ..WM6
};

/// 6RR: 6R's.
const WM6RR: Schema = Schema { game: Game::Wm6RR, ..WM6R };

static SCHEMA: std::sync::OnceLock<&'static Schema> = std::sync::OnceLock::new();

pub(crate) fn set_game(game: Game) {
    let _ = SCHEMA.set(match game {
        Game::Wm5 => &WM5,
        Game::Wm5Dx => &WM5DX,
        Game::Wm5DxPlus => &WM5DXP,
        Game::Wm6 => &WM6,
        Game::Wm6R => &WM6R,
        Game::Wm6RR => &WM6RR,
    });
}

pub(crate) fn schema() -> &'static Schema {
    SCHEMA.get().copied().unwrap_or(&WM5)
}

/// The game's feature version: 100, May 2014 (as its terminal's).
fn feature_version() -> Writer {
    Writer::new().uint(1, 100).uint(2, 2014).uint(3, 5).uint(4, 0).uint(5, 0)
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `m` with `field` set to `value` (its previous values replaced).
pub(crate) fn set(m: &mut Message, field: u32, value: Value) {
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
        // no notice, nothing transferred
        "load_drive_information" => Writer::new().uint(1, ERR_SUCCESS).message(4, transfer_notice()),
        // the shop open, its new arrivals (no item for sale), its notice seen
        "load_shop_information" => Writer::new().uint(1, ERR_SUCCESS).uint(2, 1).uint(4, 5).uint(5, 1),
        // no time attack record, no ghost battle yet (5DX: no stamp)
        "load_game_history" => crate::records::load_game_history(&req),
        // no friend, challenger or history (5: has_history 6; 5DX: stamp sheets 7)
        "load_ghost_battle_info" => match schema().game {
            Game::Wm5 => Writer::new().uint(1, ERR_SUCCESS).uint(6, 0),
            Game::Wm5Dx | Game::Wm5DxPlus | Game::Wm6 => Writer::new().uint(1, ERR_SUCCESS).uint(7, 0),
            // 6R: not promoted to buddy
            Game::Wm6R | Game::Wm6RR => Writer::new().uint(1, ERR_SUCCESS).uint(7, 0).uint(13, 0),
        },
        // no online champion match running
        "load_ghost_competition_info" => Writer::new().uint(1, ERR_SUCCESS).uint(3, 1),
        // terminal: maxi gold receivable, nothing transferred (5DX: no feature announce)
        "load_terminal_information" => {
            let w = Writer::new().uint(1, ERR_SUCCESS);
            // 6: no maxi gold
            let w = if matches!(schema().game, Game::Wm6 | Game::Wm6R | Game::Wm6RR) { w } else { w.uint(9, 1) };
            let w = w.message(14, transfer_notice());
            match schema().game {
                Game::Wm5 => w,
                Game::Wm5Dx => w.uint(16, 0),
                // 5DX+: no prize waiting, the free scratch card used
                Game::Wm5DxPlus | Game::Wm6 | Game::Wm6R | Game::Wm6RR => w.uint(16, 0).uint(17, 0).uint(18, 1),
            }
        }
        // 6: the player's session, client logs, bookmarked ghosts, items to receive (none)
        "update_user_session" | "submit_client_log" | "save_bookmarks" | "load_bookmarks" | "load_unreceived_user_items" => Writer::new().uint(1, ERR_SUCCESS),
        "receive_maxi_gold" => receive_maxi_gold(&req),
        // no car campaign accepted
        "load_car_campaign_info" => Writer::new().uint(1, ERR_SUCCESS).uint(2, 1).uint(3, 0).uint(4, 0).uint(5, 0),
        // a card transfer from another server: the card's user (none: a new card)
        "start_transfer" => Writer::new().uint(1, ERR_SUCCESS).uint(2, 0),
        // 5DX+ scratch cards (not offered): an empty sheet
        "load_scratch_information" | "save_scratch_sheet" => Writer::new().uint(1, ERR_SUCCESS).uint(3, 0).uint(4, 0),
        // 6RR: bingo (no item to receive), Ghost Koshien (closed), bingo targets (normal path)
        "load_bingo_stats" => Writer::new().uint(1, ERR_SUCCESS).uint(5, 0).uint(6, 0),
        "load_ghost_koshien_info" => Writer::new().uint(1, ERR_SUCCESS).uint(3, 1).uint(10, 0),
        "load_ghost_bingo_targets" => Writer::new().uint(1, ERR_SUCCESS).uint(4, 4),
        "bingo_receivable_items" | "merge_bingo_card" | "load_ghost_koshien_ranking" => Writer::new().uint(1, ERR_SUCCESS),
        "load_time_attack_record" => crate::records::load_time_attack_record(&req),
        "save_terminal_result" => save_terminal_result(&req),
        "rename_car" => rename_car(&req),
        "ask_access_code" => ask_access_code(&req),
        "save_screenshot" => save_screenshot(&req),
        // no campaign, no car right transfer, no charge bookkeeping
        "grant_car_right" | "accept_car_campaign" | "save_car_campaign_info" | "save_charge" | "save_client_report" => Writer::new().uint(1, ERR_SUCCESS),
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

/// SaveTerminalResultRequest: the player's car order and confirmed tutorials.
fn save_terminal_result(req: &Message) -> Writer {
    if let Some((id, mut user)) = req.uint(1).and_then(|id| Some((id, store::load("users", id)?))) {
        let order = req.uints(3);
        if !order.is_empty() {
            user.0.retain(|(f, _)| *f != 7);
            user.0.extend(order.into_iter().map(|c| (7, Value::Varint(c))));
            user.0.sort_by_key(|(f, _)| *f);
        }
        update_user(&mut user, req, [None, None, Some(5), None]);
        store::save("users", id, &user);
    }
    Writer::new().uint(1, ERR_SUCCESS)
}

/// RenameCarRequest: the car's new name.
fn rename_car(req: &Message) -> Writer {
    let (Some(car_id), Some(name)) = (req.uint(1), req.string(2)) else { return Writer::new().uint(1, ERR_REQUEST) };
    let Some(mut record) = store::load("cars", car_id) else { return Writer::new().uint(1, ERR_NOT_FOUND) };
    merge_sub(&mut record, 2, &Message(vec![(4, Value::Bytes(name.into_bytes()))]));
    store::save("cars", car_id, &record);
    Writer::new().uint(1, ERR_SUCCESS)
}

/// AskAccessCodeResponse: the access code of a card chip id (empty: an unknown card).
fn ask_access_code(req: &Message) -> Writer {
    let chip = req.string(1).unwrap_or_default();
    let code = store::all("users").into_iter().find(|(_, u)| u.string(2).as_deref() == Some(chip.as_str())).and_then(|(_, u)| u.string(3));
    Writer::new().uint(1, ERR_SUCCESS).string(2, &code.unwrap_or_default())
}

/// SaveScreenshotRequest (5DX): counted in the car's screenshots (53), the image dropped.
fn save_screenshot(req: &Message) -> Writer {
    if let Some((car_id, mut record)) = req.uint(1).and_then(|id| Some((id, store::load("cars", id)?))) {
        let count = record.uint(53).unwrap_or(0) + 1;
        set(&mut record, 53, Value::Varint(count));
        store::save("cars", car_id, &record);
    }
    Writer::new().uint(1, ERR_SUCCESS)
}

/// TransferNotice: no data transferred, no car or team to rename.
fn transfer_notice() -> Writer {
    Writer::new().uint(1, 0).uint(2, 0).uint(3, 0)
}

/// ReceiveMaxiGoldResponse: the user's maxi gold, unchanged (no gold waiting on the terminal).
fn receive_maxi_gold(req: &Message) -> Writer {
    let gold = req.uint(1).and_then(|id| store::load("users", id)).and_then(|u| u.uint(4)).unwrap_or(0);
    Writer::new().uint(1, ERR_SUCCESS).uint(2, gold).uint(3, gold)
}

/// RegisterSystemInfoResponse: the place and region the cabinet reported, no campaign, no team
/// suspension, no face recognition (5DX: no report sending either).
fn register_system_info(req: &Message) -> Writer {
    let w = Writer::new()
        .uint(1, ERR_SUCCESS)
        .uint(2, req.uint(9).unwrap_or(1))
        .string(3, &req.string(10).unwrap_or_else(|| "JPN0123".into()));
    match schema().game {
        Game::Wm5 => w.uint(4, 0).uint(5, 0).uint(6, i32::MAX as u64).uint(7, i32::MAX as u64).uint(8, 0).message(9, feature_version()),
        Game::Wm5Dx | Game::Wm5DxPlus => w.uint(6, i32::MAX as u64).uint(7, i32::MAX as u64).uint(8, 0).uint(9, 0).message(10, feature_version()),
        // 6: no client log asked, the ghost selection's redo wait (Bayshore's 30..4000)
        Game::Wm6 | Game::Wm6R | Game::Wm6RR => w.message(10, feature_version()).uint(28, 30).uint(29, 4000),
    }
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
            .uint(10, 0)
            .uint(13, 0)
            .uint(14, 0)
            .uint(15, 0)
            .uint(21, 0)
            .uint(22, 0);
    };
    if matches!(schema().game, Game::Wm6 | Game::Wm6R | Game::Wm6RR) {
        return load_user_6(req, id, &user);
    }
    let max = req.uint(8).unwrap_or(u64::MAX) as usize;
    let car_ids = user.uints(7);
    let mut res = Writer::new()
        .uint(1, ERR_SUCCESS)
        .string(3, &user.string(3).unwrap_or_default())
        .uint(4, 1)
        .uint(6, id)
        .uint(7, user.uint(4).unwrap_or(0))
        .uint(8, user.uint(9).unwrap_or(0));
    if schema().game == Game::Wm5 {
        // the shop grade (5DX: per car)
        res = res.uint(9, user.uint(8).unwrap_or(0));
    }
    res = res.uint(10, car_ids.len() as u64);
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

/// 6's (and 6R's) LoadUserResponse: no maxi gold, coupons or 600 HP cards; the tutorials a list of flags
/// (the ghost stamp ones, 20..24, set: the game locks up without, Bayshore's note); smartphone
/// app not registered.
fn load_user_6(req: &Message, id: u64, user: &Message) -> Writer {
    let max = req.uint(8).unwrap_or(u64::MAX) as usize;
    let car_ids = user.uints(7);
    let mut res = Writer::new().uint(1, ERR_SUCCESS).string(3, &user.string(3).unwrap_or_default()).uint(4, 1).uint(6, id).uint(10, car_ids.len() as u64);
    let cars: Vec<Message> = car_ids.iter().take(max).filter_map(|c| store::load("cars", *c)).collect();
    for car in &cars {
        res = res.bytes(11, car.bytes(2).unwrap_or_default());
    }
    for _ in &cars {
        // no opponent ghost, not deleted, no event, not transferred
        res = res.message(12, Writer::new().uint(1, 0).uint(4, 0).uint(5, 0).uint(6, 0));
    }
    let bits = user.uint(5).unwrap_or(0) | (0b11111 << 20);
    for i in 0..36 {
        res = res.uint(15, (bits >> i) & 1);
    }
    res = res.uint(21, 0).uint(22, 0);
    if matches!(schema().game, Game::Wm6R | Game::Wm6RR) {
        // no full tuned car ticket; ghost versus 2, 3 players and highway unlocked
        res = res.uint(14, 0).uint(34, 0).uint(35, 0).uint(36, 0);
    }
    if schema().game == Game::Wm6RR {
        // no bingo played, no item received
        res = res.uint(38, 0).uint(42, 0).uint(44, 0);
    }
    res
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
    for &field in schema().car_required {
        w = w.uint(field, 0);
    }
    if let Some(title) = schema().car_title {
        w = w.string(title, "");
    }
    let mut m = Message::parse(&w.0).unwrap_or_default();
    m.0.sort_by_key(|(f, _)| *f);
    m
}

/// CreateCarResponse: a new car (the game's), its settings and counters, first in the user's
/// car order.
fn create_car(req: &Message) -> Writer {
    let s = schema();
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
        .uint(s.car_played_at, req.uint(11).unwrap_or_else(now))
        .uint(s.car_ghost_level, car.uint(s.car_ghost_level).unwrap_or(1).max(1))
        .string(s.car_country, &car.string(s.car_country).unwrap_or_else(|| "JPN".into()));
    car.merge(&Message::parse(&fixed.0).unwrap_or_default());
    // view, transmission, no retire, meter 0, navigation map, volume 2, bgm 0, nameplate 0
    // (5DX: terminal background 0)
    let mut setting = Writer::new()
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
    if s.game != Game::Wm5 {
        setting = setting.uint(13, 0);
    }
    // the counters (versus, ghost battle, story...)
    let mut record = Writer::new().bytes(2, &car.encode()).message(s.setting, setting);
    for &field in s.record_required {
        record = record.uint(field, 0);
    }
    if let Some(stamp) = s.rg_stamp {
        record = record.uint(stamp, 1);
    }
    let mut record = Message::parse(&record.0).unwrap_or_default();
    record.0.sort_by_key(|(f, _)| *f);
    store::save("cars", car_id, &record);
    let mut order = vec![car_id];
    order.extend(user.uints(7));
    user.0.retain(|(f, _)| *f != 7);
    user.0.extend(order.into_iter().map(|c| (7, Value::Varint(c))));
    user.0.sort_by_key(|(f, _)| *f);
    store::save("users", user_id, &user);
    crate::log(&format!("wm5srv: user {user_id}: new car {car_id}"));
    let res = Writer::new().uint(1, ERR_SUCCESS).string(2, &user.string(3).unwrap_or_default()).uint(5, user_id).uint(6, car_id);
    // 5DX: the ghost battle stamp
    if matches!(s.game, Game::Wm5Dx | Game::Wm5DxPlus) { res.uint(16, 1) } else { res }
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
        let flag = |t: u64| if schema().tutorial_ids { 1u64.checked_shl(t as u32).unwrap_or(0) } else { t };
        let bits = req.uints(f).into_iter().fold(user.uint(5).unwrap_or(0), |b, t| b | flag(t));
        set(user, 5, Value::Varint(bits));
    }
    if let Some(v) = grade.and_then(|f| req.uint(f)) {
        set(user, 8, Value::Varint(v));
    }
}

/// Appends the items of `field` of `req` to the car's owned items.
fn add_items(record: &mut Message, req: &Message, field: u32) {
    for item in req.all(field).cloned().collect::<Vec<_>>() {
        record.0.push((schema().items, item));
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
        merge_sub(&mut record, schema().setting, &setting);
    }
    add_items(&mut record, req, 4);
    if let Some(v) = req.uint(10) {
        set(&mut record, schema().aura_auto, Value::Varint(v));
    }
    if let Some((stamp, v)) = schema().rg_stamp.zip(req.uint(12)) {
        set(&mut record, stamp, Value::Varint(v));
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
    let s = schema();
    let finished = req.uint(5) != Some(1) && req.uint(6) != Some(1);
    let mut car = req.message(7).unwrap_or_default();
    if let Some(at) = req.uint(3) {
        set(&mut car, s.car_played_at, Value::Varint(at));
    }
    // the ghost level of the car's tune (Bayshore's steps), a region for a car without
    let tune = car.uint(s.car_tune_power).unwrap_or(0) + car.uint(s.car_tune_power + 1).unwrap_or(0);
    let level = match tune {
        0..=5 => 1,
        6..=10 => 2,
        11..=15 => 3,
        16..=20 => 4,
        21..=26 => 5,
        27..=28 => 6,
        29..=30 => 7,
        31 => 8,
        32..=33 => 9,
        _ => 10,
    };
    set(&mut car, s.car_ghost_level, Value::Varint(level));
    if car.uint(3) == Some(0) {
        set(&mut car, 3, Value::Varint(car_id % 47 + 1));
    }
    merge_sub(&mut record, 2, &car);
    if req.uint(2) == Some(MODE_TIME_ATTACK) && finished {
        if let Some(ta) = req.message(s.save_ta) {
            crate::records::save_time_attack(car_id, &record, &ta);
        }
    }
    if let Some(setting) = req.message(8) {
        merge_sub(&mut record, s.setting, &setting);
    }
    // request field -> car record field
    let copy = |record: &mut Message, from: &Message, map: &[(u32, u32)]| {
        for &(src, dst) in map {
            if let Some(v) = from.get(src).cloned() {
                set(record, dst, v);
            }
        }
    };
    copy(&mut record, req, &[(4, s.play_count), (9, s.odometer), (11, s.custom_color)]);
    add_items(&mut record, req, 13);
    // story: play count, tuning point, then clear bits.. consecutive wins, their maximum
    if let Some(st) = req.message(s.save_st).filter(|_| finished) {
        let t = s.st;
        copy(&mut record, &st, &[(1, t), (2, 3), (3, t + 1), (4, t + 2), (5, t + 3), (6, t + 4), (7, t + 5), (8, t + 6)]);
        let wins = record.uint(t + 6).unwrap_or(0);
        if wins > record.uint(t + 7).unwrap_or(0) {
            set(&mut record, t + 7, Value::Varint(wins));
        }
    }
    // versus: play count, bursts, stars, (their maximum), cool / wild.. plain medals
    if let Some(vs) = req.message(s.save_vs).filter(|_| req.uint(5) != Some(1)) {
        let v = s.vs;
        copy(&mut record, &vs, &[(6, v), (7, v + 1), (8, v + 2), (9, v + 4), (10, v + 5), (11, v + 6), (12, v + 7), (13, v + 8), (14, v + 9)]);
        let stars = record.uint(v + 2).unwrap_or(0);
        if stars > record.uint(v + 3).unwrap_or(0) {
            set(&mut record, v + 3, Value::Varint(stars));
        }
    }
    if let Some(rg) = req.message(s.save_rg) {
        copy(&mut record, &rg, s.rg_map);
    }
    if let Some((user_id, mut user)) = car_user(&record) {
        update_user(&mut user, req, [Some(s.save_gold), Some(10), Some(12), s.save_grade]);
        // the car played first in the player's order
        let mut order = vec![car_id];
        order.extend(user.uints(7).into_iter().filter(|c| *c != car_id));
        user.0.retain(|(f, _)| *f != 7);
        user.0.extend(order.into_iter().map(|c| (7, Value::Varint(c))));
        user.0.sort_by_key(|(f, _)| *f);
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
        // Ranking: the shop's time attack bests per course, versus stars, trophies
        "ranking" => crate::records::ranking(),
        // CrownList: no crown holder, a placeholder crown per run area (the attract mode shows
        // them by index): Bayshore_5's areas 0..14 and 18, 18 the twelfth; 5DX (Bayshore_legacy
        // master_5dx): 0..13 and 18
        "crown_list" => {
            let areas: Vec<u64> = match schema().game {
                Game::Wm5 => {
                    let mut areas: Vec<u64> = (0..=14).collect();
                    areas.insert(11, 18);
                    areas
                }
                Game::Wm5Dx | Game::Wm5DxPlus => (0..=13).chain([18]).collect(),
                // 6 (Bayshore_legacy master_6): 0..14 and 18
                Game::Wm6 | Game::Wm6R | Game::Wm6RR => (0..=14).chain([18]).collect(),
            };
            let mut w = Writer::new();
            for area in areas {
                w = w.message(1, Writer::new().uint(1, 999_999_999 - area).uint(2, area).uint(3, 0));
            }
            w.0
        }
        // CarSummary: the cars of a name search (the terminal)
        "car_summary" => crate::records::car_summary(query),
        // FileList: no file to download (the next check in an hour)
        "file_list" => Writer::new().uint(2, 3600).0,
        _ => {
            crate::log(&format!("wm5srv: resource {name}?{query}: not implemented"));
            Vec::new()
        }
    }
}
