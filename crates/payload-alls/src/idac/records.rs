//! The players' time trial records and what is built from every player's: `TimeTrial/*` (the
//! best times before a race), `Advertise/GetRankingData` (the attract's course rankings),
//! `Theory/Matching` (the Theory of Street rivals: this cabinet's other players of the same grade,
//! CPUs for the rest). ARTEMiS ranks each player's best time (`time_trial_db.py`); here over the
//! players in `Y:\idac\`.

use wal_payload_common::json::Json;

use super::store::{self, val};
use super::{Ctx, STORE_NAME, ok};

/// One saved time (a player's best on a course with a car).
pub(super) struct Trial {
    pub(super) user: u64,
    pub(super) rec: Json,
}

pub(super) fn all_trials(players: &[(u64, Json)]) -> Vec<Trial> {
    players.iter().flat_map(|(id, doc)| doc.a("trials").iter().map(|t| Trial { user: *id, rec: t.clone() })).collect()
}

/// Each player's best on `course` (with `car`), fastest first.
pub(super) fn ranking(trials: &[Trial], course: i64, car: Option<i64>) -> Vec<&Trial> {
    let mut best: Vec<&Trial> = Vec::new();
    for t in trials.iter().filter(|t| t.rec.i("course_id") == course && car.is_none_or(|c| t.rec.i("style_car_id") == c)) {
        match best.iter_mut().find(|b| b.user == t.user) {
            Some(b) if t.rec.i("goal_time") < b.rec.i("goal_time") => *b = t,
            Some(_) => {}
            None => best.push(t),
        }
    }
    best.sort_by_key(|t| t.rec.i("goal_time"));
    best
}

/// `user`'s best on `course`, its rank and the players ranked.
pub(super) fn user_rank(trials: &[Trial], user: u64, course: i64) -> Option<(usize, usize, &Trial)> {
    let ranked = ranking(trials, course, None);
    let total = ranked.len();
    ranked.into_iter().enumerate().find(|(_, t)| t.user == user).map(|(i, t)| (i + 1, total, t))
}

/// A `CourseBestData` (with `course_id`) or a `CourseBestCarData.CarList` entry.
fn best(t: &Trial, course: bool, rank: usize, total: Option<usize>, store: &str, mission: Option<i64>) -> Json {
    let r = &t.rec;
    let mut out = Json::obj();
    if course {
        out.set("course_id", val(r, "course_id", 0i64));
    }
    out.set("rank", rank);
    out.set("member", t.user);
    out.set("value", val(r, "goal_time", 0i64));
    if let Some(total) = total {
        out.set("total", total);
    }
    out.set("store", store);
    out.set("style_car_id", val(r, "style_car_id", 0i64));
    out.set("play_dt", val(r, "play_dt", 0i64));
    for k in ["section_time_1", "section_time_2", "section_time_3", "section_time_4"] {
        out.set(k, val(r, k, 0i64));
    }
    out.set("mission", mission.map_or_else(|| val(r, "mission", 0i64), Json::from));
    out
}

/// The player's best per course and car (of `cars`), per course they ran (ARTEMiS's
/// `get_course_pickup_car_best_data`: ranked among their own times, so rank 1 of 1).
fn pickup_car_best(trials: &[Trial], user: u64, cars: &[i64]) -> Vec<Json> {
    let mine: Vec<&Trial> = trials.iter().filter(|t| t.user == user).collect();
    let mut courses: Vec<i64> = Vec::new();
    for t in &mine {
        if !courses.contains(&t.rec.i("course_id")) {
            courses.push(t.rec.i("course_id"));
        }
    }
    let mut out = Vec::new();
    for course in courses {
        let list: Vec<Json> = mine
            .iter()
            .filter(|t| t.rec.i("course_id") == course && cars.contains(&t.rec.i("style_car_id")))
            .map(|t| best(t, false, 1, Some(1), "251", None))
            .collect();
        if !list.is_empty() {
            out.push(Json::obj().with("course_id", course).with("car_list", list));
        }
    }
    out
}

fn car_ids(body: &Json) -> Vec<i64> {
    body.a("car_ids").iter().map(|c| c.i("style_car_id")).collect()
}

/// `TimeTrial/<endpoint>`.
pub(super) fn time_trial(ctx: &Ctx, endpoint: &str) -> Option<Json> {
    let players = store::players(None);
    let trials = all_trials(&players);
    Some(match endpoint {
        "getbestrecordpreta" => {
            let mut courses: Vec<i64> = Vec::new();
            for t in trials.iter().filter(|t| t.user == ctx.user) {
                if !courses.contains(&t.rec.i("course_id")) {
                    courses.push(t.rec.i("course_id"));
                }
            }
            let mybest: Vec<Json> = courses
                .iter()
                .filter_map(|c| user_rank(&trials, ctx.user, *c))
                .map(|(rank, total, t)| best(t, true, rank, Some(total), "251", None))
                .collect();
            ok().with("course_mybest_data", mybest).with("course_pickup_car_best_data", pickup_car_best(&trials, ctx.user, &car_ids(ctx.body)))
        }
        "getbestrecordprerace" => {
            let course = ctx.body.i("course_id");
            let car_best: Vec<Json> = car_ids(ctx.body)
                .into_iter()
                .filter_map(|car| ranking(&trials, course, Some(car)).first().map(|t| best(t, false, 1, None, "251", Some(1))))
                .map(|entry| Json::obj().with("course_id", course).with("car_list", vec![entry]))
                .collect();
            let course_best: Vec<Json> = ranking(&trials, course, None).first().map(|t| best(t, true, 1, None, "", Some(1))).into_iter().collect();
            ok().with("course_car_best_data", car_best).with("course_best_data", course_best)
        }
        "getbestrecordprebattle" => ok().with("course_pickup_car_best_data", pickup_car_best(&trials, ctx.user, &car_ids(ctx.body))),
        "getcarbest" => ok(),
        _ => return None,
    })
}

/// `Advertise/GetRankingData`: the top 10 of the courses asked.
pub(super) fn ranking_data(ctx: &Ctx) -> Json {
    let players = store::players(None);
    let trials = all_trials(&players);
    let mut national = Vec::new();
    for last in ctx.body.a("last_update_date") {
        let course = last.i("course_id");
        let mut list = Vec::new();
        for (i, t) in ranking(&trials, course, None).into_iter().take(10).enumerate() {
            let profile = players.iter().find(|(id, _)| *id == t.user).and_then(|(_, d)| d.get("profile")).cloned().unwrap_or(Json::obj());
            let r = &t.rec;
            let mut entry = Json::obj()
                .with("course_id", course)
                .with("rank", i + 1)
                .with("username", val(&profile, "username", ""))
                .with("value", val(r, "goal_time", 0i64))
                .with("store", STORE_NAME)
                .with("country", val(&profile, "country", 0i64))
                .with("style_car_id", val(r, "style_car_id", 0i64))
                .with("play_dt", val(r, "play_dt", 0i64));
            for k in ["section_time_1", "section_time_2", "section_time_3", "section_time_4", "mission"] {
                entry.set(k, val(r, k, 0i64));
            }
            list.push(entry);
        }
        national.push(Json::obj().with("course_id", course).with("ranking_data", list));
    }
    ok().with("national_best_data", national).with("shop_best_data", Vec::new()).with("rank_management_flag", 0u32)
}

/// The CPU rivals' avatars (ARTEMiS's `_get_cpu_avatars`).
const CPU_AVATARS: [[i64; 13]; 4] = [
    [0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0],
    [0, 1, 1, 1, 19, 0, 0, 2, 0, 0, 1, 0, 0],
    [1, 91, 265, 13, 369, 0, 0, 113, 0, 0, 1, 0, 0],
    [1, 91, 265, 13, 387, 0, 0, 114, 0, 0, 1, 0, 0],
];

const AVATAR: [(&str, &str); 13] = [
    ("sex", "sex"),
    ("face", "face"),
    ("eye", "eye"),
    ("mouth", "mouth"),
    ("hair", "hair"),
    ("glasses", "glasses"),
    ("face_accessory", "faceAccessory"),
    ("body", "body"),
    ("body_accessory", "bodyAccessory"),
    ("behind", "behind"),
    ("bg", "bg"),
    ("effect", "effect"),
    ("special", "special"),
];

/// Season 3's highest grade (ARTEMiS's `MAX_GRADE`).
const MAX_GRADE: i64 = 72;

fn pick_random<T: Clone>(items: &[T]) -> Option<T> {
    (!items.is_empty()).then(|| items[(store::random() % items.len() as u64) as usize].clone())
}

/// Up to 3 of `ids` at random.
fn three(mut ids: Vec<u64>) -> Vec<u64> {
    let mut out = Vec::new();
    while out.len() < 3 && !ids.is_empty() {
        let i = (store::random() % ids.len() as u64) as usize;
        out.push(ids.swap_remove(i));
    }
    out
}

/// One rival (`Season2PowerMatch`): a player (`Some(id)`) or a CPU.
fn rival(players: &[(u64, Json)], trials: &[Trial], who: Option<u64>, grade: i64, course: i64, powerhouse_lv: i64) -> Option<Json> {
    let doc = who.and_then(|id| players.iter().find(|(p, _)| *p == id)).map(|(_, d)| d);
    let (profile, avatar, car, grade) = match doc {
        Some(d) => (
            d.get("profile").cloned().unwrap_or(Json::obj()),
            d.get("avatar").cloned().unwrap_or(Json::obj()),
            pick_random(d.a("cars"))?,
            d.get("mode_rank").map_or(grade, |r| r.i("grade")),
        ),
        None => {
            let cpu = pick_random(&CPU_AVATARS)?;
            let mut avatar = Json::obj();
            for ((k, _), v) in AVATAR.iter().zip(cpu) {
                avatar.set(k, v);
            }
            let profile = Json::obj()
                .with("username", "ＣＰＵ")
                .with("country", 9u32)
                .with("store", 0u32)
                .with("stamp_key_assign_0", 0u32)
                .with("stamp_key_assign_1", 1u32)
                .with("stamp_key_assign_2", 2u32)
                .with("stamp_key_assign_3", 3u32);
            let cars: Vec<Json> = players.iter().flat_map(|(_, d)| d.a("cars").to_vec()).collect();
            (profile, avatar, pick_random(&cars)?, grade)
        }
    };
    let theory_course = doc.and_then(|d| d.a("theory_courses").iter().find(|c| c.i("course_id") == course));
    let running = doc.and_then(|d| d.a("theory_running").iter().find(|c| c.i("course_id") == course));
    let eval = who.and_then(|id| user_rank(trials, id, course)).map_or(0, |(_, _, t)| t.rec.i("eval_id"));
    let parts: Vec<Json> = car.a("parts_list").iter().map(|p| val(p, "parts", 0i64)).collect();
    let mut out = Json::obj()
        .with("id", who.map_or(Json::from(-1i64), Json::from))
        .with("name", val(&profile, "username", ""))
        .with("grade", grade)
        .with("powerhouseLv", theory_course.map_or(powerhouse_lv, |c| c.i("powerhouse_lv")))
        .with("mytitleId", val(&profile, "mytitle_id", 0i64))
        .with("country", val(&profile, "country", 0i64))
        .with("auraId", val(&profile, "aura_id", 0i64))
        .with("auraColor", val(&profile, "aura_color_id", 0i64))
        .with("auraLine", val(&profile, "aura_line_id", 0i64))
        .with("roundRanking", 0u32)
        .with("storeName", STORE_NAME);
    for (k, name) in AVATAR {
        out.set(name, val(&avatar, k, 0i64));
    }
    for (k, name) in [("style_car_id", "styleCarId"), ("color", "color"), ("bureau", "bureau"), ("kana", "kana"), ("s_no", "sNo"), ("l_no", "lNo"), ("tune_level", "tuneLv"), ("car_flag", "carFlag"), ("tune_point", "tunePoint"), ("infinity_tune", "infinityTune"), ("tune_parts", "tuneParts")] {
        out.set(name, val(&car, k, 0i64));
    }
    out.set("partsList", parts);
    out.set("partsCount", val(&car, "equip_parts_count", 0i64));
    for i in 0..4 {
        out.set(&format!("stamp{i}"), val(&profile, &format!("stamp_key_assign_{i}"), 0i64));
    }
    let real = running.filter(|_| who.is_some());
    for (k, name) in [("attack", "attack"), ("defense", "defense"), ("safety", "safety"), ("runaway", "runaway")] {
        out.set(name, real.map_or(Json::from(128i64), |r| val(r, k, 128i64)));
    }
    out.set("trickFlg", real.map_or(Json::from(0i64), |r| val(r, "trick_flag", 0i64)));
    out.set("taEval", eval);
    Some(out)
}

/// `Theory/Matching`: 3 rivals per grade around the player's (`auto_match`), 3 of the course's
/// powerhouse level when asked (`power_match`).
pub(super) fn matching(ctx: &Ctx) -> Json {
    let players = store::players(None);
    let trials = all_trials(&players);
    let device = ctx.header("device_version");
    let (grade, course, lv) = (ctx.body.i("grade"), ctx.body.i("course_id"), ctx.body.i("powerhouse_lv"));
    let others = |keep: &dyn Fn(&Json) -> bool| -> Vec<u64> {
        players.iter().filter(|(id, d)| *id != ctx.user && d.get("profile").is_some_and(|p| p.s("device_version") == device) && keep(d)).map(|(id, _)| *id).collect()
    };
    let offsets: &[i64] = if grade < 10 {
        &[0, 9]
    } else if (MAX_GRADE - 8..=MAX_GRADE).contains(&grade) {
        &[-9, 0]
    } else {
        &[-9, 0, 9]
    };
    let mut auto = Vec::new();
    for g in offsets.iter().map(|o| grade + o) {
        let mut who: Vec<Option<u64>> = three(others(&|d| d.get("mode_rank").is_some_and(|r| r.i("grade") == g))).into_iter().map(Some).collect();
        who.resize(3, None);
        auto.extend(who.into_iter().filter_map(|w| rival(&players, &trials, w, g, course, 0)));
    }
    let mut power = Vec::new();
    if lv > 0 {
        let mut who: Vec<Option<u64>> = three(others(&|d| d.a("theory_courses").iter().any(|c| c.i("course_id") == course && c.i("powerhouse_lv") == lv))).into_iter().map(Some).collect();
        who.resize(3, None);
        power.extend(who.into_iter().filter_map(|w| rival(&players, &trials, w, grade, course, lv)));
    }
    ok().with("rival_data", Json::obj().with("auto_match", auto).with("power_match", power))
}
