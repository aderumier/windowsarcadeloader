//! The players' data: ARTEMiS' tables (`core` `aime_card` / `aime_user`, `titles/sao/schema`
//! `profile.py` and `item.py`), each a list of rows (objects with an `id`), kept in one JSON file
//! (`sao.json` in the server's directory), written after each change. The unique keys of
//! ARTEMiS' tables make its inserts updates of the existing row (`on_duplicate_key_update`): the
//! same here ([`Db::upsert`]).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::value::V;

#[derive(Default)]
pub(crate) struct Db {
    tables: BTreeMap<String, Vec<V>>,
    next: BTreeMap<String, i64>,
}

static DB: Mutex<Option<Db>> = Mutex::new(None);
static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Loads the data file (`dir\sao.json`).
pub(crate) fn open(dir: &str) {
    let path = PathBuf::from(dir).join("sao.json");
    let mut db = Db::default();
    if let Some(V::Obj(tables)) = std::fs::read_to_string(&path).ok().and_then(|t| V::from_json(&t)) {
        for (name, rows) in tables {
            if let V::List(rows) = rows {
                let next = rows.iter().map(|r| r.i("id")).max().unwrap_or(0) + 1;
                db.next.insert(name.clone(), next);
                db.tables.insert(name, rows);
            }
        }
        crate::log(&format!("sao: players' data {}", path.display()));
    }
    let _ = FILE.set(path);
    *DB.lock().unwrap() = Some(db);
}

/// The data, locked (saved when the guard's handler calls [`Db::save`]).
pub(crate) fn lock() -> MutexGuard<'static, Option<Db>> {
    DB.lock().unwrap()
}

/// The current time as ARTEMiS writes it (`func.now()`: a date, `%Y%m%d%H%M%S`).
pub(crate) fn now() -> String {
    super::format_time(super::unix_now())
}

impl Db {
    /// Writes the data file.
    pub(crate) fn save(&self) {
        let Some(path) = FILE.get() else { return };
        let mut text = String::new();
        V::Obj(self.tables.iter().map(|(k, v)| (k.clone(), V::List(v.clone()))).collect()).to_json(&mut text);
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)) {
            crate::log(&format!("sao: {}: {e}", path.display()));
        }
    }

    pub(crate) fn rows(&self, table: &str) -> &[V] {
        self.tables.get(table).map_or(&[], |r| r.as_slice())
    }

    /// Inserts `row` with a new id; the id.
    pub(crate) fn insert(&mut self, table: &str, mut row: V) -> i64 {
        let next = self.next.entry(table.to_string()).or_insert(1);
        let id = *next;
        *next += 1;
        row.set("id", id);
        self.tables.entry(table.to_string()).or_default().push(row);
        id
    }

    /// Inserts `row`, or updates the row with the same `key` columns with `update`'s fields; the
    /// row's id.
    pub(crate) fn upsert(&mut self, table: &str, row: V, key: &[&str], update: &[&str]) -> i64 {
        if let Some(existing) = self.tables.get_mut(table).and_then(|rows| rows.iter_mut().find(|r| key.iter().all(|k| r.get(k) == row.get(k)))) {
            for f in update {
                existing.set(f, row.get(f).clone());
            }
            return existing.i("id");
        }
        self.insert(table, row)
    }

    pub(crate) fn find(&self, table: &str, pred: impl Fn(&V) -> bool) -> Option<&V> {
        self.rows(table).iter().find(|r| pred(r))
    }

    pub(crate) fn find_mut(&mut self, table: &str, pred: impl Fn(&V) -> bool) -> Option<&mut V> {
        self.tables.get_mut(table)?.iter_mut().find(|r| pred(r))
    }

    pub(crate) fn by_id(&self, table: &str, id: i64) -> Option<&V> {
        self.find(table, |r| r.i("id") == id)
    }

    pub(crate) fn by_id_mut(&mut self, table: &str, id: i64) -> Option<&mut V> {
        self.find_mut(table, |r| r.i("id") == id)
    }

    pub(crate) fn filter(&self, table: &str, pred: impl Fn(&V) -> bool) -> Vec<V> {
        self.rows(table).iter().filter(|r| pred(r)).cloned().collect()
    }

    /// Removes the row `id`; it.
    pub(crate) fn remove(&mut self, table: &str, id: i64) -> Option<V> {
        let rows = self.tables.get_mut(table)?;
        let i = rows.iter().position(|r| r.i("id") == id)?;
        Some(rows.remove(i))
    }

    // --- cards and users (ARTEMiS' core) ---

    pub(crate) fn card_by_access_code(&self, code: &str) -> Option<V> {
        self.find("card", |c| c.s("access_code") == code).cloned()
    }

    pub(crate) fn card_by_idm(&self, idm: &str) -> Option<V> {
        (!idm.is_empty()).then(|| self.find("card", |c| c.s("idm") == idm).cloned()).flatten()
    }

    pub(crate) fn create_user(&mut self) -> i64 {
        self.insert("user", V::obj().with("created", now()))
    }

    pub(crate) fn create_card(&mut self, user: i64, code: &str) -> i64 {
        self.insert("card", V::obj().with("user", user).with("access_code", code).with("idm", V::Null).with("chip_id", V::Null))
    }

    // --- profile.py ---

    pub(crate) fn profile(&self, user: i64) -> Option<V> {
        self.find("profile", |p| p.i("user") == user).cloned()
    }

    pub(crate) fn profile_mut(&mut self, user: i64) -> Option<&mut V> {
        self.find_mut("profile", |p| p.i("user") == user)
    }

    /// `create_profile`: the table's defaults.
    pub(crate) fn create_profile(&mut self, user: i64) -> i64 {
        let row = V::obj()
            .with("user", user)
            .with("user_type", 1)
            .with("nick_name", "PLAYER")
            .with("rank_num", 1)
            .with("rank_exp", 0)
            .with("own_col", 0)
            .with("own_vp", 0)
            .with("own_yui_medal", 0)
            .with("setting_title_id", 20005)
            .with("my_shop", V::Null)
            .with("fav_hero", V::Null)
            .with("when_register", now())
            .with("last_login_date", V::Null)
            .with("last_yui_medal_date", V::Null)
            .with("ad_confirm_date", V::Null)
            .with("login_ct", 0);
        self.upsert("profile", row, &["user"], &[])
    }

    /// Adds `n` to a profile's integer column (`add_col`, `add_vp`, `add_exp`, `add_yui_medals`).
    pub(crate) fn add_profile(&mut self, user: i64, column: &str, n: i64) {
        if let Some(p) = self.profile_mut(user) {
            let v = p.i(column) + n;
            p.set(column, v);
        }
    }

    pub(crate) fn set_profile(&mut self, user: i64, column: &str, value: impl Into<V>) {
        if let Some(p) = self.profile_mut(user) {
            p.set(column, value);
        }
    }

    // --- item.py ---

    /// `put_equipment`: a new equipment (level 1, 200 exp), unique by user and equipment.
    pub(crate) fn put_equipment(&mut self, user: i64, equipment_id: i64) -> i64 {
        let mut row = V::obj()
            .with("user", user)
            .with("equipment_id", equipment_id)
            .with("enhancement_value", 1)
            .with("enhancement_exp", 200)
            .with("awakening_exp", 0)
            .with("awakening_stage", 0)
            .with("possible_awakening_flag", 0)
            .with("is_shop_purchase", 0)
            .with("is_protect", 0);
        for p in 1..=4 {
            row.set(&format!("property{p}_property_id"), 0);
            row.set(&format!("property{p}_value1"), 0);
            row.set(&format!("property{p}_value2"), 0);
        }
        let row = row.with("converted_card_num", 0).with("get_date", now());
        self.upsert("equipment", row, &["user", "equipment_id"], &["equipment_id"])
    }

    /// `put_item`, unique by user and item.
    pub(crate) fn put_item(&mut self, user: i64, item_id: i64) -> i64 {
        let row = V::obj().with("user", user).with("item_id", item_id).with("get_date", now());
        self.upsert("item", row, &["user", "item_id"], &["item_id"])
    }

    /// `put_hero_log`, unique by user and hero.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_hero_log(&mut self, user: i64, hero_log_id: i64, level: i64, exp: i64, main_weapon: V, sub_equipment: V, skills: [V; 5]) -> i64 {
        let mut row = V::obj()
            .with("user", user)
            .with("hero_log_id", hero_log_id)
            .with("log_level", level)
            .with("log_exp", exp)
            .with("main_weapon", main_weapon)
            .with("sub_equipment", sub_equipment);
        for (i, s) in skills.into_iter().enumerate() {
            row.set(&format!("skill_slot{}_skill_id", i + 1), s);
        }
        let mut row = row
            .with("max_level_extend_num", 0)
            .with("is_awakenable", 0)
            .with("awakening_stage", 0)
            .with("awakening_exp", 0)
            .with("is_shop_purchase", 0)
            .with("is_protect", 0);
        for p in 1..=4 {
            row.set(&format!("property{p}_property_id"), 0);
            row.set(&format!("property{p}_value1"), 0);
            row.set(&format!("property{p}_value2"), 0);
        }
        let row = row.with("converted_card_num", 0).with("get_date", now());
        let update = ["log_level", "log_exp", "main_weapon", "sub_equipment", "skill_slot1_skill_id", "skill_slot2_skill_id", "skill_slot3_skill_id", "skill_slot4_skill_id", "skill_slot5_skill_id"];
        self.upsert("hero_log", row, &["user", "hero_log_id"], &update)
    }

    /// `put_hero_party`, unique by user and team.
    pub(crate) fn put_hero_party(&mut self, user: i64, team: i64, heroes: [i64; 3]) -> i64 {
        let row = V::obj()
            .with("user", user)
            .with("user_party_team_id", team)
            .with("user_hero_log_id_1", heroes[0])
            .with("user_hero_log_id_2", heroes[1])
            .with("user_hero_log_id_3", heroes[2]);
        self.upsert("hero_party", row, &["user", "user_party_team_id"], &["user_hero_log_id_1", "user_hero_log_id_2", "user_hero_log_id_3"])
    }

    /// `put_player_quest`, unique by user and quest scene.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_player_quest(&mut self, user: i64, quest_type: i64, scene: i64, clear: bool, clear_time: i64, combo: i64, damage: i64, concurrent: i64) -> i64 {
        let row = V::obj()
            .with("user", user)
            .with("quest_type", quest_type)
            .with("quest_scene_id", scene)
            .with("quest_clear_flag", clear)
            .with("clear_time", clear_time)
            .with("combo_num", combo)
            .with("total_damage", damage)
            .with("concurrent_destroying_num", concurrent)
            .with("play_date", now());
        self.upsert("quest", row, &["user", "quest_scene_id"], &["quest_clear_flag", "clear_time", "combo_num", "total_damage", "concurrent_destroying_num"])
    }

    /// `put_ex_bonus`, unique by user, quest scene and table.
    pub(crate) fn put_ex_bonus(&mut self, user: i64, scene: i64, table: i64, clear: bool) -> i64 {
        let row = V::obj().with("user", user).with("quest_scene_id", scene).with("ex_bonus_table_id", table).with("quest_clear_flag", clear);
        self.upsert("ex_bonus", row, &["user", "quest_scene_id", "ex_bonus_table_id"], &["quest_clear_flag"])
    }

    /// Adds `n` to a row's integer column (`add_hero_xp`, `add_equipment_enhancement_exp`).
    pub(crate) fn add(&mut self, table: &str, id: i64, column: &str, n: i64) {
        if let Some(r) = self.by_id_mut(table, id) {
            let v = r.i(column) + n;
            r.set(column, v);
        }
    }

    pub(crate) fn set(&mut self, table: &str, id: i64, column: &str, value: impl Into<V>) {
        if let Some(r) = self.by_id_mut(table, id) {
            r.set(column, value);
        }
    }

    /// The newest row of a user (`get_session`, `get_end_session`: ordered by play date).
    pub(crate) fn latest(&self, table: &str, user: i64) -> Option<V> {
        self.rows(table).iter().filter(|r| r.i("user") == user).max_by_key(|r| r.i("id")).cloned()
    }
}
