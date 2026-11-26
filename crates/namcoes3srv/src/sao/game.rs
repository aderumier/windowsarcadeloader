//! The game commands of ARTEMiS' `titles/sao/base.py` (WTFPL), each answered as ARTEMiS answers
//! it; the commands it has no handler for get its generic answer (result 1, the other fields
//! empty: `handle_noop`). Where ARTEMiS' code cannot work as meant (an inverted condition, a
//! function that returns nothing), the intended behaviour, noted at the place.

use super::data::{Row, table};
use super::db::{Db, now};
use super::value::V;
use super::{EPOCH, Reward};

/// ARTEMiS' `game_version` and `data_version` defaults (`config.py`).
const GAME_VERSION: i64 = 33;
const DATA_VERSION: i64 = 1;
/// ARTEMiS' default Photon application (`photon_app_id`), for the multiplayer rooms.
const PHOTON_APP_ID: &str = "7df3a2f6-d69d-4073-aafe-810ee61e1cea";

/// `RewardType`
const REWARD_HERO_LOG: i64 = 1;
const REWARD_EQUIPMENT: i64 = 2;
const REWARD_ITEM: i64 = 3;
const REWARD_COL: i64 = 4;
const REWARD_VP: i64 = 5;
const REWARD_YUI_MEDAL: i64 = 6;
const REWARD_SUPPORT_LOG: i64 = 8;

/// `QuestType`
const QUEST_EPISODE: i64 = 1;
const QUEST_TRIAL_TOWER: i64 = 2;

/// An answer: its values, and the header's error status (0: none).
pub(crate) struct Answer {
    pub(crate) value: V,
    pub(crate) err_status: u16,
}

fn ok(value: V) -> Answer {
    Answer { value: value.with("result", 1), err_status: 0 }
}

/// `handle_noop`: result 1.
fn noop() -> Answer {
    ok(V::obj())
}

fn list(items: Vec<V>) -> V {
    V::List(items)
}

/// A Python `random.choice` of `n` elements (`None` when empty).
fn choice(n: usize) -> Option<usize> {
    (n > 0).then(|| (super::random() % n as u64) as usize)
}

/// Answers command `cmd` (its request read into `req`), `None`: ARTEMiS' generic answer.
pub(crate) fn handle(cmd: u16, req: &V, server: &str) -> Answer {
    let user = req.i("user_id");
    let mut guard = super::db::lock();
    let Some(db) = guard.as_mut() else { return noop() };
    let answer = match cmd {
        // ticket/ticket
        0xc000 => ok(V::obj().with("ticket_id", "9")).value_with("result", "1"),
        // common/get_app_versions
        0xc100 => ok(V::obj().with("data_list", list(vec![V::obj().with("version_app_id", GAME_VERSION).with("applying_start_date", EPOCH)]))),
        // common/master_data_version_check
        0xc102 => {
            let current = req.i("current_data_version");
            crate::log(&format!("sao: master data version {current}"));
            ok(V::obj().with("update_flag", (DATA_VERSION != current) as i64).with("data_version", current))
        }
        0xc104 => login(db, req),
        // common/paying_play_start
        0xc10a => ok(V::obj().with("paying_session_id", "1")),
        0xc11e => auth_card_data(db, req),
        // common/get_access_code_by_keitai: a phone's card, known here only
        0xc120 => {
            let idm = req.s("chip_id").chars().take(16).collect::<String>();
            ok(V::obj().with("access_code", db.card_by_idm(&idm).map(|c| c.s("access_code")).unwrap_or_default()))
        }
        // common/get_maintenance_info
        0xc122 => ok(V::obj().with("start_date", EPOCH).with("end_date", EPOCH)),
        // common/get_resource_path_info (this server's /saoresource/, ARTEMiS' resource route)
        0xc124 => ok(V::obj()
            .with("resource_base_url", format!("http://{server}/saoresource/"))
            .with("gasha_base_dir", "gasha")
            .with("ad_base_dir", "ad")
            .with("event_base_dir", "event")),
        // common/ac_cabinet_boot_notification
        0xc12e => {
            crate::log(&format!("sao: cabinet {} booted v{} (master data v{})", req.s("serial_no"), req.i("current_version_app_id"), req.i("current_master_data_version")));
            noop()
        }
        // tutorial/various_tutorial_end
        0xc202 => {
            let row = V::obj().with("user", user).with("tutorial_byte", req.i("tutorial_type"));
            db.upsert("tutorial", row, &["user", "tutorial_byte"], &[]);
            noop()
        }
        // tutorial/get_various_tutorial_data_list
        0xc204 => ok(V::obj().with("end_tutorial_type_list", list(db.filter("tutorial", |t| t.i("user") == user).iter().map(|t| V::Int(t.i("tutorial_byte"))).collect()))),
        0xc300 => discharge_profile_card(db, req),
        // card/discharge_resource_card
        0xc302 => {
            let mut serial = String::new();
            for x in req.get("common_reward_user_data").list() {
                serial = card_serial();
                let row = V::obj()
                    .with("user", user)
                    .with("serial", &serial)
                    .with("common_reward_type", x.i("common_reward_type"))
                    .with("common_reward_id", x.i("user_common_reward_id"))
                    .with("holographic_flag", req.i("holographic_flag"));
                db.upsert("resource_card", row, &["serial"], &["common_reward_type", "common_reward_id", "holographic_flag"]);
            }
            ok(V::obj().with("resource_card_code", serial))
        }
        0xc306 => scan_profile_card(db, req),
        // card/scan_qr_shop_resource_card
        0xc308 => {
            let known = db.find("resource_card", |c| c.s("serial") == req.s("resource_card_code")).is_some();
            Answer { value: V::obj().with("result", 1), err_status: if known { 0 } else { 4832 } }
        }
        // card/scan_qr_quest_resource_card (ARTEMiS' condition is inverted: a known card's reward)
        0xc30a => match db.find("resource_card", |c| c.s("serial") == req.s("resource_card_code")) {
            Some(card) => ok(V::obj()
                .with("common_reward_type", card.i("common_reward_type"))
                .with("common_reward_id", card.i("common_reward_id"))
                .with("holographic_flag", card.i("holographic_flag"))),
            None => Answer { value: V::obj().with("result", 1), err_status: 4832 },
        },
        0xc400 => yui_medal(db, user),
        // home/get_yui_medal_bonus_user_data
        0xc402 => ok(V::obj().with("data", list(vec![V::obj().with("elapsed_days", 0).with("loop_num", 0).with("last_check_date", EPOCH).with("last_get_date", EPOCH)]))),
        // home/check_comeback_event
        0xc404 => ok(V::obj().with("get_flag", 1)),
        // home/change_my_store
        0xc406 => {
            let store = req.s("store_id");
            let shop = i64::from_str_radix(store.get(3..).unwrap_or(""), 16).unwrap_or(0);
            db.set_profile(user, "my_shop", shop);
            noop()
        }
        // home/check_ac_login_bonus
        0xc40c => ok(V::obj().with("reward_get_flag", 1)),
        0xc500 => user_basic_data(db, user),
        // user_info/get_present_box_num
        0xc504 => ok(V::obj().with("num", 0).with("max_num", 0)),
        0xc600 => ok(V::obj().with("hero_log_user_data_list", list(db.filter("hero_log", |h| h.i("user") == user).iter().map(hero_with_static).collect()))),
        0xc602 => ok(V::obj().with("equipment_user_data_list", list(db.filter("equipment", |e| e.i("user") == user).iter().map(equipment_with_static).collect()))),
        // have_object/get_item_user_data_list
        0xc604 => ok(V::obj().with("item_user_data_list", list(db.filter("item", |i| i.i("user") == user).iter().map(item_user_data).collect()))),
        // have_object/get_support_log_user_data_list: every support log
        0xc606 => {
            let supports = table("SupportLog");
            let items = supports
                .iter()
                .enumerate()
                .map(|(x, s)| V::obj().with("user_support_log_id", format!("{user}{x}")).with("support_log_id", s.int("SupportLogId")).with("get_date", "19691231190000"))
                .collect();
            ok(V::obj().with("support_log_user_data_list", list(items)))
        }
        // custom/get_title_user_data_list: every title
        0xc800 => {
            let mut ids: Vec<i64> = table("Title").iter().map(|t| t.int("TitleId")).collect();
            ids.sort();
            ok(V::obj().with("title_user_data_list", list(ids.into_iter().map(|t| V::obj().with("user_title_id", format!("{user}{t}")).with("title_id", t)).collect())))
        }
        // custom/change_title: the game sends its title's user id (ARTEMiS': the user id and the
        // title's), ARTEMiS stores it as is; the title's id here
        0xc802 => {
            let user_title = req.s("user_title_id");
            let title = user_title.strip_prefix(&user.to_string()).filter(|t| !t.is_empty()).unwrap_or(&user_title).parse::<i64>().unwrap_or(0);
            db.set_profile(user, "setting_title_id", title);
            noop()
        }
        0xc804 => party_list(db, user),
        0xc806 => change_party(db, req),
        0xc812 => disposal(db, req),
        0xc814 => synthesize_hero(db, req),
        0xc816 => synthesize_equipment(db, req),
        0xc900 => quest_list(db, user),
        // quest/get_quest_scene_prev_scan_profile_card: one empty card
        0xc902 => ok(V::obj().with("profile_card_data", list(vec![V::obj()]))),
        0xc904 | 0xc914 => play_start(db, req, if cmd == 0xc904 { "episode_id" } else { "trial_tower_id" }),
        0xc908 => play_end(db, req, false),
        0xc918 => play_end(db, req, true),
        0xc90a | 0xc91a => log_fixed(db, user),
        // quest_multi_play_room/get_quest_scene_multi_play_photon_server
        0xca02 => ok(V::obj().with("application_id", PHOTON_APP_ID)),
        // defrag_match/get_defrag_match_ranking_user_data
        0xcd02 => {
            let p = db.profile(user).unwrap_or_default();
            ok(V::obj().with(
                "ranking_user_data",
                list(vec![V::obj().with("nick_name", p.s("nick_name")).with("setting_title_id", p.i("setting_title_id")).with("favorite_hero_log_id", p.i("fav_hero"))]),
            ))
        }
        // mission/get_beginner_mission_user_data
        0xd200 => {
            let mut data = V::obj().with("ad_confirm_flag", 0).with("ad_confirm_date", now());
            if let Some(p) = db.profile(user).filter(|p| p.get("ad_confirm_date").truthy()) {
                data.set("ad_confirm_date", p.s("ad_confirm_date"));
                data.set("ad_confirm_flag", 1);
            }
            ok(V::obj().with("data", list(vec![data])))
        }
        // mission/beginner_mission_ad_confirm_notification
        0xd206 => {
            db.set_profile(user, "ad_confirm_date", now());
            noop()
        }
        _ => {
            if let Some(v) = super::data::master(cmd) {
                return Answer { value: v, err_status: 0 };
            }
            noop()
        }
    };
    db.save();
    answer
}

impl Answer {
    fn value_with(mut self, key: &str, value: impl Into<V>) -> Answer {
        self.value.set(key, value);
        self
    }
}

/// A printed card's serial: ARTEMiS anchors its own with 2 (the real ones start 10..17).
fn card_serial() -> String {
    format!("2{:018}", super::random() % 999_999_999_999_999_999 + 1)
}

/// common/login: the card's user (registered on its first login, with ARTEMiS' starting heroes
/// and equipment), 100 VP once a day.
fn login(db: &mut Db, req: &V) -> Answer {
    let code = req.s("access_code");
    let chip = req.s("chip_id");
    let idm: String = chip.chars().take(16).collect();
    let user = match db.card_by_access_code(&code) {
        Some(card) => card.i("user"),
        None => {
            let user = match db.card_by_idm(&idm) {
                Some(card) => {
                    let id = card.i("id");
                    db.set("card", id, "access_code", code.as_str());
                    card.i("user")
                }
                None => {
                    let user = db.create_user();
                    db.create_card(user, &code);
                    user
                }
            };
            crate::log(&format!("sao: card {code} registered to user {user}"));
            if let Some(card) = db.find_mut("card", |c| c.s("access_code") == code) {
                if code.starts_with('5') {
                    card.set("idm", idm.as_str());
                } else if (code.starts_with("010") || code.starts_with('3')) && i64::from_str_radix(chip.get(..8).unwrap_or(""), 16).unwrap_or(0) != 0x0403_0201 {
                    card.set("chip_id", i64::from_str_radix(chip.get(..8).unwrap_or(""), 16).unwrap_or(0));
                }
            }
            user
        }
    };
    let answer = match db.profile(user) {
        None => {
            db.create_profile(user);
            let equip = [101000000, 102000000, 109000000].map(|e| db.put_equipment(user, e));
            let heroes = [(101000010, equip[0], [1002, 1003, 1014]), (102000010, equip[1], [3001, 3002, 3004]), (105000010, equip[2], [10005, 10002, 10004])];
            let ids = heroes.map(|(hero, weapon, s)| db.put_hero_log(user, hero, 1, 0, V::Int(weapon), V::Null, [V::Int(s[0]), V::Int(s[1]), V::Int(s[2]), V::Null, V::Null]));
            db.put_hero_party(user, 0, ids);
            crate::log(&format!("sao: new player {user}"));
            login_answer(user, true, false)
        }
        Some(p) => {
            // midnight today (the date's start), as the saved dates
            let today = &now()[..8];
            let logged_today = p.get("last_login_date").truthy() && p.s("last_login_date").as_str() > &format!("{today}000000")[..];
            if !logged_today {
                db.add_profile(user, "own_vp", 100);
            }
            login_answer(user, p.i("login_ct") < 1, logged_today)
        }
    };
    if let Some(p) = db.profile_mut(user) {
        let n = p.i("login_ct") + 1;
        p.set("login_ct", n);
        p.set("last_login_date", now());
    }
    answer
}

fn login_answer(user: i64, first_play: bool, logged_today: bool) -> Answer {
    ok(V::obj()
        .with("user_id", user.to_string())
        .with("first_play_flag", first_play)
        .with("grantable_free_ticket_flag", !logged_today)
        .with("login_reward_vp", if logged_today { 0 } else { 100 })
        .with("today_paying_flag", logged_today))
}

/// common/get_auth_card_data: the card's player name, "NEW PLAYER" for a card not registered.
fn auth_card_data(db: &mut Db, req: &V) -> Answer {
    let code = req.s("access_code");
    let idm: String = req.s("chip_id").chars().take(16).collect();
    let card = match db.card_by_access_code(&code) {
        Some(c) => Some(c),
        None => db.card_by_idm(&idm).inspect(|c| {
            let id = c.i("id");
            db.set("card", id, "access_code", code.as_str());
        }),
    };
    let answer = |name: &str, user: i64| ok(V::obj().with("unused_card_flag", "").with("first_play_flag", 0).with("tutorial_complete_flag", 1).with("nick_name", name).with("personal_id", user.to_string()));
    let Some(card) = card else {
        crate::log(&format!("sao: new card {code}"));
        return answer("NEW PLAYER", 0);
    };
    let user = card.i("user");
    match db.profile(user) {
        Some(p) => answer(&p.s("nick_name"), user),
        None => answer("NEW PLAYER", user),
    }
}

/// The skills a hero starts with: its skill table's level 1 skills without awakening, each in
/// the slot of its level. ARTEMiS' `hero_default_skills` means it, but never fills a slot.
fn default_skills(skill_table_sub_id: i64) -> [V; 5] {
    let mut slots: [V; 5] = Default::default();
    for row in table("SkillTable").filter("SkillTableSubId", skill_table_sub_id) {
        if row.int("Level") != 1 || row.int("AwakeningId") != 0 {
            continue;
        }
        let skill = row.int("SkillId");
        let Some(info) = table("Skill").find("SkillId", skill) else { continue };
        let slot = (info.int("Level") - 1).clamp(0, 4) as usize;
        if slots[slot].is_null() {
            slots[slot] = V::Int(skill);
        }
    }
    slots
}

/// `add_reward`: a hero, equipment or item given, or Col, VP, Yui medals added.
fn add_reward(db: &mut Db, reward: Reward, user: i64) {
    match reward.kind {
        REWARD_HERO_LOG => {
            let sub = table("HeroLog").find("HeroLogId", reward.id).map_or(0, |h| h.int("SkillTableSubId"));
            let id = db.put_hero_log(user, reward.id, 1, 0, V::Null, V::Null, default_skills(sub));
            crate::log(&format!("sao: user {user} gets hero {} ({id})", reward.id));
        }
        REWARD_EQUIPMENT => {
            db.put_equipment(user, reward.id);
        }
        REWARD_ITEM => {
            db.put_item(user, reward.id);
        }
        REWARD_COL => db.add_profile(user, "own_col", reward.num),
        REWARD_VP => db.add_profile(user, "own_vp", reward.num),
        REWARD_YUI_MEDAL => db.add_profile(user, "own_yui_medal", reward.num),
        k => crate::log(&format!("sao: user {user}: reward type {k} not given")),
    }
}

/// card/discharge_profile_card (the terminal's print): a new card serial for the hero
/// (ARTEMiS' condition is inverted: the player's hero, given when missing).
fn discharge_profile_card(db: &mut Db, req: &V) -> Answer {
    let user = req.i("user_id");
    let mut serial = card_serial();
    while db.find("hero_card", |c| c.s("serial") == serial).is_some() {
        serial = card_serial();
    }
    let hero_log_id = req.i("hero_log_user_hero_log_id");
    let hero = match db.find("hero_log", |h| h.i("user") == user && h.i("hero_log_id") == hero_log_id) {
        Some(h) => h.i("id"),
        None => {
            let Some(sub) = table("HeroLog").find("HeroLogId", hero_log_id).map(|h| h.int("SkillTableSubId")) else {
                return ok(V::obj().with("profile_card_code", serial));
            };
            db.put_hero_log(user, hero_log_id, 1, 0, V::Null, V::Null, default_skills(sub))
        }
    };
    let row = V::obj().with("user", user).with("serial", &serial).with("user_hero_id", hero).with("holographic_flag", req.i("holographic_flag"));
    db.upsert("hero_card", row, &["serial"], &["user_hero_id", "holographic_flag"]);
    ok(V::obj().with("profile_card_code", serial))
}

/// card/scan_qr_quest_profile_card: a printed card's player and hero.
fn scan_profile_card(db: &mut Db, req: &V) -> Answer {
    let code = req.s("profile_card_code");
    let Some(card) = db.find("hero_card", |c| c.s("serial") == code).cloned() else {
        crate::log(&format!("sao: unknown profile card {code}"));
        return noop();
    };
    let (Some(hero), Some(p)) = (db.by_id("hero_log", card.i("user_hero_id")).cloned(), db.profile(card.i("user"))) else {
        return noop();
    };
    let data = V::obj()
        .with("profile_card_code", code)
        .with("nick_name", p.s("nick_name"))
        .with("rank_num", p.i("rank_num"))
        .with("setting_title_id", p.i("setting_title_id"))
        .with("skill_id", hero.i("skill_slot1_skill_id"))
        .with("hero_log_hero_log_id", hero.i("hero_log_id"))
        .with("hero_log_log_level", hero.i("log_level"))
        .with("hero_log_awakening_stage", 1);
    ok(V::obj().with("read_profile_card_data", list(vec![data])))
}

/// home/check_yui_medal_get_condition: a Yui medal for a day's first play after a day without.
fn yui_medal(db: &mut Db, user: i64) -> Answer {
    let p = db.profile(user).unwrap_or_default();
    let days = if p.get("last_yui_medal_date").truthy() {
        let last = super::parse_time(&p.s("last_yui_medal_date")).unwrap_or(0);
        let today = super::parse_time(&format!("{}000000", &now()[..8])).unwrap_or(0);
        if today > last { (today - last) / 86400 } else { 0 }
    } else {
        1
    };
    if days > 1 {
        db.add_profile(user, "own_yui_medal", 1);
    }
    db.set_profile(user, "last_yui_medal_date", now());
    let get = (days > 1) as i64;
    ok(V::obj().with("get_flag", get).with("elapsed_days", days).with("get_yui_medal_num", get))
}

/// user_info/get_user_basic_data: the profile, its rank's experience.
fn user_basic_data(db: &mut Db, user: i64) -> Answer {
    let Some(p) = db.profile(user) else { return noop() };
    let rank = p.i("rank_num");
    let rank_exp = table("PlayerRank").find("PlayerRankId", rank).map_or(p.i("rank_exp"), |r| p.i("rank_exp") - r.int("TotalExp"));
    let shop = p.i("my_shop");
    let data = V::obj()
        .with("user_type", p.i("user_type"))
        .with("nick_name", p.s("nick_name"))
        .with("rank_num", rank)
        .with("rank_exp", rank_exp)
        .with("own_col", p.i("own_col"))
        .with("own_vp", p.i("own_vp"))
        .with("own_yui_medal", p.i("own_yui_medal"))
        .with("setting_title_id", p.i("setting_title_id"))
        .with("favorite_user_hero_log_id", p.s("fav_hero"))
        .with("favorite_user_support_log_id", "")
        .with("my_store_id", if shop != 0 { format!("JPN0{shop:04X}") } else { "0".into() })
        // ARTEMiS' default shop name is its own
        .with("my_store_name", "Arcade")
        .with("user_reg_date", if p.get("when_register").truthy() { p.s("when_register") } else { "20230101120000".into() });
    ok(V::obj().with("user_basic_data", list(vec![data])))
}

/// `HeroLogUserData.from_args`. ARTEMiS sends property 2's id as properties 3 and 4's: each
/// its own here.
fn hero_user_data(h: &V) -> V {
    let mut v = V::obj()
        .with("user_hero_log_id", h.i("id").to_string())
        .with("hero_log_id", h.i("hero_log_id"))
        .with("log_level", h.i("log_level"))
        .with("max_log_level_extended_num", h.i("max_level_extend_num"))
        .with("log_exp", h.i("log_exp"))
        .with("possible_awakening_flag", h.i("is_awakenable"))
        .with("awakening_stage", h.i("awakening_stage"))
        .with("awakening_exp", h.i("awakening_exp"))
        .with("skill_slot_correction_value", 0);
    for s in 1..=5 {
        v.set(&format!("last_set_skill_slot{s}_skill_id"), h.i(&format!("skill_slot{s}_skill_id")));
    }
    for p in 1..=4 {
        for f in ["property_id", "value1", "value2"] {
            let k = format!("property{p}_{f}");
            v.set(&k, h.i(&k));
        }
    }
    v.with("converted_card_num", h.i("converted_card_num"))
        .with("shop_purchase_flag", h.i("is_shop_purchase"))
        .with("protect_flag", h.i("is_protect"))
        .with("get_date", h.s("get_date"))
}

/// The static properties of a hero or equipment (`Property1PropertyId`..).
fn static_properties(v: &mut V, row: Row, only_missing: bool) {
    for p in 1..=4 {
        if only_missing && v.i(&format!("property{p}_property_id")) != 0 {
            continue;
        }
        v.set(&format!("property{p}_property_id"), row.int(&format!("Property{p}PropertyId")));
        v.set(&format!("property{p}_value1"), row.int(&format!("Property{p}Value1")));
        v.set(&format!("property{p}_value2"), row.int(&format!("Property{p}Value2")));
    }
}

/// have_object/get_hero_log_user_data_list's hero: its static properties, its level's
/// experience.
fn hero_with_static(h: &V) -> V {
    let mut v = hero_user_data(h);
    if let Some(row) = table("HeroLog").find("HeroLogId", h.i("hero_log_id")) {
        static_properties(&mut v, row, false);
        if let Some(level) = table("HeroLogLevel").find("HeroLogLevelId", h.i("log_level")) {
            v.set("log_exp", h.i("log_exp") - level.int("TotalExp"));
        }
    }
    v
}

/// `EquipmentUserData.from_args` (the same property fix as heroes').
fn equipment_user_data(e: &V) -> V {
    let mut v = V::obj()
        .with("user_equipment_id", e.i("id").to_string())
        .with("equipment_id", e.i("equipment_id"))
        .with("enhancement_value", e.i("enhancement_value"))
        .with("max_enhancement_value_extended_num", 0)
        .with("enhancement_exp", e.i("enhancement_exp"))
        .with("possible_awakening_flag", e.i("possible_awakening_flag"))
        .with("awakening_stage", e.i("awakening_stage"))
        .with("awakening_exp", e.i("awakening_exp"));
    for p in 1..=4 {
        for f in ["property_id", "value1", "value2"] {
            let k = format!("property{p}_{f}");
            v.set(&k, e.i(&k));
        }
    }
    v.with("converted_card_num", e.i("converted_card_num"))
        .with("shop_purchase_flag", e.i("is_shop_purchase"))
        .with("protect_flag", e.i("is_protect"))
        .with("get_date", e.s("get_date"))
}

/// have_object/get_equipment_user_data_list's equipment: the static properties it has none of
/// (ARTEMiS' conditions for properties 3 and 4 are inverted), its level's experience.
fn equipment_with_static(e: &V) -> V {
    let mut v = equipment_user_data(e);
    if let Some(row) = table("Equipment").find("EquipmentId", e.i("equipment_id")) {
        static_properties(&mut v, row, true);
        if let Some(level) = table("EquipmentLevel").find("EquipmentLevelId", e.i("enhancement_value")) {
            v.set("enhancement_exp", e.i("enhancement_exp") - level.int("TotalExp"));
        }
    }
    v
}

fn item_user_data(i: &V) -> V {
    V::obj().with("user_item_id", i.i("id").to_string()).with("item_id", i.i("item_id")).with("protect_flag", 0).with("get_date", i.s("get_date"))
}

/// custom/get_party_data_list: each party, its three heroes.
fn party_list(db: &mut Db, user: i64) -> Answer {
    let parties = db.filter("hero_party", |p| p.i("user") == user);
    let items = parties
        .iter()
        .map(|party| {
            let (id, team) = (party.i("id"), party.i("user_party_team_id"));
            let members = (1..=3)
                .map(|n| {
                    let hero = db.by_id("hero_log", party.i(&format!("user_hero_log_id_{n}"))).cloned().unwrap_or_default();
                    let mut m = V::obj()
                        .with("user_party_team_id", format!("{id}-{team}-{n}"))
                        .with("arrangement_num", n)
                        .with("user_hero_log_id", hero.s("id"))
                        .with("main_weapon_user_equipment_id", hero.s("main_weapon"))
                        .with("sub_equipment_user_equipment_id", hero.s("sub_equipment"));
                    for s in 1..=5 {
                        m.set(&format!("skill_slot{s}_skill_id"), hero.i(&format!("skill_slot{s}_skill_id")));
                    }
                    m
                })
                .collect();
            V::obj().with("user_party_id", id.to_string()).with("team_no", team).with("party_team_data_list", list(members))
        })
        .collect();
    ok(V::obj().with("party_data_list", list(items)))
}

/// custom/change_party: each hero's weapons and skills, each team's heroes.
fn change_party(db: &mut Db, req: &V) -> Answer {
    let user = req.i("user_id");
    let positive = |n: i64| if n > 0 { V::Int(n) } else { V::Null };
    for party in req.get("party_data_list").list() {
        let members = party.get("party_team_data_list").list();
        for hero in members {
            let id = hero.i("user_hero_log_id");
            db.set("hero_log", id, "main_weapon", positive(hero.i("main_weapon_user_equipment_id")));
            db.set("hero_log", id, "sub_equipment", positive(hero.i("sub_equipment_user_equipment_id")));
            for s in 1..=5 {
                let k = format!("skill_slot{s}_skill_id");
                db.set("hero_log", id, &k, positive(hero.i(&k)));
            }
        }
        if members.len() >= 3 {
            db.put_hero_party(user, party.i("team_no"), [members[0].i("user_hero_log_id"), members[1].i("user_hero_log_id"), members[2].i("user_hero_log_id")]);
        }
    }
    noop()
}

/// custom/disposal_resource: heroes, equipment and items sold for their price in Col (ARTEMiS'
/// removal returns nothing, so it never adds the price; an item's is looked up as equipment's).
fn disposal(db: &mut Db, req: &V) -> Answer {
    let user = req.i("user_id");
    let mut col = 0;
    for d in req.get("disposal_common_reward_user_data_list").list() {
        let id = d.i("user_common_reward_id");
        match d.i("common_reward_type") {
            REWARD_HERO_LOG => {
                if let Some(h) = db.remove("hero_log", id) {
                    col += table("HeroLog").find("HeroLogId", h.i("hero_log_id")).map_or(0, |r| r.int("SalePrice"));
                }
            }
            REWARD_EQUIPMENT => {
                if let Some(e) = db.remove("equipment", id) {
                    col += table("Equipment").find("EquipmentId", e.i("equipment_id")).map_or(0, |r| r.int("SalePrice"));
                }
            }
            REWARD_ITEM => {
                if let Some(i) = db.remove("item", id) {
                    col += table("Item").find("ItemId", i.i("item_id")).map_or(0, |r| r.int("SalePrice"));
                }
            }
            REWARD_SUPPORT_LOG => {}
            k => crate::log(&format!("sao: disposal of type {k} not handled")),
        }
    }
    db.add_profile(user, "own_col", col);
    ok(V::obj().with("get_col", col))
}

/// The experience the materials of a synthesis give (removed), as ARTEMiS counts it: items of
/// type 7 their value, equipment and heroes their composition experience and their next level's.
fn materials_exp(db: &mut Db, req: &V) -> i64 {
    let hero_levels = table("HeroLogLevel");
    let equip_levels = table("EquipmentLevel");
    let next_require = |levels: &super::data::Table, col: &str, level: i64| -> i64 {
        (0..levels.len()).find(|&e| levels.row(e).is_some_and(|r| r.int(col) == level)).and_then(|e| levels.row(e + 1)).map_or(0, |r| r.int("RequireExp"))
    };
    let mut exp = 0;
    for x in req.get("material_common_reward_user_data_list").list() {
        let id = x.i("user_common_reward_id");
        match x.i("common_reward_type") {
            REWARD_ITEM => {
                let Some(item) = db.by_id("item", id) else { continue };
                if let Some(s) = table("Item").find("ItemId", item.i("item_id")).filter(|s| s.int("ItemTypeId") == 7) {
                    exp += s.int("Value");
                    db.remove("item", id);
                }
            }
            REWARD_EQUIPMENT => {
                let Some(e) = db.by_id("equipment", id).cloned() else { continue };
                let req_exp = next_require(equip_levels, "EquipmentLevelId", e.i("enhancement_value"));
                exp += table("Equipment").find("EquipmentId", e.i("equipment_id")).map_or(0, |s| s.int("CompositionExp")) + req_exp;
                db.remove("equipment", id);
            }
            REWARD_HERO_LOG => {
                let Some(h) = db.by_id("hero_log", id).cloned() else { continue };
                let req_exp = next_require(hero_levels, "HeroLogLevelId", h.i("log_level"));
                exp += table("HeroLog").find("HeroLogId", h.i("hero_log_id")).map_or(0, |s| s.int("CompositionExp")) + req_exp;
                db.remove("hero_log", id);
            }
            k => crate::log(&format!("sao: synthesis material type {k} not handled")),
        }
    }
    (exp as f64 * 1.5) as i64
}

/// The level row whose total experience range holds `exp` (ARTEMiS stops before the last).
fn level_of<'a>(levels: &'a super::data::Table, exp: i64) -> Option<Row<'a>> {
    (0..levels.len().saturating_sub(1)).find_map(|e| {
        let (r, next) = (levels.row(e)?, levels.row(e + 1)?);
        (exp >= r.int("TotalExp") && exp < next.int("TotalExp")).then_some(r)
    })
}

/// The Col a synthesis costs (ARTEMiS' formula).
fn synthesis_cost(level: i64, materials: usize, awakening: i64) -> i64 {
    let mut cost = level as f64 * (100.0 * materials as f64) * (1.0 - awakening as f64 * 0.1);
    if level >= 100 {
        cost *= 10.0;
    }
    ((cost * 1000.0) as i64 / 1000).max(100)
}

/// custom/synthesize_enhancement_hero_log
fn synthesize_hero(db: &mut Db, req: &V) -> Answer {
    let user = req.i("user_id");
    let origin = req.i("origin_user_hero_log_id");
    let exp = materials_exp(db, req);
    db.add("hero_log", origin, "log_exp", exp);
    let log_exp = db.by_id("hero_log", origin).map_or(0, |h| h.i("log_exp"));
    let levels = table("HeroLogLevel");
    let mut remain = log_exp;
    if let Some(level) = level_of(levels, log_exp) {
        remain = log_exp - level.int("TotalExp");
        db.set("hero_log", origin, "log_level", level.int("HeroLogLevelId"));
    }
    let pre_level = level_of(levels, log_exp - exp).map_or(1, |r| r.int("HeroLogLevelId"));
    let Some(hero) = db.by_id("hero_log", origin).cloned() else { return noop() };
    let cost = synthesis_cost(pre_level, req.get("material_common_reward_user_data_list").list().len(), hero.i("awakening_stage"));
    crate::log(&format!("sao: hero {origin} +{exp} exp, {cost} Col"));
    db.add_profile(user, "own_col", -cost);
    ok(V::obj().with("after_hero_log_user_data", list(vec![hero_user_data(&hero).with("log_exp", remain)])))
}

/// custom/synthesize_enhancement_equipment (ARTEMiS reads the player's first equipment back,
/// its query's condition being a Python `and`: the synthesized one here).
fn synthesize_equipment(db: &mut Db, req: &V) -> Answer {
    let user = req.i("user_id");
    let origin = req.i("origin_user_equipment_id");
    let exp = materials_exp(db, req);
    db.add("equipment", origin, "enhancement_exp", exp);
    let Some(equip) = db.by_id("equipment", origin).cloned() else { return noop() };
    let (new_exp, pre_level) = (equip.i("enhancement_exp"), equip.i("enhancement_value"));
    let (mut new_level, mut remain) = (1, new_exp);
    if let Some(level) = level_of(table("EquipmentLevel"), new_exp) {
        new_level = level.int("EquipmentLevelId");
        remain = new_exp - level.int("TotalExp");
        db.set("equipment", origin, "enhancement_value", new_level);
    }
    let cost = synthesis_cost(pre_level, req.get("material_common_reward_user_data_list").list().len(), equip.i("awakening_stage"));
    crate::log(&format!("sao: equipment {origin} +{exp} exp, {cost} Col"));
    db.add_profile(user, "own_col", -cost);
    let after = equipment_user_data(&equip).with("enhancement_exp", remain).with("enhancement_value", new_level);
    ok(V::obj().with("after_equipment_user_data", list(vec![after])))
}

/// quest/get_quest_scene_user_data_list: each quest played, its best score and EX bonuses.
fn quest_list(db: &mut Db, user: i64) -> Answer {
    let quests = db.filter("quest", |q| q.i("user") == user);
    let items = quests
        .iter()
        .map(|q| {
            let scene = q.i("quest_scene_id");
            let bonuses = db
                .filter("ex_bonus", |b| b.i("user") == user && b.i("quest_scene_id") == scene)
                .iter()
                .map(|b| V::obj().with("ex_bonus_table_id", b.i("ex_bonus_table_id")).with("achievement_flag", b.i("quest_clear_flag")))
                .collect();
            V::obj()
                .with("quest_type", q.i("quest_type"))
                .with("quest_scene_id", scene)
                .with("clear_flag", q.i("quest_clear_flag"))
                .with(
                    "quest_scene_best_score_user_data",
                    list(vec![V::obj()
                        .with("clear_time", q.i("clear_time"))
                        .with("combo_num", q.i("combo_num"))
                        .with("total_damage", q.i("total_damage").to_string())
                        .with("concurrent_destroying_num", q.i("concurrent_destroying_num"))]),
                )
                .with("quest_scene_ex_bonus_user_data_list", list(bonuses))
        })
        .collect();
    ok(V::obj().with("quest_scene_user_data_list", list(items)))
}

/// quest/episode_play_start, quest/trial_tower_play_start: a play session (its party).
fn play_start(db: &mut Db, req: &V, quest_field: &str) -> Answer {
    let user = req.i("user_id");
    let start = req.get("play_start_request_data").list().first().cloned().unwrap_or_default();
    let row = V::obj()
        .with("user", user)
        .with("user_party_team_id", start.i("user_party_id"))
        .with("episode_id", req.i(quest_field))
        .with("play_mode", req.i("play_mode"))
        .with("quest_drop_boost_apply_flag", start.i("quest_drop_boost_apply_flag"))
        .with("play_date", now());
    let session = db.insert("session", row);
    let nick = db.profile(user).map(|p| p.s("nick_name")).unwrap_or_default();
    crate::log(&format!("sao: user {user} starts {quest_field} {}", req.i(quest_field)));
    ok(V::obj()
        .with("play_start_response_data", list(vec![V::obj().with("appearance_player_trace_data_list", list(vec![V::obj().with("user_quest_scene_player_trace_id", session.to_string()).with("nick_name", nick)]))]))
        .with("multi_play_start_response_data", list(vec![V::obj()])))
}

/// quest/episode_play_end, quest/trial_tower_play_end: Col, VP and player experience, the
/// quest's best score, the EX bonuses met, the drops, the logs' rewards (kept for the
/// unanalyzed log answer), the party's heroes' experience.
fn play_end(db: &mut Db, req: &V, tower: bool) -> Answer {
    let user = req.i("user_id");
    let quest_id = req.i(if tower { "trial_tower_id" } else { "episode_id" });
    let end = req.get("play_end_request_data").list().first().cloned().unwrap_or_default();
    let base = end.get("base_get_data").list().first().cloned().unwrap_or_default();
    let score = end.get("score_data").list().first().cloned().unwrap_or_default();

    let mut response = V::obj().with("ex_bonus_data_list", list(vec![])).with("play_end_player_trace_reward_data_list", list(vec![]));
    let mut answer = V::obj().with("multi_play_end_response_data", list(vec![V::obj()]));
    if tower {
        answer.set("trial_tower_play_end_updated_notification_data", list(vec![V::obj()]));
        answer.set("treasure_hunt_play_end_response_data", list(vec![V::obj()]));
    }
    let quest = table(if tower { "TrialTower" } else { "Episode" }).find(if tower { "TrialTowerId" } else { "EpisodeId" }, quest_id);
    let Some(quest) = quest else {
        crate::log(&format!("sao: unknown quest {quest_id}"));
        return ok(answer.with("play_end_response_data", list(vec![response])));
    };
    let scene_id = quest.int("QuestSceneId");
    let scene = table("QuestScene").find("QuestSceneId", scene_id);
    let scene_int = |col: &str| scene.map_or(0, |s| s.int(col));

    db.add_profile(user, "own_col", base.i("get_col"));
    let clear = score.i("clear_time") > 0;
    let (clear_time, combo, damage, concurrent) = (score.i("clear_time"), score.i("combo_num"), score.i("total_damage"), score.i("concurrent_destroying_num"));
    if clear {
        db.add_profile(user, "own_vp", scene_int("SingleRewardVp"));
        db.add_profile(user, "rank_exp", scene_int("SuccessPlayerExp"));
    } else {
        db.add_profile(user, "rank_exp", scene_int("FailedPlayerExp"));
    }
    let exp = db.profile(user).map_or(0, |p| p.i("rank_exp"));
    if let Some(rank) = level_of(table("PlayerRank"), exp) {
        db.set_profile(user, "rank_num", rank.int("PlayerRankId"));
    }
    let quest_type = if tower { QUEST_TRIAL_TOWER } else { QUEST_EPISODE };
    match db.find("quest", |q| q.i("user") == user && q.i("quest_scene_id") == scene_id).cloned() {
        Some(cur) => {
            let cleared = clear || cur.i("quest_clear_flag") != 0;
            let time = if clear { clear_time.min(cur.i("clear_time")) } else { cur.i("clear_time") };
            db.put_player_quest(user, quest_type, scene_id, cleared, time, combo.max(cur.i("combo_num")), damage.max(cur.i("total_damage")), concurrent.max(cur.i("concurrent_destroying_num")));
        }
        None => {
            db.put_player_quest(user, quest_type, scene_id, clear, clear_time, combo, damage, concurrent);
        }
    }

    let (mut missions, mut hardest) = (0, 0);
    for m in end.get("mission_data_list").list() {
        if m.i("clear_flag") == 1 {
            missions += 1;
            hardest = hardest.max(m.i("mission_difficulty_id"));
        }
    }
    let mut defeated = 0;
    let mut by_kind = std::collections::HashMap::new();
    for e in end.get("discovery_enemy_data_list").list() {
        defeated += e.i("destroy_num");
        by_kind.insert(e.i("enemy_kind_id"), e.i("destroy_num"));
    }
    let logs = end.get("get_unanalyzed_log_tmp_reward_data_list").list();
    for bonus in table("ExBonusTable").filter("ExBonusTableSubId", quest.int("ExBonusTableSubId")) {
        let table_id = bonus.int("ExBonusTableId");
        let (v1, v2) = (bonus.int("ConditionValue1"), bonus.int("ConditionValue2"));
        let met = match bonus.int("ExBonusConditionId") {
            1 => clear && clear_time / 1000 < v1,
            2 => by_kind.get(&v1).copied().unwrap_or(0) >= v2,
            3 => defeated >= v1,
            4 => missions >= v1,
            5 => hardest >= v1,
            6 => logs.len() as i64 >= v1,
            7 => score.i("reaching_skill_level") >= v1,
            8 => score.i("total_loss_num") == 0,
            9 => score.i("acceleration_invocation_num") >= v1,
            10 => combo >= v1,
            // MULTIPLAYER_CLEAR_X: not counted (ARTEMiS' TODO)
            11 => false,
            c => {
                crate::log(&format!("sao: EX bonus condition {c} not handled"));
                false
            }
        };
        if met {
            add_reward(db, bonus.reward(), user);
            db.put_ex_bonus(user, scene_id, table_id, true);
        }
        response.push("ex_bonus_data_list", V::obj().with("ex_bonus_table_id", table_id).with("achievement_status", if met { 2 } else { 0 }));
    }
    crate::log(&format!("sao: user {user} {} {} {quest_id}", if clear { "cleared" } else { "ended" }, if tower { "trial tower" } else { "episode" }));

    for drop in end.get("get_rare_drop_data_list").list() {
        if let Some(row) = table("QuestRareDrop").find("QuestRareDropId", drop.i("quest_rare_drop_id")) {
            add_reward(db, row.reward(), user);
        }
    }
    let rewards = table("RewardTable").filter("RewardTableSubId", scene_int("RewardTableSubId"));
    let mut kept = Vec::new();
    for log in logs {
        let grade = log.i("unanalyzed_log_grade_id");
        let able: Vec<&Row> = rewards.iter().filter(|r| r.int("UnanalyzedLogGradeId") == grade).collect();
        if let Some(i) = choice(able.len()) {
            let r = able[i];
            add_reward(db, r.reward(), user);
            kept.push(
                V::obj()
                    .with("UnanalyzedLogGradeId", grade)
                    .with("CommonRewardType", r.int("CommonRewardType"))
                    .with("CommonRewardId", r.int("CommonRewardId"))
                    .with("CommonRewardNum", r.int("CommonRewardNum")),
            );
        }
    }
    let traces = table("PlayerTraceTable").filter("PlayerTraceTableSubId", scene_int("PlayerTraceTableSubId"));
    for _ in end.get("get_player_trace_data_list").list() {
        if let Some(i) = choice(traces.len()) {
            let r = traces[i].reward();
            response.push("play_end_player_trace_reward_data_list", V::obj().with("common_reward_data", list(vec![reward_data(r)])));
        }
    }
    let end_row = V::obj().with("user", user).with("quest_id", scene_id).with("play_result_flag", end.i("play_result_flag")).with("reward_data", V::List(kept)).with("play_date", now());
    db.insert("end_session", end_row);

    // the party's heroes
    let session = db.latest("session", user).unwrap_or_default();
    match db.by_id("hero_party", session.i("user_party_team_id")).cloned() {
        Some(party) => {
            for n in 1..=3 {
                let hero = party.i(&format!("user_hero_log_id_{n}"));
                db.add("hero_log", hero, "log_exp", base.i("get_hero_log_exp"));
                let log_exp = db.by_id("hero_log", hero).map_or(0, |h| h.i("log_exp"));
                if log_exp != 0
                    && let Some(level) = level_of(table("HeroLogLevel"), log_exp)
                {
                    db.set("hero_log", hero, "log_level", level.int("HeroLogLevelId"));
                }
            }
        }
        None => crate::log(&format!("sao: no party {} for user {user}", session.i("user_party_team_id"))),
    }
    ok(answer.with("play_end_response_data", list(vec![response])))
}

fn reward_data(r: Reward) -> V {
    V::obj().with("common_reward_type", r.kind).with("common_reward_id", r.id).with("common_reward_num", r.num)
}

/// quest/*_play_end_unanalyzed_log_fixed: the rewards the last play's logs gave.
fn log_fixed(db: &mut Db, user: i64) -> Answer {
    let session = db.latest("end_session", user).unwrap_or_default();
    let items = session
        .get("reward_data")
        .list()
        .iter()
        .map(|d| {
            let r = Reward { kind: d.i("CommonRewardType"), id: d.i("CommonRewardId"), num: d.i("CommonRewardNum") };
            V::obj().with("unanalyzed_log_grade_id", d.i("UnanalyzedLogGradeId")).with("common_reward_data", list(vec![reward_data(r)]))
        })
        .collect();
    ok(V::obj().with("play_end_unanalyzed_log_reward_data_list", list(items)))
}
