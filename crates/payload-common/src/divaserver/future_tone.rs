//! Project DIVA Arcade Future Tone's players (Nu), ARTEMiS' `titles/diva/base.py` handlers ported
//! whole (its configuration's defaults: every module and customize item owned): `pre_start`,
//! `registration`, `start`, `pd_unlock`, `spend_credit`, `get_pv_pd`, `stage_start`,
//! `stage_result`, `end`, `shop_exit`, `buy_module`, `buy_cstmz_itm`, `card_procedure`,
//! `change_name`, `change_passwd`. Its tables kept in the save folder (`diva_ft.txt`, one record
//! a line: its key, a tab, its form): `profile <aime>` (ARTEMiS' `diva_profile` columns and
//! defaults), `score <aime> <pv> <difficulty> <edition>` (the best scores), `shop <aime>` (the
//! common modules and items equipped), `pvc <aime> <pv>` (a song's own), `module <aime>` /
//! `item <aime>` (the ones bought). ARTEMiS' play log (never read back) is not kept.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::log;

use super::quote;

/// The commands answered here.
pub(super) const COMMANDS: &[&str] = &[
    "pre_start",
    "registration",
    "start",
    "pd_unlock",
    "spend_credit",
    "get_pv_pd",
    "stage_start",
    "stage_result",
    "end",
    "shop_exit",
    "buy_module",
    "buy_cstmz_itm",
    "card_procedure",
    "change_name",
    "change_passwd",
];

/// ARTEMiS' `diva_profile` columns' defaults.
const PROFILE: &[(&str, &str)] = &[
    ("lv_str", "Dab on 'em"),
    ("lv_num", "0"),
    ("lv_pnt", "0"),
    ("vcld_pts", "0"),
    ("hp_vol", "100"),
    ("btn_se_vol", "100"),
    ("btn_se_vol2", "100"),
    ("sldr_se_vol2", "100"),
    ("sort_kind", "2"),
    ("use_pv_mdl_eqp", "1"),
    ("use_mdl_pri", "0"),
    ("use_pv_skn_eqp", "0"),
    ("use_pv_btn_se_eqp", "1"),
    ("use_pv_sld_se_eqp", "0"),
    ("use_pv_chn_sld_se_eqp", "0"),
    ("use_pv_sldr_tch_se_eqp", "0"),
    ("btn_se_eqp", "-1"),
    ("sld_se_eqp", "-1"),
    ("chn_sld_se_eqp", "-1"),
    ("sldr_tch_se_eqp", "-1"),
    ("nxt_pv_id", "708"),
    ("nxt_dffclty", "2"),
    ("nxt_edtn", "0"),
    ("cnp_cid", "-1"),
    ("cnp_val", "-1"),
    ("cnp_rr", "-1"),
    ("cnp_sp", ""),
    ("dsp_clr_brdr", "7"),
    ("dsp_intrm_rnk", "1"),
    ("dsp_clr_sts", "1"),
    ("rgo_sts", "1"),
    ("lv_efct_id", "0"),
    ("lv_plt_id", "1"),
    ("skn_eqp", "0"),
    ("passwd_stat", "0"),
    ("passwd", "**********"),
];

const QST_25: &str = "-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1";
const MDL_EQP: &str = "-999,-999,-999";
const C_ITM_EQP: &str = "-999,-999,-999,-999,-999,-999,-999,-999,-999,-999,-999,-999";

/// ARTEMiS' level: one per 13979 attained points of the best scores.
const LEVEL_POINTS: i64 = 13979;

type Record = BTreeMap<String, String>;

struct Store {
    records: BTreeMap<String, Record>,
    loaded: bool,
}

static STORE: Mutex<Store> = Mutex::new(Store { records: BTreeMap::new(), loaded: false });

fn path() -> std::path::PathBuf {
    let dir = std::env::var("WAL_SEGANU_SAVE").or_else(|_| std::env::var("WAL_RINGEDGE_SAVE")).unwrap_or_else(|_| "sv".into());
    std::path::Path::new(&dir).join("diva_ft.txt")
}

impl Store {
    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Ok(text) = std::fs::read_to_string(path()) else { return };
        for line in text.lines() {
            if let Some((key, form)) = line.split_once('\t') {
                let record = form.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), super::unquote(v))).collect();
                self.records.insert(key.to_string(), record);
            }
        }
    }

    fn save(&self) {
        let text: String = self
            .records
            .iter()
            .map(|(k, r)| format!("{k}\t{}\n", r.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect::<Vec<_>>().join("&")))
            .collect();
        if let Err(e) = std::fs::write(path(), text) {
            log!("diva: cannot write {}: {e}", path().display());
        }
    }

    fn profile(&self, aime: &str) -> Option<&Record> {
        self.records.get(&format!("profile {aime}"))
    }

    /// The player's best scores: (pv, difficulty, edition, record).
    fn scores(&self, aime: &str) -> Vec<(i64, i64, i64, &Record)> {
        let prefix = format!("score {aime} ");
        self.records
            .iter()
            .filter_map(|(k, r)| {
                let rest = k.strip_prefix(&prefix)?;
                let mut n = rest.split(' ').map(|v| v.parse::<i64>().ok());
                Some((n.next()??, n.next()??, n.next()??, r))
            })
            .collect()
    }

    /// The number of best scores (every player's) at least the player's, on that song's
    /// difficulty and edition (ARTEMiS' `get_global_ranking`).
    fn ranking(&self, aime: &str, pv: i64, difficulty: i64, edition: i64) -> i64 {
        let Some(mine) = self.records.get(&format!("score {aime} {pv} {difficulty} {edition}")).map(|r| int(r, "score")) else { return 0 };
        let suffix = format!(" {pv} {difficulty} {edition}");
        self.records.iter().filter(|(k, r)| k.starts_with("score ") && k.ends_with(&suffix) && int(r, "score") >= mine).count() as i64
    }

    /// A set of ids owned (`module` / `item`) as ARTEMiS' 250 digit hex string.
    fn have(&self, kind: &str, aime: &str) -> String {
        let ids = self.records.get(&format!("{kind} {aime}")).and_then(|r| r.get("ids")).cloned().unwrap_or_default();
        let mut bits = [0u8; 125];
        for id in ids.split(',').filter_map(|v| v.parse::<usize>().ok()).filter(|&i| i < 1000) {
            // the number's bit `id`: its digit (250 hex digits, the lowest last)
            let digit = id / 4;
            if digit < 250 {
                let byte = 124 - digit / 2;
                bits[byte] |= (1 << (id % 4)) << if digit % 2 == 1 { 4 } else { 0 };
            }
        }
        bits.iter().map(|b| format!("{b:02X}")).collect()
    }

    fn add_owned(&mut self, kind: &str, aime: &str, id: &str) {
        let record = self.records.entry(format!("{kind} {aime}")).or_default();
        let mut ids: Vec<String> = record.get("ids").map(|v| v.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
        if !ids.iter().any(|i| i == id) {
            ids.push(id.to_string());
        }
        record.insert("ids".into(), ids.join(","));
    }
}

fn int(record: &Record, key: &str) -> i64 {
    record.get(key).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

/// A profile's value (its column's default when unset).
fn get(profile: &Record, key: &str) -> String {
    profile.get(key).cloned().unwrap_or_else(|| PROFILE.iter().find(|(k, _)| *k == key).map_or(String::new(), |(_, v)| v.to_string()))
}

/// The lists' update time (ARTEMiS' `time_lut`: now, as `%Y-%m-%d %H:%M:%S:16.0`, quoted).
fn time_lut() -> String {
    let secs = crate::clock::unix_now() as i64 + 9 * 3600;
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
    quote(&format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}:16.0", rem / 3600, rem % 3600 / 60, rem % 60))
}

/// The values of a Future Tone player command, `None` for the others.
pub(super) fn values(cmd: &str, form: &[(String, String)]) -> Option<String> {
    if !COMMANDS.contains(&cmd) {
        return None;
    }
    let data = |key: &str| form.iter().find(|(k, _)| k == key).map_or(String::new(), |(_, v)| v.clone());
    let mut store = STORE.lock().unwrap();
    store.load();
    let answer = match cmd {
        "pre_start" => pre_start(&store, &data("aime_id")),
        "registration" => {
            let aime = data("aime_id");
            store.records.entry(format!("profile {aime}")).or_default().insert("player_name".into(), data("player_name"));
            store.save();
            format!("&cd_adm_result=1&pd_id={aime}")
        }
        "start" => start(&store, &data("pd_id")),
        "spend_credit" => match store.profile(&data("pd_id")) {
            Some(p) => format!(
                "&cmpgn_rslt=-1,-1,x,-1,-1,x,x,-1,x,-1,-1,x,-1,-1,x,x,-1,x,-1,-1,x,-1,-1,x,x,-1,x,-1,-1,x,-1,-1,x,x,-1,x,-1,-1,x,-1,-1,x,x,-1,x,-1,-1,x,-1,-1,x,x,-1,x&cmpgn_rslt_num=0&vcld_pts={}&lv_str={}&lv_efct_id={}&lv_plt_id={}",
                get(p, "vcld_pts"),
                get(p, "lv_str"),
                get(p, "lv_efct_id"),
                get(p, "lv_plt_id")
            ),
            None => String::new(),
        },
        "get_pv_pd" => get_pv_pd(&store, &data("pd_id"), &data("pd_pv_id_lst"), &data("difficulty")),
        "stage_result" => {
            let answer = stage_result(&mut store, &data);
            store.save();
            answer
        }
        "end" => {
            let aime = data("pd_id");
            if let Some(p) = store.records.get_mut(&format!("profile {aime}")) {
                p.insert("my_qst_id".into(), data("my_qst_id"));
                p.insert("my_qst_sts".into(), data("my_qst_sts"));
                store.save();
            }
            String::new()
        }
        "shop_exit" => {
            let aime = data("pd_id");
            let shop = store.records.entry(format!("shop {aime}")).or_default();
            shop.insert("mdl_eqp_ary".into(), data("mdl_eqp_cmn_ary"));
            shop.insert("c_itm_eqp_ary".into(), data("c_itm_eqp_cmn_ary"));
            shop.insert("ms_itm_flg_ary".into(), data("ms_itm_flg_cmn_ary"));
            let pvc = store.records.entry(format!("pvc {aime} {}", data("ply_pv_id"))).or_default();
            if data("use_pv_mdl_eqp").trim() == "1" {
                pvc.insert("mdl_eqp_ary".into(), data("mdl_eqp_pv_ary"));
                pvc.insert("c_itm_eqp_ary".into(), data("c_itm_eqp_pv_ary"));
                pvc.insert("ms_itm_flg_ary".into(), data("ms_itm_flg_pv_ary"));
            } else {
                pvc.insert("mdl_eqp_ary".into(), "-1,-1,-1".into());
                pvc.insert("c_itm_eqp_ary".into(), "-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1".into());
                pvc.insert("ms_itm_flg_ary".into(), "1,1,1,1,1,1,1,1,1,1,1,1".into());
            }
            store.save();
            "&shp_rslt=1".into()
        }
        "buy_module" => buy(&mut store, &data, "module", "mdl_id", "mdl_price", "mdl_have", include_str!("../diva_data/ShopCatalog.dat")),
        "buy_cstmz_itm" => buy(&mut store, &data, "item", "cstmz_itm_id", "cstmz_itm_price", "cstmz_itm_have", include_str!("../diva_data/ItemCatalog.dat")),
        "card_procedure" => match store.profile(&data("aime_id")) {
            None => "&cd_adm_result=0".into(),
            Some(p) => format!(
                "&cd_adm_result=1&chg_name_price=100&accept_idx=100&pd_id={}&player_name={}&lv_num={}&lv_pnt={}&lv_str={}&lv_efct_id={}&lv_plt_id={}&vcld_pts={}&passwd_stat={}",
                data("aime_id"),
                get(p, "player_name"),
                get(p, "lv_num"),
                get(p, "lv_pnt"),
                get(p, "lv_str"),
                get(p, "lv_efct_id"),
                get(p, "lv_plt_id"),
                get(p, "vcld_pts"),
                get(p, "passwd_stat")
            ),
        },
        "change_name" => {
            let aime = data("pd_id");
            let price: i64 = data("chg_name_price").trim().parse().unwrap_or(0);
            match store.records.get_mut(&format!("profile {aime}")) {
                Some(p) if int(p, "vcld_pts") >= price => {
                    let points = int(p, "vcld_pts") - price;
                    p.insert("vcld_pts".into(), points.to_string());
                    p.insert("player_name".into(), data("player_name"));
                    store.save();
                    format!("&cd_adm_result=1&accept_idx=100&pd_id={aime}&player_name={}", data("player_name"))
                }
                _ => "&cd_adm_result=0".into(),
            }
        }
        "change_passwd" => {
            let aime = data("pd_id");
            match store.records.get_mut(&format!("profile {aime}")) {
                Some(p) if get(p, "passwd") == data("passwd") => {
                    p.insert("passwd_stat".into(), "1".into());
                    p.insert("passwd".into(), data("new_passwd"));
                    store.save();
                    format!("&cd_adm_result=1&accept_idx=100&pd_id={aime}")
                }
                _ => "&cd_adm_result=0".into(),
            }
        }
        // pd_unlock, stage_start
        _ => String::new(),
    };
    Some(answer)
}

fn pre_start(store: &Store, aime: &str) -> String {
    let Some(p) = store.profile(aime) else { return "&ps_result=-3".into() };
    let mut out = format!("&ps_result=1&accept_idx=100&nblss_ltt_stts=-1&nblss_ltt_tckt=-1&nblss_ltt_is_opn=-1&pd_id={aime}");
    // ARTEMiS answers the name as the sort kind too
    for (key, column) in [
        ("player_name", "player_name"),
        ("sort_kind", "player_name"),
        ("lv_efct_id", "lv_efct_id"),
        ("lv_plt_id", "lv_plt_id"),
        ("lv_str", "lv_str"),
        ("lv_num", "lv_num"),
        ("lv_pnt", "lv_pnt"),
        ("vcld_pts", "vcld_pts"),
        ("skn_eqp", "skn_eqp"),
        ("btn_se_eqp", "btn_se_eqp"),
        ("sld_se_eqp", "sld_se_eqp"),
        ("chn_sld_se_eqp", "chn_sld_se_eqp"),
        ("sldr_tch_se_eqp", "sldr_tch_se_eqp"),
        ("passwd_stat", "passwd_stat"),
    ] {
        out.push_str(&format!("&{key}={}", get(p, column)));
    }
    let shop = store.records.get(&format!("shop {aime}"));
    out.push_str(&format!("&mdl_eqp_tm={}&mdl_eqp_ary={}", time_lut(), shop.and_then(|s| s.get("mdl_eqp_ary")).map_or(MDL_EQP, |v| v)));
    out
}

fn start(store: &Store, aime: &str) -> String {
    let Some(p) = store.profile(aime) else { return String::new() };
    let mut out = format!("&pd_id={aime}&start_result=1&accept_idx=100");
    for key in ["hp_vol", "btn_se_vol", "btn_se_vol2", "sldr_se_vol2", "sort_kind", "player_name", "lv_num", "lv_pnt", "lv_efct_id", "lv_plt_id"] {
        out.push_str(&format!("&{key}={}", get(p, key)));
    }
    // every module and item owned (ARTEMiS' unlock_all_modules / unlock_all_items)
    out.push_str(&format!("&mdl_have={}&cstmz_itm_have={}", "F".repeat(250), "F".repeat(250)));
    for key in ["use_pv_mdl_eqp", "use_mdl_pri", "use_pv_skn_eqp", "use_pv_btn_se_eqp", "use_pv_sld_se_eqp", "use_pv_chn_sld_se_eqp", "use_pv_sldr_tch_se_eqp"] {
        out.push_str(&format!("&{key}={}", get(p, key)));
    }
    // ARTEMiS answers the level effect as the points
    out.push_str(&format!("&vcld_pts={}", get(p, "lv_efct_id")));
    for key in ["nxt_pv_id", "nxt_dffclty", "nxt_edtn", "dsp_clr_brdr", "dsp_intrm_rnk", "dsp_clr_sts", "rgo_sts"] {
        out.push_str(&format!("&{key}={}", get(p, key)));
    }
    out.push_str("&cv_cid=-1,-1,-1,-1&cv_sc=-1,-1,-1,-1&cv_bv=-1,-1,-1,-1&cv_bv=-1,-1,-1,-1&cv_bf=-1,-1,-1,-1");
    out.push_str(&format!("&cnp_cid={}&cnp_val={}&cnp_rr={}", get(p, "cnp_cid"), get(p, "cnp_val"), get(p, "cnp_rr")));
    if !get(p, "cnp_sp").is_empty() {
        out.push_str(&format!("&cnp_sp={}", get(p, "cnp_sp")));
    }
    match (p.get("my_qst_id"), p.get("my_qst_sts")) {
        (Some(id), Some(sts)) => out.push_str(&format!("&my_qst_id={id}&my_qst_sts={sts}")),
        _ => out.push_str(&format!("&my_qst_id={QST_25}&my_qst_sts=0,0,0,0,0,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1")),
    }
    out.push_str("&my_qst_prgrs=0,0,0,0,0,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1");
    out.push_str("&my_qst_et=2022-06-19%2010%3A28%3A52.0,2022-06-19%2010%3A28%3A52.0,2022-06-19%2010%3A28%3A52.0,2100-01-01%2008%3A59%3A59.0,2100-01-01%2008%3A59%3A59.0,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx,xxx");
    // the clears per difficulty (easy, normal, hard, extreme, extra extreme: any extra edition):
    // clear (cheap, standard), great, excellent, perfect
    let mut clears = [[0i64; 4]; 5];
    for (_, difficulty, edition, r) in store.scores(aime) {
        let row = if edition == 0 { difficulty.clamp(0, 4) as usize } else { 4 };
        match int(r, "clr_kind") {
            1 | 2 => clears[row][0] += 1,
            3 => clears[row][1] += 1,
            4 => clears[row][2] += 1,
            5 => clears[row][3] += 1,
            _ => {}
        }
    }
    out.push_str(&format!("&clr_sts={}", clears.iter().flatten().map(i64::to_string).collect::<Vec<_>>().join(",")));
    let shop = store.records.get(&format!("shop {aime}"));
    let value = |key: &str, default: &'static str| shop.and_then(|s| s.get(key)).cloned().unwrap_or_else(|| default.to_string());
    out.push_str(&format!(
        "&mdl_eqp_tm={}&mdl_eqp_ary={}&c_itm_eqp_ary={}&ms_itm_flg_ary={}",
        time_lut(),
        value("mdl_eqp_ary", MDL_EQP),
        value("c_itm_eqp_ary", C_ITM_EQP),
        value("ms_itm_flg_ary", "1,1,1,1,1,1,1,1,1,1,1,1")
    ));
    out
}

/// One song's result (ARTEMiS' `_get_pv_pd_result`) for an edition.
fn pv_result(store: &Store, aime: &str, pv: i64, difficulty: i64, edition: i64) -> String {
    let song = store.records.get(&format!("score {aime} {pv} {difficulty} {edition}"));
    let ranking = if song.is_some() { store.ranking(aime, pv, difficulty, edition) } else { 0 };
    let ranking = if ranking != 0 { ranking } else { -1 };
    let mut out = format!("{pv},{edition},");
    match song {
        Some(s) => out.push_str(&format!("{},{},{},{},", int(s, "clr_kind"), int(s, "score"), int(s, "atn_pnt"), int(s, "sort_kind"))),
        None => out.push_str("-1,-1,-1,0,"),
    }
    let pvc = store.records.get(&format!("pvc {aime} {pv}"));
    let value = |key: &str, default: &str| pvc.and_then(|c| c.get(key)).cloned().unwrap_or_else(|| default.to_string());
    out.push_str(&format!(
        "{},{},{},{},",
        value("mdl_eqp_ary", MDL_EQP),
        value("c_itm_eqp_ary", C_ITM_EQP),
        value("ms_itm_flg_ary", "-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1"),
        // skin, button, slide, chain slide, slider touch sounds
        if pvc.is_some() {
            ["skin", "btn_se", "sld_se", "chsld_se", "sldtch_se"].map(|k| value(k, "-1")).join(",")
        } else {
            "-1,-1,-1,-1,-1".into()
        }
    ));
    out.push_str(&format!("-1,-1,-1,-1,-1,{ranking},1,1,1,0,0,0"));
    out
}

fn get_pv_pd(store: &Store, aime: &str, list: &str, difficulty: &str) -> String {
    let difficulty: i64 = difficulty.trim().parse().unwrap_or(0);
    let entries: Vec<String> = list
        .split(',')
        .map(|song| match song.trim().parse::<i64>() {
            Ok(pv) if pv > 0 => quote(&format!("{},{}", pv_result(store, aime, pv, difficulty, 0), pv_result(store, aime, pv, difficulty, 1))),
            _ => quote(&format!("{song}***")),
        })
        .collect();
    format!("&pd_by_pv_id={}&pdddt_flg=0&pdddt_tm={}", entries.join(","), time_lut())
}

fn stage_result(store: &mut Store, data: &dyn Fn(&str) -> String) -> String {
    let aime = data("pd_id");
    let Some(profile) = store.profile(&aime).cloned() else { return String::new() };
    let list = |key: &str| data(key).split(',').map(str::to_string).collect::<Vec<_>>();
    let (songs, difficulties, editions, scores, points, clears) = (list("stg_ply_pv_id"), list("stg_difficulty"), list("stg_edtn"), list("stg_score"), list("stg_atn_pnt"), list("stg_clr_kind"));
    let counts = ["stg_cool_cnt", "stg_fine_cnt", "stg_safe_cnt", "stg_sad_cnt", "stg_wt_wg_cnt", "stg_max_cmb"].map(list);
    let at = |v: &[String], i: usize| v.get(i).cloned().unwrap_or_default();
    for (i, song) in songs.iter().enumerate() {
        if song.contains("-1") {
            continue;
        }
        let key = format!("score {aime} {song} {} {}", at(&difficulties, i), at(&editions, i));
        let score: i64 = at(&scores, i).trim().parse().unwrap_or(0);
        // the best score kept (a tie replaces it)
        if store.records.get(&key).is_some_and(|best| score < int(best, "score")) {
            continue;
        }
        let mut record = Record::new();
        record.insert("score".into(), score.to_string());
        record.insert("atn_pnt".into(), at(&points, i));
        record.insert("clr_kind".into(), at(&clears, i));
        record.insert("sort_kind".into(), data("sort_kind"));
        for (name, values) in ["cool", "fine", "safe", "sad", "worst", "max_combo"].iter().zip(&counts) {
            record.insert(name.to_string(), at(values, i));
        }
        store.records.insert(key, record);
    }
    let total: i64 = store.scores(&aime).iter().map(|(_, _, _, r)| int(r, "atn_pnt")).sum();
    let level = total / LEVEL_POINTS + 1;
    let level_points = ((total % LEVEL_POINTS) as f64 / LEVEL_POINTS as f64 * 100.0).round() as i64;
    let mut out = format!("&chllng_kind=-1&lv_num_old={}&lv_pnt_old={}", int(&profile, "lv_num"), int(&profile, "lv_pnt"));
    let vcld: i64 = data("vcld_pts").trim().parse().unwrap_or(0);
    if let Some(p) = store.records.get_mut(&format!("profile {aime}")) {
        for (key, value) in [
            ("lv_num", level.to_string()),
            ("lv_pnt", level_points.to_string()),
            ("vcld_pts", vcld.to_string()),
            ("hp_vol", data("hp_vol")),
            ("btn_se_vol", data("btn_se_vol")),
            ("sldr_se_vol2", data("sldr_se_vol2")),
            ("sort_kind", data("sort_kind")),
            ("nxt_pv_id", data("ply_pv_id")),
            ("nxt_dffclty", data("nxt_dffclty")),
            ("nxt_edtn", data("nxt_edtn")),
            ("my_qst_id", data("my_qst_id")),
            ("my_qst_sts", data("my_qst_sts")),
        ] {
            p.insert(key.into(), value);
        }
    }
    out.push_str(&format!(
        "&lv_num={level}&lv_str={}&lv_pnt={level_points}&lv_efct_id={}&lv_plt_id={}&vcld_pts={vcld}&prsnt_vcld_pts={}",
        get(&profile, "lv_str"),
        int(&profile, "lv_efct_id"),
        int(&profile, "lv_plt_id"),
        int(&profile, "vcld_pts")
    ));
    out.push_str("&cerwd_kind=-1&cerwd_value=-1&cerwd_str_0=***&cerwd_str_1=***&ttl_str_ary=xxx,xxx,xxx,xxx,xxx&ttl_plt_id_ary=-1,-1,-1,-1,-1&ttl_desc_ary=xxx,xxx,xxx,xxx,xxx");
    out.push_str("&skin_id_ary=xxx,xxx,xxx,xxx,xxx&skin_name_ary=xxx,xxx,xxx,xxx,xxx&skin_illust_ary=xxx,xxx,xxx,xxx,xxx&skin_desc_ary=xxx,xxx,xxx,xxx,xxx");
    out.push_str(&format!("&my_qst_id={}", profile.get("my_qst_id").map_or(QST_25, |v| v)));
    out.push_str(&format!("&my_qst_r_qid={QST_25}&my_qst_r_knd={QST_25}&my_qst_r_vl={QST_25}&my_qst_r_nflg={QST_25}"));
    out.push_str("&my_ccd_r_qid=-1,-1,-1,-1,-1&my_ccd_r_hnd=-1,-1,-1,-1,-1&my_ccd_r_vp=-1,-1,-1,-1,-1");
    out
}

/// `buy_module` / `buy_cstmz_itm`: the catalog's id (ARTEMiS' imported shop tables: here the
/// catalogs the game was given), bought with the player's points.
fn buy(store: &mut Store, data: &dyn Fn(&str) -> String, kind: &str, id_key: &str, price_key: &str, have_key: &str, catalog: &str) -> String {
    let aime = data("pd_id");
    let id = data(id_key);
    let Some(points) = store.profile(&aime).map(|p| int(p, "vcld_pts")) else { return String::new() };
    let price: i64 = data(price_key).trim().parse().unwrap_or(0);
    let listed = catalog.lines().any(|l| l.split(',').next().is_some_and(|v| v.trim() == id.trim()));
    if !listed || points < price {
        return format!("&shp_rslt=0&vcld_pts={points}");
    }
    let left = points - price;
    if let Some(p) = store.records.get_mut(&format!("profile {aime}")) {
        p.insert("vcld_pts".into(), left.to_string());
    }
    store.add_owned(kind, &aime, &id);
    store.save();
    format!("&shp_rslt=1&{id_key}={id}&{have_key}={}&vcld_pts={left}", store.have(kind, &aime))
}
