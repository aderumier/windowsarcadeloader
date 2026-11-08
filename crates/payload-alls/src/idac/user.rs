//! `User/*`: the player's account (`CreateAccount`, ARTEMiS's season 2 `user_controller.py`),
//! everything the game shows of it (`GetData`, with season 3's team fields: no team), and the
//! results of each mode saved back (`Update*Result`: profile, rank, stock, tickets, car use,
//! course proficiency, best times, story progress, challenges, versus and theory records).
//! Season 3's result answers carry the team's league (`Season3UpdateResultTeam`): none here.

use wal_payload_common::json::Json;

use super::records::{self, Trial};
use super::store::{self, arr, fields, merge, obj, pick, upsert, val};
use super::{Ctx, STORE_NAME, now, ok};

/// The profile's numbers after the user id and name (ARTEMiS's `idac_profile` defaults).
const PROFILE_HEAD: [(&str, i64); 4] = [("total_play", 0), ("daily_play", 0), ("day_play", 0), ("mileage", 0)];

const PROFILE_IDS: [(&str, i64); 24] = [
    ("mytitle_id", 0),
    ("mytitle_efffect_id", 0),
    ("sticker_id", 0),
    ("sticker_effect_id", 0),
    ("papercup_id", 0),
    ("tachometer_id", 0),
    ("aura_id", 0),
    ("aura_color_id", 0),
    ("aura_line_id", 0),
    ("bgm_id", 0),
    ("keyholder_id", 0),
    ("start_menu_bg_id", 0),
    ("use_car_id", 1),
    ("use_style_car_id", 1),
    ("bothwin_count", 0),
    ("bothwin_score", 0),
    ("subcard_count", 0),
    ("vs_history", 0),
    ("stamp_key_assign_0", 0),
    ("stamp_key_assign_1", 0),
    ("stamp_key_assign_2", 0),
    ("stamp_key_assign_3", 0),
    ("name_change_category", 0),
    ("factory_disp", 0),
];

const PROFILE_MONEY: [(&str, i64); 4] = [("cash", 0), ("dressup_point", 0), ("avatar_point", 0), ("total_cash", 0)];

/// `ModeRankData` (ARTEMiS's `ProfileRankDto` defaults).
const MODE_RANK: [(&str, i64); 23] = [
    ("story_rank_exp", 0),
    ("story_rank", 1),
    ("time_trial_rank_exp", 0),
    ("time_trial_rank", 1),
    ("online_battle_rank_exp", 0),
    ("online_battle_rank", 1),
    ("store_battle_rank_exp", 0),
    ("store_battle_rank", 1),
    ("theory_exp", 0),
    ("theory_rank", 1),
    ("pride_group_id", 0),
    ("pride_point", 0),
    ("grade_exp", 0),
    ("grade", 1),
    ("grade_reward_dist", 0),
    ("story_rank_reward_dist", 0),
    ("time_trial_rank_reward_dist", 0),
    ("online_battle_rank_reward_dist", 0),
    ("store_battle_rank_reward_dist", 0),
    ("theory_rank_reward_dist", 0),
    ("max_attained_online_battle_rank", 0),
    ("max_attained_pride_point", 0),
    ("is_last_max", 0),
];

const THEORY: [(&str, i64); 11] = [
    ("play_count", 0),
    ("play_count_multi", 0),
    ("partner_id", 0),
    ("partner_progress", 0),
    ("partner_progress_score", 0),
    ("practice_start_rank", 0),
    ("general_flag", 0),
    ("vs_history", 0),
    ("vs_history_multi", 0),
    ("win_count", 0),
    ("win_count_multi", 0),
];

const TIPS_COUNTS: [&str; 7] = [
    "timetrial_play_count",
    "story_play_count",
    "store_battle_play_count",
    "online_battle_play_count",
    "special_play_count",
    "challenge_play_count",
    "theory_play_count",
];

/// A challenge (special or Bunta mode) result's saved fields (ARTEMiS's `ChallengeDto`).
const CHALLENGE: [&str; 19] = [
    "vs_type",
    "play_difficulty",
    "cleared_difficulty",
    "story_type",
    "weak_difficulty",
    "eval_id",
    "advantage",
    "sec1_advantage_avg",
    "sec2_advantage_avg",
    "sec3_advantage_avg",
    "sec4_advantage_avg",
    "nearby_advantage_rate",
    "win_flag",
    "result",
    "record",
    "course_id",
    "last_play_course_id",
    "style_car_id",
    "course_day",
];

/// Story types (`StoryTypeEnum`).
const STORY: i64 = 1;
const BUNTA: i64 = 3;
const HOT_VERSION: i64 = 6;

/// Full tune tickets (`RewardCategoryEnum.FULLTUNE_TICKET`).
pub(super) const FULLTUNE_TICKET: i64 = 25;

/// The version's special mode (ARTEMiS season 3: Hot Version on 2.10).
fn special_story_type(ctx: &Ctx) -> i64 {
    if ctx.version == "210" { HOT_VERSION } else { 0 }
}

/// An unknown session (`StatusCodeEnum.BAD_SESSION`).
fn bad_session() -> Json {
    Json::obj().with("status_code", "E1001")
}

/// Season 3's result answer team fields, a player without a team.
fn no_team(resp: Json) -> Json {
    resp.with("team_daily_bonus", 0u32).with("team_weekly_bonus", 0u32)
}

fn mode_rank_data(doc: &Json) -> Json {
    fields(doc.get("mode_rank").unwrap_or(&Json::obj()), &MODE_RANK)
}

fn user_base_data(ctx: &Ctx, doc: &Json) -> Json {
    let empty = Json::obj();
    let p = doc.get("profile").unwrap_or(&empty);
    let mut b = Json::obj()
        .with("id", ctx.user)
        .with("username", val(p, "username", ""))
        .with("access_code", "11111111111111111111")
        .with("country", val(p, "country", 0i64))
        .with("store", val(p, "store", 0i64));
    for (k, d) in PROFILE_HEAD {
        b.set(k, val(p, k, d));
    }
    b.set("device_version", val(p, "device_version", "1.50.00"));
    b.set("asset_version", 0u32);
    b.set("last_play_dt", val(p, "last_play_date", now()));
    for (k, d) in PROFILE_IDS {
        b.set(k, val(p, k, d));
    }
    b.set("create_dt", val(p, "create_date", now()));
    b.set("store_name", STORE_NAME);
    b.set("mode_rank_data", mode_rank_data(doc));
    b.set("have_car_cnt", doc.a("cars").len());
    for (k, d) in PROFILE_MONEY {
        b.set(k, val(p, k, d));
    }
    b
}

/// The story progress (ARTEMiS's `generate_story_data`): an episode shows its last difficulty's
/// status, new again until played in the chapter's current loop.
fn story_data(doc: &Json) -> Vec<Json> {
    let mut out = Vec::new();
    for s in doc.a("stories") {
        let loop_count = s.get("loop_count").map_or(1, |_| s.i("loop_count"));
        let mut episodes = Vec::new();
        for e in s.a("episode_data") {
            let mut diffs: Vec<Json> = e
                .a("difficulty_data")
                .iter()
                .map(|d| Json::obj().with("difficulty", val(d, "difficulty", 0i64)).with("play_count", val(d, "play_count", 0i64)).with("clear_count", val(d, "clear_count", 0i64)).with("play_status", val(d, "play_status", 0i64)).with("play_score", val(d, "play_score", 0i64)))
                .collect();
            diffs.sort_by_key(|d| d.i("difficulty"));
            let status = if (diffs.len() as i64) < loop_count { 0 } else { diffs.last().map_or(0, |d| d.i("play_status")) };
            episodes.push(Json::obj().with("episode", val(e, "episode", 0i64)).with("play_status", status).with("difficulty_data", diffs));
        }
        if episodes.is_empty() {
            continue;
        }
        out.push(Json::obj().with("story_type", val(s, "story_type", STORY)).with("chapter", val(s, "chapter", 0i64)).with("loop_count", loop_count).with("episode_data", episodes));
    }
    out
}

/// A story result (ARTEMiS's `update_story_data`), the progress after it.
fn update_story(doc: &mut Json, req: &Json) -> Vec<Json> {
    let mut stories = story_data(doc);
    let (chapter, episode, difficulty) = (req.i("chapter"), req.i("episode"), req.i("difficulty"));
    let status = req.i("play_status");
    let ci = match stories.iter().position(|s| s.i("chapter") == chapter) {
        Some(i) => i,
        None => {
            stories.push(Json::obj().with("story_type", val(req, "story_type", STORY)).with("chapter", chapter).with("loop_count", 1i64).with("episode_data", Vec::new()));
            stories.len() - 1
        }
    };
    let ch = &mut stories[ci];
    let loop_count = ch.i("loop_count");
    let episodes = arr(ch, "episode_data");
    let ei = match episodes.iter().position(|e| e.i("episode") == episode) {
        Some(i) => i,
        None => {
            episodes.push(Json::obj().with("episode", episode).with("play_status", 0i64).with("difficulty_data", Vec::new()));
            episodes.len() - 1
        }
    };
    let ep = &mut episodes[ei];
    let diffs = arr(ep, "difficulty_data");
    let di = match diffs.iter().position(|d| d.i("difficulty") == difficulty) {
        Some(i) => i,
        None => {
            diffs.push(Json::obj().with("difficulty", difficulty).with("play_count", 0i64).with("clear_count", 0i64).with("play_status", 0i64).with("play_score", 0i64));
            diffs.len() - 1
        }
    };
    let d = &mut diffs[di];
    if status > d.i("play_status") {
        d.set("play_status", status);
        if status == 2 {
            d.set("clear_count", d.i("clear_count") + 1);
        }
    }
    if difficulty >= loop_count && status > ep.i("play_status") {
        ep.set("play_status", status);
    }
    if loop_count < req.i("chapter_loop_max") && req.i("chapter_clear") == 1 {
        ch.set("loop_count", loop_count + 1);
        for e in arr(ch, "episode_data") {
            e.set("play_status", 0i64);
        }
    }
    let d = &mut arr(&mut arr(ch, "episode_data")[ei], "difficulty_data")[di];
    d.set("play_count", d.i("play_count") + 1);
    d.set("play_score", val(req, "play_score", 0i64));
    doc.set("stories", stories.clone());
    stories
}

/// Each `vs_type`'s hardest played challenge of `story_type` (ARTEMiS's
/// `get_best_challenges_by_vs_type`), as special (`SpecialMode`) or challenge mode data.
fn challenges(doc: &Json, story_type: i64, with_count: bool) -> Vec<Json> {
    let mut best: Vec<&Json> = Vec::new();
    for c in doc.a("challenges").iter().filter(|c| c.i("story_type") == story_type) {
        match best.iter_mut().find(|b| b.i("vs_type") == c.i("vs_type")) {
            Some(b) if c.i("play_difficulty") > b.i("play_difficulty") => *b = c,
            Some(_) => {}
            None => best.push(c),
        }
    }
    best.sort_by_key(|c| c.i("vs_type"));
    best.into_iter()
        .map(|c| {
            let out = Json::obj()
                .with("story_type", val(c, "story_type", 0i64))
                .with("vs_type", val(c, "vs_type", 0i64))
                .with("max_clear_lv", val(c, "cleared_difficulty", 0i64))
                .with("last_play_lv", val(c, "play_difficulty", 0i64))
                .with("last_play_course_id", val(c, "last_play_course_id", 0i64));
            if with_count { out.with("play_count", val(c, "play_count", 1i64)) } else { out }
        })
        .collect()
}

fn save_challenge(doc: &mut Json, req: &Json) {
    let list = arr(doc, "challenges");
    let (vs_type, difficulty) = (req.i("vs_type"), req.i("play_difficulty"));
    let count = list.iter().find(|c| c.i("vs_type") == vs_type && c.i("play_difficulty") == difficulty).map_or(0, |c| c.i("play_count"));
    upsert(list, &["vs_type", "play_difficulty"], pick(req, &CHALLENGE).with("play_count", count + 1));
}

/// A battle mode's record (`VsInfo`).
fn vs_info(v: &Json, priority: i64) -> Json {
    let courses: Vec<Json> = v.a("courses").iter().map(|c| Json::obj().with("course_id", val(c, "course_id", 0i64)).with("vs_cnt", val(c, "vs_cnt", 0i64)).with("vs_win", val(c, "vs_win", 0i64))).collect();
    let flag = match v.get("break_penalty_flag") {
        Some(Json::Bool(b)) => *b,
        Some(n) => n.as_i64().unwrap_or(0) != 0,
        None => false,
    };
    Json::obj()
        .with("battle_mode", val(v, "battle_mode", 0i64))
        .with("vs_cnt", courses.iter().map(|c| c.i("vs_cnt")).sum::<i64>())
        .with("vs_win", courses.iter().map(|c| c.i("vs_win")).sum::<i64>())
        .with("invalid", val(v, "invalid", 0i64))
        .with("str", val(v, "str", 0i64))
        .with("str_now", val(v, "str_now", 0i64))
        .with("lose_now", val(v, "lose_now", 0i64))
        .with("vs_history", val(v, "vs_history", 0i64))
        .with("course_select_priority", priority)
        .with("break_count", val(v, "break_count", 0i64))
        .with("break_penalty_flag", flag)
        .with("vsinfo_course_data", courses)
}

/// A battle result into the mode's record (ARTEMiS's `update_profile_vs_info`), the record.
fn update_vs_info(doc: &mut Json, mode: i64, req: &Json, priority: i64) -> Json {
    let list = arr(doc, "vs_info");
    let i = match list.iter().position(|v| v.i("battle_mode") == mode) {
        Some(i) => i,
        None => {
            list.push(Json::obj().with("battle_mode", mode).with("invalid", 0i64).with("str", 0i64).with("str_now", 0i64).with("lose_now", 0i64).with("vs_history", 0i64).with("break_count", 0i64).with("break_penalty_flag", false).with("courses", Vec::new()));
            list.len() - 1
        }
    };
    let v = &mut list[i];
    let win = req.i("win_flg");
    v.set("invalid", v.i("invalid") + req.i("result"));
    v.set("str_now", win);
    if win > v.i("str") {
        v.set("str", win);
    }
    v.set("lose_now", i64::from(win == 0));
    v.set("vs_history", val(req, "vs_history", 0i64));
    v.set("break_count", v.i("break_count") + req.i("break_count"));
    v.set("break_penalty_flag", val(req, "break_penalty_flag", false));
    let course = req.i("course_id");
    let courses = arr(v, "courses");
    match store::find_mut(courses, "course_id", course) {
        Some(c) => {
            c.set("vs_cnt", c.i("vs_cnt") + 1);
            c.set("vs_win", c.i("vs_win") + win);
        }
        None => courses.push(Json::obj().with("course_id", course).with("vs_cnt", 1i64).with("vs_win", win)),
    }
    vs_info(&list[i], priority)
}

/// The profile fields every result carries (ARTEMiS's `update_profile_from_result`).
fn update_profile_from_result(doc: &mut Json, req: &Json) {
    {
        let p = obj(doc, "profile");
        p.set("total_play", p.i("total_play") + 1);
        p.set("last_play_date", now());
        merge(p, &pick(req, &["mileage", "cash", "total_cash", "avatar_point", "dressup_point", "aura_id", "aura_color_id", "aura_line_id", "factory_disp"]), false, &[]);
    }
    if let Some(rank) = req.get("mode_rank_obj").filter(|r| matches!(r, Json::Obj(_))) {
        let r = obj(doc, "mode_rank");
        merge(r, rank, false, &[]);
        if let Some(dist) = req.get("reward_dist_obj") {
            merge(r, dist, false, &[]);
        }
    }
    store::update_inventory(doc, req);
}

/// A car's use after a race (ARTEMiS's `update_car_usage_mileage`), `mode` its mode's count.
fn use_car(doc: &mut Json, req: &Json, mode: &str) {
    if let Some(car) = store::car_mut(doc, req.i("style_car_id")) {
        car.set("car_mileage", val(req, "car_mileage", 0i64));
        car.set("use_count", car.i("use_count") + 1);
        car.set(mode, car.i(mode) + 1);
    }
}

fn tips_play(doc: &mut Json, field: &str) {
    let tips = obj(doc, "tips");
    tips.set(field, tips.i(field) + 1);
}

/// The car counts the results answer (titles).
fn use_counts(doc: &Json, resp: Json) -> Json {
    let cars = doc.a("cars");
    resp.with("car_use_count", store::car_use_count(cars)).with("maker_use_count", store::maker_use_count(cars))
}

fn create_account(ctx: &Ctx) -> Json {
    let req = ctx.body;
    let mut doc = store::load(ctx.user).unwrap_or(Json::obj());
    let use_ticket = req.i("use_ticket") != 0;
    let mut car = store::new_car(req.get("car_obj").unwrap_or(&Json::obj()));
    if use_ticket {
        car.set("tune_level", 16i64);
    }
    {
        let p = obj(&mut doc, "profile");
        p.set("username", val(req, "username", ""));
        p.set("country", ctx.header_num("a_country"));
        p.set("store", ctx.header_num("a_store"));
        p.set("device_version", ctx.header("device_version"));
        for k in ["stamp_key_assign_0", "stamp_key_assign_1", "stamp_key_assign_2", "stamp_key_assign_3"] {
            p.set(k, val(req, k, 0i64));
        }
        p.set("cash", val(req, "cash", 0i64));
        p.set("total_cash", val(req, "cash", 0i64));
        p.set("create_date", now());
        p.set("last_play_date", now());
        if use_ticket {
            // one free name change (takeover)
            p.set("name_change_category", 2i64);
        }
    }
    upsert(arr(&mut doc, "cars"), &["style_car_id"], car);
    doc.set("avatar", val(req, "avatar_obj", Json::obj()));
    merge(obj(&mut doc, "config"), req.get("config_obj").unwrap_or(&Json::obj()), false, &["id", "config_id"]);
    doc.set("mode_rank", fields(req.get("mode_rank_data").unwrap_or(&Json::obj()), &MODE_RANK));
    merge(obj(&mut doc, "stock"), req.get("takeover_stock_obj").unwrap_or(&Json::obj()), true, &[]);
    store::put_tickets(&mut doc, req.a("takeover_ticket"));
    store::save(ctx.user, &doc);
    let cars = doc.a("cars");
    ok().with("round_event", Vec::new())
        .with("past_round_event", Vec::new())
        .with("stamp_event_data", Vec::new())
        .with("fulltune_count", store::fulltune_count(cars))
        .with("total_car_parts_count", 0u32)
        .with("car_layout_count", store::car_layout_count(cars))
        .with("car_style_count", store::car_style_count(cars))
        .with("battle_gift_data", Vec::new())
}

fn get_data(ctx: &Ctx) -> Json {
    let Some(doc) = store::load(ctx.user) else { return bad_session() };
    let empty = Json::obj();
    let cars = doc.a("cars");
    let pickup: Vec<Json> = cars.iter().filter(|c| c.get("pickup_seq").is_none_or(|_| c.i("pickup_seq") != 0)).map(|c| store::car_out(ctx.user, c, false)).collect();
    let courses: Vec<Json> = doc.a("courses").iter().map(|c| Json::obj().with("id", ctx.user).with("course_id", val(c, "course_id", 0i64)).with("run_counts", val(c, "run_counts", 0i64)).with("skill_level_exp", val(c, "skill_level_exp", 0i64))).collect();
    let mut config = Json::obj().with("id", ctx.user);
    merge(&mut config, doc.get("config").unwrap_or(&empty), false, &["id", "config_id"]);
    let tickets: Vec<Json> = doc.a("tickets").iter().map(|t| Json::obj().with("ticket_id", val(t, "ticket_id", 0i64)).with("ticket_cnt", val(t, "ticket_cnt", 0i64))).collect();
    let mut resp = ok().with("user_base_data", user_base_data(ctx, &doc));
    if let Some(avatar) = doc.get("avatar") {
        resp.set("avatar_data", avatar.clone());
    }
    resp.set("pick_up_car_data", pickup);
    resp.set("story_data", story_data(&doc));
    resp.set("vsinfo_data", doc.a("vs_info").iter().map(|v| vs_info(v, 0)).collect::<Vec<_>>());
    if let Some(stock) = doc.get("stock") {
        resp.set("stock_data", stock.clone());
    }
    resp.set("course_data", courses);
    resp.set("rewards_data", Vec::new());
    resp.set("frozen_data", Json::obj().with("frozen_status", 0u32));
    resp.set("config_data", config);
    resp.set("battle_gift_data", Vec::new());
    resp.set("ticket_data", tickets);
    resp.set("round_event", Vec::new());
    resp.set("last_round_event", Vec::new());
    resp.set("past_round_event", Vec::new());
    resp.set("stamp_event_data", Vec::new());
    resp.set("avatar_gacha_lottery_data", Vec::new());
    resp.set("fulltune_count", store::fulltune_count(cars));
    resp.set("total_car_parts_count", val(doc.get("profile").unwrap_or(&empty), "total_car_parts_count", 0i64));
    resp.set("car_layout_count", store::car_layout_count(cars));
    resp.set("car_style_count", store::car_style_count(cars));
    resp.set("car_use_count", store::car_use_count(cars));
    resp.set("maker_use_count", store::maker_use_count(cars));
    resp.set("story_course", Vec::new());
    if let Some(theory) = doc.get("theory") {
        resp.set("theory_data", fields(theory, &THEORY));
    }
    resp.set(
        "theory_course_data",
        doc.a("theory_courses")
            .iter()
            .map(|c| fields(c, &[("course_id", 0), ("max_victory_grade", 0), ("run_count", 0), ("powerhouse_lv", 0), ("powerhouse_exp", 0), ("played_powerhouse_lv", 0), ("update_dt", 0)]))
            .collect::<Vec<_>>(),
    );
    resp.set("theory_partner_data", doc.a("theory_partners").iter().map(|c| fields(c, &[("partner_id", 0), ("fellowship_lv", 0), ("fellowship_exp", 0)])).collect::<Vec<_>>());
    resp.set(
        "theory_running_pram_data",
        doc.a("theory_running").iter().map(|c| fields(c, &[("course_id", 0), ("attack", 0), ("defense", 0), ("safety", 0), ("runaway", 0), ("trick_flag", 0)])).collect::<Vec<_>>(),
    );
    resp.set("special_mode_data", challenges(&doc, special_story_type(ctx), false));
    resp.set("challenge_mode_data", challenges(&doc, BUNTA, true));
    resp.set("season_rewards_data", Vec::new());
    if let Some(tips) = doc.get("tips") {
        let mut t = Json::obj().with("tips_list", val(tips, "tips_list", "QAAAAAAAAAAAAAAA"));
        for k in TIPS_COUNTS {
            t.set(k, val(tips, k, 0i64));
        }
        if let Some(dt) = tips.get("last_team_join_dt") {
            t.set("last_team_join_dt", dt.clone());
        }
        resp.set("tips_info", t);
    }
    // season 3: no team (team_info, league_info absent)
    resp.with("last_league_info", Vec::new()).with("league_rewards", Vec::new()).with("team_invite_flag", 1u32)
}

/// The season's best time per course the player ran (`SeasonTimetrialData`); past seasons have
/// none (only the courses' proficiency).
fn season_trials(doc: &Json, trials: &[Trial], user: u64, current: bool) -> Vec<Json> {
    doc.a("courses")
        .iter()
        .map(|c| {
            let course = c.i("course_id");
            let out = Json::obj();
            match records::user_rank(trials, user, course).filter(|_| current) {
                Some((rank, _, t)) => out
                    .with("style_car_id", val(&t.rec, "style_car_id", 0i64))
                    .with("course_id", course)
                    .with("skill_level_exp", val(c, "skill_level_exp", 0i64))
                    .with("goal_time", val(&t.rec, "goal_time", 0i64))
                    .with("rank", rank)
                    .with("rank_dt", val(&t.rec, "play_dt", 0i64)),
                None => out.with("style_car_id", 0u32).with("course_id", course).with("skill_level_exp", val(c, "skill_level_exp", 0i64)).with("goal_time", 0u32).with("rank", 0u32).with("rank_dt", 0u32),
            }
        })
        .collect()
}

fn update_login(doc: &mut Json, ctx: &Ctx) -> Json {
    let req = ctx.body;
    {
        let p = obj(doc, "profile");
        merge(
            p,
            &pick(
                req,
                &[
                    "mytitle_id",
                    "mytitle_efffect_id",
                    "sticker_id",
                    "sticker_effect_id",
                    "keyholder_id",
                    "papercup_id",
                    "tachometer_id",
                    "aura_id",
                    "aura_color_id",
                    "aura_line_id",
                    "bgm_id",
                    "start_menu_bg_id",
                    "cash",
                    "total_cash",
                    "dressup_point",
                    "avatar_point",
                ],
            ),
            false,
            &[],
        );
        p.set("device_version", ctx.header("device_version"));
    }
    store::update_inventory(doc, req);
    if let Some(avatar) = req.get("avatar_obj").filter(|a| matches!(a, Json::Obj(_))) {
        doc.set("avatar", avatar.clone());
    }
    ok()
}

fn update_time_trial(doc: &mut Json, ctx: &Ctx) -> Json {
    let req = ctx.body;
    update_profile_from_result(doc, req);
    use_car(doc, req, "timetrial_use_count");
    let (course, car, goal) = (req.i("course_id"), req.i("style_car_id"), req.i("goal_time"));
    // course proficiency
    let courses = arr(doc, "courses");
    let runs = courses.iter().find(|c| c.i("course_id") == course).map_or(0, |c| c.i("run_counts"));
    upsert(courses, &["course_id"], Json::obj().with("course_id", course).with("run_counts", runs + 1).with("skill_level_exp", val(req, "skill_level_exp", 0i64)));
    // the best time per course and car
    if goal > 0 {
        let trials = arr(doc, "trials");
        let best = trials.iter().find(|t| t.i("course_id") == course && t.i("style_car_id") == car).map(|t| t.i("goal_time"));
        if best.is_none_or(|b| goal < b) {
            let mut rec = pick(req, &["style_car_id", "course_id", "eval_id", "goal_time", "section_time_1", "section_time_2", "section_time_3", "section_time_4", "mission"]);
            rec.set("play_dt", now());
            upsert(trials, &["course_id", "style_car_id"], rec);
        }
    }
    tips_play(doc, "timetrial_play_count");
    let players = store::players(Some((ctx.user, &*doc)));
    let trials = records::all_trials(&players);
    let course_rank = records::ranking(&trials, course, None).iter().filter(|t| t.rec.i("goal_time") < goal).count() + 1;
    let car_rank = records::ranking(&trials, course, Some(car)).iter().filter(|t| t.rec.i("goal_time") < goal).count() + 1;
    no_team(use_counts(doc, ok().with("course_rank", course_rank).with("course_car_rank", car_rank).with("location_course_store_rank", 0u32)))
}

fn update_theory(doc: &mut Json, ctx: &Ctx) -> Json {
    let req = ctx.body;
    update_profile_from_result(doc, req);
    let mode = req.i("battle_mode");
    let theory = obj(doc, "theory");
    merge(theory, &pick(req, &["partner_id", "partner_progress", "partner_progress_score", "practice_start_rank", "general_flag", "vs_history"]), false, &[]);
    // online 0, in store 1
    if mode == 0 {
        theory.set("play_count", theory.i("play_count") + 1);
        if req.i("advantage_2") > 0 {
            theory.set("win_count", theory.i("win_count") + 1);
        }
    } else if mode == 1 {
        theory.set("play_count_multi", theory.i("play_count_multi") + 1);
        if req.i("advantage_2") > 0 || req.i("advantage_3") > 0 || req.i("advantage_4") > 0 {
            theory.set("win_count_multi", theory.i("win_count_multi") + 1);
        }
    }
    let counts = fields(theory, &THEORY);
    let course = req.i("course_id");
    let theory_courses = arr(doc, "theory_courses");
    let picked = pick(req, &["course_id", "max_victory_grade", "powerhouse_lv", "powerhouse_exp"]);
    match store::find_mut(theory_courses, "course_id", course) {
        Some(c) => {
            merge(c, &picked, false, &[]);
            c.set("run_count", c.i("run_count") + 1);
        }
        None => theory_courses.push(picked.with("run_count", 1i64).with("played_powerhouse_lv", val(req, "powerhouse_lv", 0i64)).with("update_dt", now())),
    }
    upsert(arr(doc, "theory_partners"), &["partner_id"], fields(req, &[("partner_id", 0), ("fellowship_lv", 0), ("fellowship_exp", 0)]));
    upsert(arr(doc, "theory_running"), &["course_id"], fields(req, &[("course_id", 0), ("attack", 0), ("defense", 0), ("safety", 0), ("runaway", 0), ("trick_flag", 0)]));
    use_car(doc, req, "theory_use_count");
    tips_play(doc, "theory_play_count");
    let resp = ok().with("played_powerhouse_lv", val(req, "powerhouse_lv", 0i64));
    let resp = use_counts(doc, resp);
    no_team(
        resp.with("play_count", val(&counts, "play_count", 0i64))
            .with("play_count_multi", val(&counts, "play_count_multi", 0i64))
            .with("win_count", val(&counts, "win_count", 0i64))
            .with("win_count_multi", val(&counts, "win_count_multi", 0i64)),
    )
}

/// `User/<endpoint>`.
pub(super) fn reply(ctx: &Ctx, endpoint: &str) -> Option<Json> {
    match endpoint {
        "createaccount" => return Some(create_account(ctx)),
        "getdata" => return Some(get_data(ctx)),
        "gettadata" | "getpastseasontadata" | "getpastseasonrounddata" => {
            let doc = store::load(ctx.user).unwrap_or(Json::obj());
            let season = val(ctx.body, "season_id", 0i64);
            return Some(match endpoint {
                "gettadata" => {
                    let players = store::players(None);
                    ok().with("timetrial_data", season_trials(&doc, &records::all_trials(&players), ctx.user, true))
                }
                "getpastseasontadata" => ok().with("season_id", season).with("past_season_timetrial_data", season_trials(&doc, &[], ctx.user, false)),
                _ => ok().with("season_id", season).with("past_season_round_event_data", Vec::new()),
            });
        }
        "updatestampinfo" => {
            let mut doc = store::load(ctx.user).unwrap_or(Json::obj());
            for stamp in ctx.body.a("stamp_event_data") {
                let mut s = stamp.clone();
                s.set("create_date_daily", now());
                s.set("create_date_weekly", now());
                upsert(arr(&mut doc, "stamps"), &["m_stamp_event_id"], s);
            }
            store::save(ctx.user, &doc);
            return Some(ok());
        }
        _ => {}
    }
    let known = matches!(
        endpoint,
        "updatelogin"
            | "updatetimetrialresult"
            | "updatemoderesult"
            | "updatestoryresult"
            | "updatecartune"
            | "updatespecialmoderesult"
            | "updatetheoryresult"
            | "updatechallengemoderesult"
            | "updatestorebattleresult"
            | "updateonlinebattleresult"
    );
    if !known {
        return None;
    }
    let Some(mut doc) = store::load(ctx.user) else { return Some(bad_session()) };
    let req = ctx.body;
    let doc = &mut doc;
    let resp = match endpoint {
        "updatelogin" => update_login(doc, ctx),
        "updatetimetrialresult" => update_time_trial(doc, ctx),
        "updatemoderesult" => {
            update_profile_from_result(doc, req);
            if let Some(tips) = req.get("tips_list") {
                obj(doc, "tips").set("tips_list", tips.clone());
            }
            if let Some(config) = req.get("config_obj") {
                merge(obj(doc, "config"), config, false, &["id", "config_id"]);
            }
            ok().with("server_status", 1u32)
        }
        "updatestoryresult" => {
            update_profile_from_result(doc, req);
            let stories = update_story(doc, req);
            use_car(doc, req, "story_use_count");
            no_team(use_counts(doc, ok().with("story_data", stories)))
        }
        "updatecartune" => {
            let mut tune = pick(req, &["car_flag", "tune_point", "tune_level", "tune_parts"]);
            if req.i("use_ticket") != 0 {
                store::use_tickets(doc, FULLTUNE_TICKET, 1);
                tune.set("tune_level", 16i64);
            }
            if let Some(car) = store::car_mut(doc, req.i("style_car_id")) {
                merge(car, &tune, false, &[]);
            }
            use_counts(doc, ok().with("story_data", story_data(doc)))
        }
        "updatespecialmoderesult" => {
            update_profile_from_result(doc, req);
            if req.i("rental_flag") == 0 {
                use_car(doc, req, "story_use_count");
            }
            save_challenge(doc, req);
            tips_play(doc, "special_play_count");
            let special = challenges(doc, req.i("story_type"), false);
            no_team(use_counts(doc, ok().with("special_mode_data", special)))
        }
        "updatechallengemoderesult" => {
            update_profile_from_result(doc, req);
            use_car(doc, req, "story_use_count");
            save_challenge(doc, req);
            tips_play(doc, "story_play_count");
            let challenge = challenges(doc, req.i("story_type"), true);
            no_team(use_counts(doc, ok().with("challenge_mode_data", challenge)))
        }
        "updatetheoryresult" => update_theory(doc, ctx),
        "updatestorebattleresult" => {
            update_profile_from_result(doc, req);
            let vs = update_vs_info(doc, 1, req, 0);
            tips_play(doc, "store_battle_play_count");
            use_car(doc, req, "vs_use_count");
            no_team(use_counts(doc, ok().with("vsinfo_data", vs)))
        }
        _ => {
            // updateonlinebattleresult
            update_profile_from_result(doc, req);
            let vs = update_vs_info(doc, 0, req, req.i("course_select_priority"));
            tips_play(doc, "online_battle_play_count");
            use_car(doc, req, "net_vs_use_count");
            no_team(use_counts(doc, ok().with("vsinfo_data", vs).with("round_event", Vec::new())))
        }
    };
    store::save(ctx.user, doc);
    Some(resp)
}
