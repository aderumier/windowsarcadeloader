//! Border Break Scramble's title server (the ALL.Net power-on's uri `/SBVA/`, the router's port
//! 80, `wal_payload_common::allnet`'s server). Enabled with `WAL_RINGEDGE_ALLNET=borderbreak`.
//!
//! The game's own HTTP client (`RingEdge-BorderBreak/4.5`) posts each request as a form
//! (`command=init_request&protocol_version=...`), deflated then base64 (`Pragma: DFI`), and reads
//! the answer the same way (`NetData*Response::Text2Class`, 0x92bb10 for init): `key=value`
//! pairs separated by `&`. A value is URL-encoded after the game's escaping (`d` `d0`, `e` `e0`,
//! `x` `xx`, the empty string `x`); a structure's fields end with `del` (`command_common`: the
//! request's number, a time; `response_header`: a number). The answer to `init_request`
//! (`init_response`): `command`, `protocol_version`, `command_common`, `response_header`,
//! `local_uid`, `db_start_time`, `db_stop_time`. `attend_request`: the shop's settings
//! ([`attend`]); a structure's arrays and sub-structures each end with one more `del`. The other
//! requests are logged, not answered yet.

use wal_payload_common::log;

fn value<'a>(form: &'a str, key: &str) -> &'a str {
    form.split('&').find_map(|p| p.strip_prefix(key)?.strip_prefix('=')).unwrap_or("")
}

/// The game's escaping (`d` `d0`, `e` `e0`, `x` `xx`, empty `x`), then URL-encoding.
fn encode(s: &str) -> String {
    if s.is_empty() {
        return "x".into();
    }
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            'd' => escaped.push_str("d0"),
            'e' => escaped.push_str("e0"),
            'x' => escaped.push_str("xx"),
            c => escaped.push(c),
        }
    }
    let mut out = String::new();
    for b in escaped.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The game's escaping undone (`d0` `d`, `e0` `e`, `xx` `x`).
fn unescape(s: &str) -> String {
    s.replace("d0", "d").replace("e0", "e").replace("xx", "x")
}

/// The server's time as the game writes it (`2026-11-13 19:38:13.0`).
fn now() -> String {
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.0", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// A structure's scalar fields, `0` each (`0del`).
fn zeros(n: usize) -> String {
    "0del".repeat(n)
}

/// An array of `n` zeros: its fields, then its closing `del`.
fn array(n: usize) -> String {
    zeros(n) + "del"
}

/// `count` elements of a sub-structure, each closed by one more `del`, then the list's closing.
fn elements(count: usize, element: &str) -> String {
    format!("{element}del").repeat(count) + "del"
}

/// One `rules` element (`FUN_008fa2e0`): 161 fields, 187 `del`s.
fn rule() -> String {
    // the stages: the game's stage table (0x1280490: 0 NULL, an empty field; 11 ILND01, 13
    // EURO01, 15 CANY01, 22 BILD01...)
    let stages = "11del13del15del22deldel";
    // rotation_repeat_count[4], then the battle's settings (the rule's dump, FUN_008fc5f0):
    // map_member, bot_member, map_limit_time, map_limit_core, tmp_limit_time, tmp_limit_core,
    // battle_time (seconds), respawn_interval, map_to_truemap, map_to_tempmap,
    // map_close_team_diff, break_chance_rate, doubleup_rate, cp_bounus_rate, ev_bonus_rate,
    // sp_bonus_rate, battle_kind, (unnamed), extra_flag, disp_info_dist_ratio, lockon_dist_ratio,
    // cp_vital_ratio, cp_conquer_ratio; all 0, a battle ends as it starts
    let settings = [10, 10, 600, 100, 600, 100, 600, 10, 0, 0, 0, 100, 100, 100, 100, 100, 0, 0, 0, 100, 100, 100, 100];
    let mut r = stages.to_string() + &array(4) + &settings.iter().map(|v| format!("{v}del")).collect::<String>();
    r += &elements(4, &zeros(2));
    r += &elements(4, &array(4));
    r += &array(16);
    r += &array(2);
    r += &zeros(3);
    r += &elements(5, &zeros(7));
    r += &array(40);
    r += &array(10);
    r
}

/// One `event_param` element (`FUN_008f33e0`): 123 fields, 144 `del`s (strings empty, `x`).
fn event_param() -> String {
    let mut p = "xdel".repeat(4) + &zeros(4) + &"xdel".repeat(4) + &zeros(2);
    p += &array(4).repeat(4);
    p += &elements(4, &zeros(2));
    p += &array(16);
    p += &array(22);
    p += &array(2);
    p += &array(4).repeat(4);
    p += &array(3);
    p += &zeros(3);
    for _ in 0..3 {
        p += &array(3);
        p += &zeros(1);
    }
    p += &array(4);
    p += &zeros(7);
    p
}

/// `attend_response` (`FUN_00902040`): the shop's game settings, all zero; the lists' sizes fixed
/// by the parser (`map_id` 40, `rules` 16, `event_attr`/`event_param`/`event_result` 3,
/// `ranking` 4), every key required.
fn attend(protocol: &str, common: &str) -> String {
    let update = encode("2026-01-01 00:00:00.0");
    let mut kv: Vec<(&str, String)> = vec![
        ("command", encode("attend_response")),
        ("protocol_version", protocol.into()),
        ("command_common", common.into()),
        ("response_header", "0del".into()),
        ("ms_url", encode("")),
        // the matching server's hours and maintenance: full times (the game reads them with its
        // `YYYY-MM-DD hh:mm:ss` parser, else the answer is refused), open all day
        ("ms_open", encode("2000-01-01 00:00:00.0")),
        ("ms_close", encode("2000-01-01 23:59:00.0")),
        ("ms_maintenance_time", encode("2000-01-01 05:00:00.0")),
        ("gp_rate_tempmap", "0".into()),
        ("cp_rate_on_time", "0".into()),
        ("map_id", zeros(40)),
        ("rules", format!("{}del", rule()).repeat(16)),
        ("ranking_coefficient", zeros(9)),
        ("event_attr", format!("{}del", zeros(3)).repeat(3)),
        ("event_param", format!("{}del", event_param()).repeat(3)),
        ("event_result", format!("{}del", zeros(2) + &array(5)).repeat(3)),
        ("ranking", format!("{}del", array(3).repeat(8)).repeat(4)),
    ];
    for key in ["ngword_update", "ranking_update", "event_update", "replay_update", "seal_update", "cap_update", "paramadjust_update", "placelist_update", "nickname_update", "attend_update", "book_keep_flg"] {
        kv.push((key, update.clone()));
    }
    for key in [
        "ms_port", "ms_ping", "ms_maintenance_week", "ms_match_flag", "ul_start", "dl_start", "ul_interval", "dl_interval", "ul_max_size", "dl_count", "illegal_delay", "illegal_recovery",
        "exchange_rate", "max_exchange_rearity", "material_daily_bonus_table", "material_daily_bonus_num", "material_bonus_probability", "material_multi_ratio", "bonus_adder_open",
        "gp_by_1credit", "gp_by_2credit", "gp_by_3credit", "gp_by_5credit", "gp_continue_bonus", "gp_bonus_count", "gp_tutorial1", "gp_tutorial2", "gp_color_change",
        "gp_character_change", "cp_rate_by_gp", "cp_max", "chip_max_battle_time", "chip_max_point", "new_material_probability", "sv_setting_version", "rank_battle_count_max",
        "max_scrm_rank", "max_team_scrm_rank", "ms_stop_flag", "pushed_flag", "extra_flag",
    ] {
        kv.push((key, "0".into()));
    }
    kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// A response: the header (`command`, `protocol_version`, `command_common`,
/// `response_header`), then the command's own `key=value` pairs.
fn response(name: &str, protocol: &str, common: &str, fields: &[(&str, String)]) -> String {
    let mut out = format!("command={}&protocol_version={protocol}&command_common={common}&response_header=0del", encode(name));
    for (k, v) in fields {
        out += &format!("&{k}={v}");
    }
    out
}

/// The data's last update (a full time: the game parses it, else drops the answer).
fn update() -> String {
    encode("2026-01-01 00:00:00")
}

/// `params_response`: the game's 256 float and 256 integer parameters.
fn params(protocol: &str, common: &str) -> String {
    response("params_response", protocol, common, &[("update_timestamp", update()), ("param_update", "0".into()), ("param_floatval", zeros(256)), ("param_intbal", zeros(256))])
}

/// `seal_response`: the 256 seal items.
fn seal(protocol: &str, common: &str) -> String {
    response("seal_response", protocol, common, &[("update_timestamp", update()), ("param_update", "0".into()), ("seal_item_id", zeros(256))])
}

/// `cap_response`: no items (`have_item` / `have_avater`, base64, empty).
fn cap(protocol: &str, common: &str) -> String {
    response(
        "cap_response",
        protocol,
        common,
        &[("update_timestamp", update()), ("cap_update", "0".into()), ("have_item", encode("")), ("have_avater", encode("")), ("cap_open_rank", "0".into())],
    )
}

/// `advertiseinfo_response`: the stages (2 current, 2 next: a 4-int array and 2 ints each).
fn advertiseinfo(protocol: &str, common: &str) -> String {
    let stage = format!("{}{}del", array(4), zeros(2));
    response(
        "advertiseinfo_response",
        protocol,
        common,
        &[("publicity_image_id", "0".into()), ("current_stage_idx", zeros(2)), ("next_stage", stage.repeat(2)), ("next_stage_start", update())],
    )
}

/// `ping_response`: no news, no file to fetch, no operator command; `book_keep_flg` 8 zero
/// bytes (base64).
fn ping(protocol: &str, common: &str) -> String {
    let int = |k| (k, "0".to_string());
    response(
        "ping_response",
        protocol,
        common,
        &[
            ("attend_update", update()),
            ("book_keep_flg", encode("AAAAAAAAAAA=")),
            int("ms_stop_flag"),
            int("client_stat"),
            ("news_text", encode("")),
            int("news_kind"),
            ("news_update", update()),
            int("book_keep_id"),
            ("book_keep_req_time", update()),
            int("file_type"),
            ("file_file_name", encode("")),
            int("file_command_id"),
            int("file_unit_size"),
            int("file_interval"),
            int("opcmd_id"),
            int("opcmd_command"),
            int("opcmd_param"),
        ],
    )
}

/// `WAL_RINGEDGE_BB_ROLE` (default `1`): the cabinet's role in the shop (0 satellite, 1 server,
/// 2 title server, 3 live), byte 0x30 of its settings record, copied to DAT_01696ad8 at boot
/// (FUN_0048f9c0). A satellite (a new backup's 0) waits for its shop's server cabinet at "LOCAL
/// GAME SERVER ..." (its LFS client's UDP discovery, port 30001); the server runs the shop's LFS
/// server itself. Set right after the copy (its CRC, the record's first dword, set).
pub(crate) fn set_role() {
    let role = std::env::var("WAL_RINGEDGE_BB_ROLE").ok().and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(1);
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    std::thread::spawn(move || {
        let (crc, at) = ((base + 0x1296aa8) as *const u32, (base + 0x1296ad8) as *mut u8);
        for _ in 0..60_000 {
            if unsafe { std::ptr::read_volatile(crc) } != 0 {
                let old = unsafe { std::ptr::read_volatile(at) };
                unsafe { std::ptr::write_volatile(at, role) };
                log!("bbserver: cabinet role {old} -> {role}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
}

/// The common free play option (`WAL_RINGEDGE_FREEPLAY`, else `WAL_FREEPLAY`): the system test
/// menu's FREE PLAY, amCredit's configuration byte 2 (0x128855a: its coin and credit checks
/// skipped when 1), copied at boot from the EEPROM credit record's byte 0xa (0x16014e2) by
/// amCreditInit (FUN_0097d320, done when 0x1288550 is set): both set once it has run. Off: the
/// game's own setting. (A code patch can't do it: the image is relocated, its addresses too.)
pub(crate) fn free_play() {
    if wal_payload_common::free_play("WAL_RINGEDGE_FREEPLAY") != Some(true) {
        return;
    }
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    std::thread::spawn(move || {
        let (ready, config, record) = ((base + 0xe88550) as *const u32, (base + 0xe8855a) as *mut u8, (base + 0x12014e2) as *mut u8);
        for _ in 0..120_000 {
            if unsafe { std::ptr::read_volatile(ready) } != 0 {
                unsafe {
                    std::ptr::write_volatile(config, 1);
                    std::ptr::write_volatile(record, 1);
                }
                log!("bbserver: free play");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
}

/// The extended image (the cabinet's second program, its storage's `Y:\SBVA.extend`) marked
/// installed: the boot checks for the two files (empty) on its data root Y:, else "Error 0913
/// Game Program Not Found on Storage Device". Created when missing.
pub(crate) fn extend_markers() {
    for name in ["Y:\\SBVA.extend", "Y:\\SBVA.extend.complete"] {
        if !std::path::Path::new(name).exists() {
            match std::fs::write(name, b"") {
                Ok(()) => log!("bbserver: {name} created"),
                Err(e) => log!("bbserver: {name}: {e}"),
            }
        }
    }
}

pub(crate) fn answer(path: &str, body: &[u8]) -> Vec<u8> {
    // the form deflated (zlib) then base64 (`Pragma: DFI`), or plain
    let inflated = miniz_oxide::inflate::decompress_to_vec_zlib(&wal_payload_common::allnet::base64_decode(body)).ok();
    let text = String::from_utf8_lossy(inflated.as_deref().unwrap_or(body)).into_owned();
    let command = value(&text, "command");
    log!("bbserver: {path} ({} bytes): {}", body.len(), text.trim());
    let protocol = value(&text, "protocol_version");
    // command_common: the request's number, echoed with the server's time
    let number = value(&text, "command_common").split("del").next().unwrap_or("0");
    let common = format!("{number}del{}del", encode(&now()));
    let (p, c) = (protocol, common.as_str());
    let header = |name: &str, fields: &[(&str, String)]| response(name, p, c, fields);
    let int = |k| (k, "0".to_string());
    let answer = match unescape(command).as_str() {
        "init_request" => header(
            "init_response",
            &[("local_uid", "1".into()), ("db_start_time", encode("2000-01-01 00:00:00.0")), ("db_stop_time", encode("2099-12-31 23:59:59.0"))],
        ),
        "attend_request" => attend(p, c),
        "params_request" => params(p, c),
        "seal_request" => seal(p, c),
        "cap_request" => cap(p, c),
        "advertiseinfo_request" => advertiseinfo(p, c),
        "ping_request" => ping(p, c),
        // no delivery (`instruction_id` 0; its times must still be valid)
        "delivinst_request" => header(
            "delivinst_response",
            &[
                int("instruction_id"),
                ("instruction_order_time", update()),
                ("instruction_release_time", update()),
                ("instruction_interval", zeros(4)),
                ("instruction_cloud", zeros(48)),
                int("instruction_part_size"),
                int("instruction_report_interval"),
                int("instruction_flag"),
                ("instruction_partition", encode("")),
            ],
        ),
        // reports and the event ranking: no answer parsed, the header frees the connection
        "delivreport_request" => header("delivreport_response", &[]),
        "event_rank_request" => header("event_rank_response", &[]),
        "places_request" => header("places_response", &[("placelist_update", update()), ("placelist", encode(""))]),
        "ngword_request" => header("ngword_response", &[("ng_word", encode(""))]),
        // no ranking (an empty `ranking_flag`)
        "rank_request" => header(
            "rank_response",
            &[
                ("ranking_flag", encode("")),
                int("ranking_data_version"),
                ("ranking_start", update()),
                ("ranking_end", update()),
                int("ranking_is_compleat"),
                int("ranking_month"),
                ("ranking_param", encode("")),
            ],
        ),
        "nickname_request" => header("nickname_response", &[("nickname_update", update()), ("nickname_perts_string", encode(""))]),
        // no image to download (an empty file list)
        "imginfo_request" => header(
            "imginfo_response",
            &[
                int("image_list_id"),
                ("image_open_time", update()),
                ("image_close_time", update()),
                ("image_release_time", update()),
                int("image_dl_interval"),
                ("image_filelist", encode("")),
            ],
        ),
        "image_request" => header("image_response", &[("image_file_binary", encode("")), int("image_file_size"), int("image_crc")]),
        "eventinfo_request" => header(
            "eventinfo_response",
            &[("event_attr", format!("{}del", zeros(3)).repeat(3)), ("event_result", format!("{}del", zeros(2) + &array(5)).repeat(3))],
        ),
        "file_request" => header("file_response", &[int("file_file_index"), int("file_index")]),
        "selecterinfo_request" => header("selecterinfo_response", &[int("clan_id"), int("playing_member_clan")]),
        _ => String::new(),
    };
    if !answer.is_empty() {
        log!("bbserver: -> ({} bytes) {}", answer.len(), &answer[..answer.len().min(300)]);
    }
    answer.into_bytes()
}
