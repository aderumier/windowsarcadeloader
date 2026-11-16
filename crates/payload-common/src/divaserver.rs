//! Project DIVA Arcade's title server (`DivaServlet`, the ALL.Net power-on's uri: the router's
//! port 80, `crate::allnet`'s server), from ARTEMiS' `titles/diva` (WTFPL; written
//! for Future Tone, the same commands and keys as this version's). Its boot requests answered
//! (the startup checks: PV INFORMATION, NET PARAM, ...), with ARTEMiS' lists (`diva_data/`: the
//! songs' PV list, the module shop and customize item catalogs, the quests). The players of the
//! Aime cards ([`players`]): registered, their profile and best scores kept in the save folder
//! (`diva_players.txt`), with this version's own keys and bounds; Future Tone's (Nu,
//! [`set_future_tone`]) ARTEMiS' handlers ported whole ([`future_tone`], `diva_ft.txt`); the other
//! commands `stat=ok` with no values.
//!
//! A request's body is the form (`cmd=...&req_id=...&...`) deflated (zlib) then base64, or a
//! multipart body (`THIS_STRING_SEPARATES`) with the form on its fourth line. An answer is
//! `cmd=<cmd>&req_id=<req_id>&stat=ok` and the command's values, plain.

use crate::{allnet, log};

mod future_tone;
mod players;

/// The lists' last update time (the game asks for a list again when it changes).
const LUT: &str = "2020-01-01 00:00:00.0";

/// Python's `urllib.parse.quote` (`/` kept).
fn quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Python's `urllib.parse.urlencode` of `params` (values `quote_plus`ed), `,` kept, spaces as
/// `%20` with `plus_as_20`.
fn urlencode(params: &[(&str, String)], plus_as_20: bool) -> String {
    let encoded: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", quote(v).replace("%20", "+").replace("/", "%2F"))).collect();
    let mut s = encoded.join("&");
    if plus_as_20 {
        s = s.replace('+', "%20");
    }
    s.replace("%2C", ",")
}

/// A catalog's lines (the file's, newline included), each quoted with its comma, quoted again.
fn catalog(data: &str) -> String {
    let all: String = data.split_inclusive('\n').map(|line| quote(&(quote(line) + ","))).collect();
    // its last 3 characters dropped (ARTEMiS': the end of the last comma, `%252C`)
    all[..all.len().saturating_sub(3)].to_string()
}

fn attend() -> String {
    let params = [
        ("atnd_prm1", "0,1,1,0,0,0,1,0,100,0,0,0,0,0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1".to_string()),
        ("atnd_prm2", "30,10,100,4,1,50,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1".into()),
        (
            "atnd_prm3",
            "100,0,1,1,1,1,1,1,1,1,2,3,4,1,1,1,3,4,5,1,1,1,4,5,6,1,1,1,5,6,7,4,4,4,9,10,14,5,10,10,25,20,50,30,90,5,10,10,25,20,50,30,90,5,10,10,25,20,50,30,90,5,10,10,25,20,50,30,90,5,10,10,25,20,50,30,90,10,30,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0".into(),
        ),
        ("atnd_lut", LUT.into()),
    ];
    format!("&{}", urlencode(&params, false))
}

fn ping() -> String {
    let none = "-1,-1,-1,-1,-1,-1,-1,-1,-1,-1";
    let params = [
        ("ping_b_msg", "Welcome to WindowsArcadeLoader!".to_string()),
        ("ping_m_msg", "xxx".into()),
        ("atnd_lut", LUT.into()),
        ("fi_lut", LUT.into()),
        ("ci_lut", LUT.into()),
        ("qi_lut", LUT.into()),
        ("pvl_lut", "2021-05-22 12:08:16.0".into()),
        ("shp_ctlg_lut", "2020-06-10 19:44:16.0".into()),
        ("cstmz_itm_ctlg_lut", "2019-10-08 20:23:12.0".into()),
        ("ngwl_lut", "2019-10-08 20:23:12.0".into()),
        ("rnk_nv_lut", "2020-06-10 19:51:30.0".into()),
        ("rnk_ps_lut", LUT.into()),
        ("bi_lut", "2020-09-18 10:00:00.0".into()),
        ("cpi_lut", "2020-10-25 09:25:10.0".into()),
        ("bdlol_lut", "2020-09-18 10:00:00.0".into()),
        ("p_std_hc_lut", "2019-08-01 04:00:36.0".into()),
        ("p_std_i_n_lut", "2019-08-01 04:00:36.0".into()),
        ("pdcl_lut", "2019-08-01 04:00:36.0".into()),
        ("pnml_lut", "2019-08-01 04:00:36.0".into()),
        ("cinml_lut", "2019-08-01 04:00:36.0".into()),
        ("rwl_lut", "2019-08-01 04:00:36.0".into()),
        ("req_inv_cmd_num", none.into()),
        ("req_inv_cmd_prm1", none.into()),
        ("req_inv_cmd_prm2", none.into()),
        ("req_inv_cmd_prm3", none.into()),
        ("req_inv_cmd_prm4", none.into()),
        ("pow_save_flg", "0".into()),
        ("nblss_dnt_p", "100".into()),
        ("nblss_ltt_rl_vp", "1500".into()),
        ("nblss_ex_ltt_flg", "1".into()),
        ("nblss_dnt_st_tm", "2019-07-15 12:00:00.0".into()),
        ("nblss_dnt_ed_tm", "2019-09-17 12:00:00.0".into()),
        ("nblss_ltt_st_tm", "2019-09-18 12:00:00.0".into()),
        ("nblss_ltt_ed_tm", "2019-09-22 12:00:00.0".into()),
    ];
    format!("&{}", urlencode(&params, true))
}

fn pv_list() -> String {
    let lists = [
        include_str!("diva_data/PvList0.dat"),
        include_str!("diva_data/PvList1.dat"),
        include_str!("diva_data/PvList2.dat"),
        include_str!("diva_data/PvList3.dat"),
        include_str!("diva_data/PvList4.dat"),
    ];
    format!("&pvl_lut={}&pv_lst={}", quote(LUT), lists.join(","))
}

/// Future Tone's server (`set_future_tone`): its festivals' attributes (`fi_attr`).
static FUTURE_TONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The game is Project DIVA Arcade Future Tone (Nu), the version ARTEMiS's server answers.
pub fn set_future_tone() {
    FUTURE_TONE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The festivals: none running (they ended). Future Tone's `fi_attr` only for it: the older
/// Project DIVA Arcade refuses the answer with a key it does not know (NET PARAM stuck at 3/4),
/// Future Tone without it (NET PARAM stuck at 5/6).
fn festa_info() -> String {
    let mut params = vec![
        ("fi_id", "1,2".to_string()),
        ("fi_name", "WindowsArcadeLoader Opening,Project DIVA Festa".into()),
        // 0 pink, 1 green
        ("fi_kind", "1,0".into()),
        ("fi_difficulty", "-1,-1".into()),
        ("fi_pv_id_lst", "ALL,ALL".into()),
    ];
    if FUTURE_TONE.load(std::sync::atomic::Ordering::Relaxed) {
        params.push(("fi_attr", "7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF,7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF".into()));
    }
    params.extend([
        ("fi_add_vp", "20".to_string()),
        ("fi_mul_vp", "1".into()),
        ("fi_st", "2019-01-01 00:00:00.0,2019-01-01 00:00:00.0".into()),
        ("fi_et", "2029-01-01 00:00:00.0,2029-01-01 00:00:00.0".into()),
        ("fi_lut", LUT.into()),
    ]);
    format!("&{}", urlencode(&params, true))
}

fn qst_inf() -> String {
    let quests: Vec<String> = include_str!("diva_data/QuestInfo.dat").split_inclusive('\n').map(quote).collect();
    let none = vec!["%2D1"; 11].join("%2C");
    format!("&qi_lut={}&qhi_str={}&qrai_str={none},{none},{none}", quote(LUT), quests.join(","))
}

/// The command's values (after `cmd`, `req_id`, `stat`).
fn values(cmd: &str, form: &[(String, String)]) -> String {
    if FUTURE_TONE.load(std::sync::atomic::Ordering::Relaxed) {
        if let Some(v) = future_tone::values(cmd, form) {
            return v;
        }
    } else if let Some(v) = players::values(cmd, form) {
        return v;
    }
    match cmd {
        "attend" => attend(),
        "ping" => ping(),
        "pv_list" => pv_list(),
        "shop_catalog" => format!("&shp_ctlg_lut={}&shp_ctlg={}", quote(LUT), catalog(include_str!("diva_data/ShopCatalog.dat"))),
        "cstmz_itm_ctlg" => format!("&cstmz_itm_ctlg_lut={}&cstmz_itm_ctlg={}", quote(LUT), catalog(include_str!("diva_data/ItemCatalog.dat"))),
        "festa_info" => festa_info(),
        "contest_info" => format!("&ci_lut={}&ci_str={}", quote(LUT), vec!["%2A%2A%2A"; 8].join(",")),
        "qst_inf" => qst_inf(),
        _ => String::new(),
    }
}

/// The request's form: base64 + zlib, or multipart (its fourth line).
fn form(body: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(body);
    let plain = if text.contains("THIS_STRING_SEPARATES") {
        text.lines().nth(3).unwrap_or("").to_string()
    } else {
        let packed = allnet::base64_decode(body);
        let inflated = miniz_oxide::inflate::decompress_to_vec_zlib(&packed).unwrap_or_default();
        unquote(&String::from_utf8_lossy(&inflated))
    };
    plain.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// `allnet::Title`: the answer to a `DivaServlet` request.
pub fn answer(path: &str, body: &[u8]) -> Vec<u8> {
    let form = form(body);
    let value = |key: &str| form.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str());
    let cmd = value("cmd");
    let players = players::COMMANDS.contains(&cmd) || future_tone::COMMANDS.contains(&cmd);
    if players {
        let keys: Vec<String> = form.iter().filter(|(k, _)| !matches!(k.as_str(), "cmd" | "req_id")).map(|(k, v)| format!("{k}={v}")).collect();
        log!("diva: {path} {cmd} {}", keys.join("&"));
    } else {
        log!("diva: {path} {cmd}");
    }
    let reply = format!("cmd={cmd}&req_id={}&stat=ok{}", value("req_id"), values(cmd, &form));
    if players {
        log!("diva: -> {reply}");
    }
    reply.into_bytes()
}
