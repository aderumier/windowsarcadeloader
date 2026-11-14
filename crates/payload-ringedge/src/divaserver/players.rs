//! The title server's players: one per Aime card (its Aime id, the player data id `pd_id`),
//! registered by the game (`registration`, the player's name), their profile and best scores
//! kept in the save folder (`diva_players.txt`: a line per record, its key, a tab, its values as
//! a form). After ARTEMiS' `titles/diva` (WTFPL), with this version's keys (`mdl_equip_ary`,
//! `next_pv_id`, `skin_equip`, `act_vol`...) and their bounds: the game checks every value of an
//! answer against its table of keys (`0xbd4140`: each key's count of values, type and range) and
//! refuses the whole answer for one out of them (a communication error), and a key it reads that
//! the answer lacks throws `requested key is bad key`.
//!
//! The settings the game sends with a player's commands (volumes, sort, the next song, the
//! equipped modules...) are kept as they come and given back at the next `start`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use wal_payload_common::log;

use super::{quote, unquote, LUT};

/// The commands answered here.
pub(super) const COMMANDS: &[&str] = &[
    "pre_start",
    "registration",
    "start",
    "get_pv_pd",
    "stage_result",
    "end",
    "pd_unlock",
    "spend_credit",
    "card_procedure",
    "change_name",
    "change_passwd",
    "init_passwd",
    "buy_module",
    "shop_exit",
    "no_card_end",
    "deport",
    "investigate",
    "store_ss",
];

/// A My List: 40 songs, none.
const MY_LIST: &str = "-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1";

/// A new player's profile.
const DEFAULTS: &[(&str, &str)] = &[
    ("lv_num", "1"),
    ("lv_pnt", "0"),
    ("lv_str", "WindowsArcadeLoader"),
    ("lv_efct_id", "0"),
    ("lv_plt_id", "1"),
    ("vcld_pts", "0"),
    ("hp_vol", "100"),
    ("act_vol", "1"),
    ("act_vol2", "100"),
    ("passwd_stat", "0"),
    ("passwd", "**********"),
    ("next_pv_id", "-1"),
    ("next_difficulty", "1"),
    ("touch_reaction", "1"),
    ("sort_kind", "2"),
    ("skin_equip", "0"),
    ("my_lst_0", MY_LIST),
    ("my_lst_1", MY_LIST),
    ("my_lst_2", MY_LIST),
    ("my_lst_3", MY_LIST),
    ("my_lst_4", MY_LIST),
    ("dsp_clr_brdr", "1"),
    ("dsp_intrm_rnk", "1"),
    ("use_pv_module_equip", "0"),
    ("use_pv_skin_equip", "0"),
    ("use_pv_btn_se_equip", "0"),
    ("mdl_equip_ary", "-999,-999"),
    ("mdl_equip", "-999"),
];

/// The settings kept from the player's commands.
const SETTINGS: &[&str] = &[
    "hp_vol",
    "act_vol",
    "act_vol2",
    "next_pv_id",
    "next_difficulty",
    "touch_reaction",
    "sort_kind",
    "skin_equip",
    "my_lst_0",
    "my_lst_1",
    "my_lst_2",
    "my_lst_3",
    "my_lst_4",
    "dsp_clr_brdr",
    "dsp_intrm_rnk",
    "use_pv_module_equip",
    "use_pv_skin_equip",
    "use_pv_btn_se_equip",
    "mdl_equip_ary",
    "mdl_equip",
];

/// The modules owned: all of them (as ARTEMiS' `unlock_all_modules`).
const MDL_HAVE: usize = 250;

/// The level's points (ARTEMiS: a level per 13979 attained points of the best scores).
const LEVEL_POINTS: i64 = 13979;

type Record = BTreeMap<String, String>;

struct Store {
    records: BTreeMap<String, Record>,
    loaded: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store { records: BTreeMap::new(), loaded: false });

fn path() -> std::path::PathBuf {
    let dir = std::env::var("WAL_RINGEDGE_SAVE").unwrap_or_else(|_| "sv".into());
    std::path::Path::new(&dir).join("diva_players.txt")
}

fn parse_form(text: &str) -> Record {
    text.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), unquote(v))).collect()
}

fn to_form(record: &Record) -> String {
    record.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect::<Vec<_>>().join("&")
}

impl Store {
    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Ok(text) = std::fs::read_to_string(path()) {
            for line in text.lines() {
                if let Some((key, form)) = line.split_once('\t') {
                    self.records.insert(key.to_string(), parse_form(form));
                }
            }
        }
    }

    fn save(&self) {
        let text: String = self.records.iter().map(|(k, r)| format!("{k}\t{}\n", to_form(r))).collect();
        if let Err(e) = std::fs::write(path(), text) {
            log!("diva: cannot write {}: {e}", path().display());
        }
    }
}

fn profile_key(pd_id: &str) -> String {
    format!("player {pd_id}")
}

/// A record's integer value.
fn int(record: &Record, key: &str) -> i64 {
    record.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

/// `key=value` pairs, `&` before each, each key once (its first value), the values quoted but
/// their commas kept (the game splits lists on raw commas, not decoded).
fn pairs(list: &[(&str, String)]) -> String {
    let mut seen = std::collections::BTreeSet::new();
    list.iter().filter(|(k, _)| seen.insert(*k)).map(|(k, v)| format!("&{k}={}", quote(v).replace("%2C", ","))).collect()
}

/// `n` copies of `value`, comma separated (the game checks each list key's count of values).
fn rep(n: usize, value: &str) -> String {
    vec![value; n].join(",")
}

/// The profile's value of `key`, else its default.
fn get(profile: &Record, key: &str) -> String {
    profile.get(key).cloned().unwrap_or_else(|| DEFAULTS.iter().find(|(d, _)| *d == key).map_or(String::new(), |(_, v)| v.to_string()))
}

/// The player's data the game reads from `pre_start`, `start`, `card_procedure` and the
/// player's administration commands. Every value within its key's bounds in the game's table
/// (`0xbd4140`: each key's count of values, type and range; an answer holding a value out of
/// them is refused, a communication error): `mdl_equip_ary` 2 modules, `skin_equip` 0 to 999,
/// `act_vol` -1 to 1, the lottery state 0 to 7...
fn player_keys(pd_id: &str, profile: &Record) -> Vec<(&'static str, String)> {
    let mut list = vec![
        ("pd_id", pd_id.to_string()),
        ("accept_idx", "100".to_string()),
        ("start_idx", "100".into()),
        ("exec_vu", "0".into()),
        ("mdl_equip_tm", LUT.into()),
        ("mdl_have", "F".repeat(MDL_HAVE)),
        ("nblss_ltt_stts", "0".into()),
        ("nblss_ltt_tckt", "-1".into()),
        ("nblss_ltt_is_opn", "0".into()),
    ];
    for k in ["player_name", "hp_vol", "act_vol", "act_vol2", "lv_num", "lv_pnt", "lv_str", "lv_efct_id", "lv_plt_id", "vcld_pts", "passwd", "passwd_stat"].iter().chain(SETTINGS) {
        list.push((k, get(profile, k)));
    }
    list
}

/// The level from the player's best scores' attained points.
fn level(store: &Store, pd_id: &str) -> (i64, i64) {
    let prefix = format!("score {pd_id} ");
    let total: i64 = store.records.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, r)| int(r, "stg_atn_pnt")).sum();
    (total / LEVEL_POINTS + 1, (total % LEVEL_POINTS) * 100 / LEVEL_POINTS)
}

/// The values of a player command, `None` for the others.
pub(super) fn values(cmd: &str, form: &[(String, String)]) -> Option<String> {
    if !COMMANDS.contains(&cmd) {
        return None;
    }
    let value = |key: &str| form.iter().find(|(k, _)| k == key).map_or(String::new(), |(_, v)| v.clone());
    let mut store = STORE.lock().unwrap();
    store.load();
    let pd_id = {
        let pd = value("pd_id");
        if pd.is_empty() || pd == "-1" { value("aime_id") } else { pd }
    };
    let key = profile_key(&pd_id);
    // the settings sent with the command
    if let Some(profile) = store.records.get_mut(&key) {
        let mut changed = false;
        for s in SETTINGS {
            let v = value(s);
            if !v.is_empty() && profile.get(*s) != Some(&v) {
                profile.insert(s.to_string(), v);
                changed = true;
            }
        }
        if changed {
            store.save();
        }
    }
    let profile = store.records.get(&key).cloned();
    let none_bought = [("mdl_buy_hist_id", rep(10, "-1")), ("mdl_buy_hist_tm", rep(10, LUT))];
    let answer = match cmd {
        // the card's player: none (-3, a new card: registration offered) or registered (1)
        "pre_start" => match &profile {
            None => "&ps_result=-3".to_string(),
            Some(p) => {
                let mut list = vec![("ps_result", "1".to_string())];
                list.extend(player_keys(&pd_id, p));
                pairs(&list)
            }
        },
        "registration" => {
            let name = value("player_name");
            let mut record: Record = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            record.insert("player_name".into(), name.clone());
            store.records.insert(key.clone(), record);
            store.save();
            log!("diva: player {pd_id} registered: {name}");
            let mut list = vec![("cd_adm_result", "1".to_string()), ("cd_adm_time", LUT.into()), ("chg_name_price", "100".into())];
            list.extend(player_keys(&pd_id, &store.records[&key]));
            pairs(&list)
        }
        "start" => match &profile {
            None => "&start_result=-1".to_string(),
            Some(p) => {
                let mut list = vec![("start_result", "1".to_string())];
                list.extend(player_keys(&pd_id, p));
                // no module bought, no contest, no songs' records here (get_pv_pd)
                list.extend(none_bought);
                list.extend([
                    ("cv_cid", rep(4, "-1")),
                    ("cv_sc", rep(4, "-1")),
                    ("cv_rr", rep(4, "-1")),
                    ("cv_bv", rep(4, "-1")),
                    ("cv_bf", rep(4, "-1")),
                    ("cnp_cid", "-1".into()),
                    ("cnp_val", "-1".into()),
                    ("cnp_rr", "-1".into()),
                    ("cnp_sp", rep(50, "-1")),
                    ("pd_by_pv_id", rep(30, "xxx")),
                ]);
                pairs(&list)
            }
        },
        // the songs' best records at the difficulty asked: 16 values each, comma separated (quoted
        // in the list: the game splits the list, then each record), an entry with fewer values
        // read as no record (FUN_006e02d0): the song, its clear kind, score, attained points, a
        // challenge, the 2 modules, the skin, the button sound, then no rival (its player, score
        // and points) and no rankings
        "get_pv_pd" => {
            let difficulty = value("difficulty");
            let mut songs: Vec<String> = value("pd_pv_id_lst")
                .split(',')
                .map(|pv| {
                    let pv = pv.trim();
                    match store.records.get(&format!("score {pd_id} {pv} {difficulty}")) {
                        Some(r) => quote(&format!(
                            "{pv},{},{},{},-1,-999,-999,0,-1,-1,-1,0,0,-1,-1,-1",
                            get(r, "stg_clr_kind"),
                            get(r, "stg_score"),
                            get(r, "stg_atn_pnt")
                        )),
                        None => quote(&format!("{pv}***")),
                    }
                })
                .collect();
            songs.resize(30, "xxx".into());
            songs.truncate(30);
            format!("&pd_by_pv_id={}{}", songs.join(","), pairs(&[("pdddt_flg", "0".into()), ("pdddt_tm", LUT.into())]))
        }
        "stage_result" => {
            let Some(p) = profile else { return Some(String::new()) };
            let (old_num, old_pnt) = (int(&p, "lv_num"), int(&p, "lv_pnt"));
            // the best score of each song played
            const KEPT: [&str; 6] = ["stg_ply_pv_id", "stg_difficulty", "stg_score", "stg_atn_pnt", "stg_clr_kind", "stg_max_cmb"];
            let lists: Vec<Vec<String>> = KEPT.iter().map(|k| value(k).split(',').map(|v| v.trim().to_string()).collect()).collect();
            for (i, pv) in lists[0].iter().enumerate() {
                if pv.is_empty() || pv.starts_with('-') {
                    continue;
                }
                let difficulty = lists[1].get(i).cloned().unwrap_or_default();
                let score_key = format!("score {pd_id} {pv} {difficulty}");
                let score: i64 = lists[2].get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
                if score > store.records.get(&score_key).map_or(-1, |r| int(r, "stg_score")) {
                    let r: Record = KEPT.iter().zip(&lists).map(|(k, list)| (k.to_string(), list.get(i).cloned().unwrap_or_default())).collect();
                    store.records.insert(score_key, r);
                }
            }
            let (num, pnt) = level(&store, &pd_id);
            let vcld_pts = Some(value("vcld_pts")).filter(|v| !v.is_empty()).unwrap_or_else(|| get(&p, "vcld_pts"));
            if let Some(p) = store.records.get_mut(&key) {
                p.insert("lv_num".into(), num.to_string());
                p.insert("lv_pnt".into(), pnt.to_string());
                p.insert("vcld_pts".into(), vcld_pts.clone());
            }
            store.save();
            let list = vec![
                ("chllng_kind", "-1".to_string()),
                ("lv_num_old", old_num.to_string()),
                ("lv_pnt_old", old_pnt.to_string()),
                ("lv_num", num.to_string()),
                ("lv_str", get(&p, "lv_str")),
                ("lv_pnt", pnt.to_string()),
                ("lv_efct_id", get(&p, "lv_efct_id")),
                ("lv_plt_id", get(&p, "lv_plt_id")),
                ("vcld_pts", vcld_pts),
                ("prsnt_vcld_pts", get(&p, "vcld_pts")),
                ("touch_reaction", get(&p, "touch_reaction")),
                // no reward, title or skin won, no contest
                ("cerwd_kind", "-1".into()),
                ("cerwd_value", "-1".into()),
                ("cerwd_str_0", "xxx".into()),
                ("cerwd_str_1", "xxx".into()),
                ("crwd_kind", rep(3, "-1")),
                ("crwd_value", rep(3, "-1")),
                ("crwd_str_0", rep(3, "xxx")),
                ("crwd_str_1", rep(3, "xxx")),
                ("ttl_str_ary", rep(5, "xxx")),
                ("ttl_plt_id_ary", rep(5, "-1")),
                ("ttl_desc_ary", rep(5, "xxx")),
                ("skin_id_ary", rep(5, "-1")),
                ("skin_name_ary", rep(5, "xxx")),
                ("skin_illust_ary", rep(5, "xxx")),
                ("skin_desc_ary", rep(5, "xxx")),
                ("cnp_cid", "-1".into()),
                ("cnp_val", "-1".into()),
                ("cnp_sp", rep(50, "-1")),
                ("pdddt_flg", "0".into()),
                ("pdddt_tm", LUT.into()),
                ("nblss_ltt_is_opn", "0".into()),
                ("nblss_ltt_stts", "0".into()),
                ("nblss_ltt_nxt_stts", "0".into()),
                ("nblss_ltt_nxt_tckt", "-1".into()),
                ("nblss_ltt_rslt", "-1".into()),
                ("nblss_ltt_prz", "0".into()),
            ];
            pairs(&list)
        }
        "spend_credit" => match &profile {
            None => String::new(),
            Some(p) => pairs(&[
                // no campaign: 5 results of 9 values (ARTEMiS' empty result)
                ("cmpgn_rslt", rep(5, "-1,-1,x,-1,-1,x,x,-1,x")),
                ("cmpgn_rslt_num", "0".into()),
                ("vcld_pts", get(p, "vcld_pts")),
                ("lv_str", get(p, "lv_str")),
                ("lv_efct_id", get(p, "lv_efct_id")),
                ("lv_plt_id", get(p, "lv_plt_id")),
            ]),
        },
        "card_procedure" => match &profile {
            None => "&cd_adm_result=0".to_string(),
            Some(p) => {
                let mut list = vec![("cd_adm_result", "1".to_string()), ("cd_adm_time", LUT.into()), ("chg_name_price", "100".into())];
                list.extend(player_keys(&pd_id, p));
                pairs(&list)
            }
        },
        "change_name" | "change_passwd" | "init_passwd" => {
            if profile.is_none() {
                "&cd_adm_result=0".to_string()
            } else {
                if let Some(p) = store.records.get_mut(&key) {
                    match cmd {
                        "change_name" => {
                            p.insert("player_name".into(), value("player_name"));
                        }
                        "change_passwd" => {
                            p.insert("passwd".into(), value("new_passwd"));
                            p.insert("passwd_stat".into(), "1".into());
                        }
                        _ => {
                            p.insert("passwd".into(), "**********".into());
                            p.insert("passwd_stat".into(), "0".into());
                        }
                    }
                }
                store.save();
                let mut list = vec![("cd_adm_result", "1".to_string()), ("cd_adm_time", LUT.into()), ("chg_name_price", "100".into())];
                list.extend(player_keys(&pd_id, &store.records[&key]));
                pairs(&list)
            }
        }
        "buy_module" => {
            let vp = profile.as_ref().map_or("0".to_string(), |p| get(p, "vcld_pts"));
            let mut list = vec![("shp_rslt", "1".to_string()), ("mdl_id", value("mdl_id")), ("mdl_have", "F".repeat(MDL_HAVE)), ("vcld_pts", vp)];
            list.extend(none_bought);
            pairs(&list)
        }
        "shop_exit" => "&shp_rslt=1".to_string(),
        // nothing to answer: end, pd_unlock, no_card_end, deport, investigate, store_ss
        _ => String::new(),
    };
    Some(answer)
}
