//! Winning Eleven 2008 (model `HCK`): its own modules (`ac_net_*` in game.exe) on a 2008
//! e-Amusement library.
//!
//! Most answers are wrapped: the module node holds `data` strings, base64 chunks of a document
//! (at most 1020 characters each: the game reads each into 1 KB) whose `/player` node holds the
//! answer's fields. The session methods (`playerdata.login` / `logout`...) answer directly in
//! the module node (`status` str, `invalid` a failure; `comid` str, `login` / `logout` s32).
//!
//! The player's data (`usergamedatadetail_send` / `_recv`, by card: `eaid`) is records by key
//! (`T1I01`, `T1S01`...), a CSV line of the asked columns each (`data/columns`: `vk` the key,
//! `i1`.. integers, `d1`.. reals, `l1`.. longs, `s1`.. strings), maybe with a `bin` child. The
//! game sends each as a `d` string: base64 of its length (u32) and its LZSS-compressed text;
//! it reads them back as base64 of the text. Kept in `dev/nvram/easrv/<eaid>/HCK/<key>.kbin`
//! (the columns, the text, the binary), answered in the asked keys' order (`<NODATA>` for a key
//! never sent) with the asked columns.

use crate::cards::DIR;
use crate::kbin::{Node, Value};
use crate::playerdata::base64_decode;

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// The module node of a wrapped answer: `player` as base64 `data` chunks.
fn wrapped(module: &str, player: Node) -> Node {
    let text = base64_encode(&player.to_binary());
    let chunks: Vec<Node> = text.as_bytes().chunks(1020).map(|c| Node::str("data", std::str::from_utf8(c).unwrap_or_default())).collect();
    Node::new(module).children(chunks)
}

fn s32(name: &str, v: i32) -> Node {
    Node::value(name, Value::S32(v))
}

fn u32(name: &str, v: u32) -> Node {
    Node::value(name, Value::U32(v))
}

/// A sent record's text: LZSS (a 4 KB ring from 0xfee, zeros; a flag byte, from its lowest
/// bit: 1 a literal byte, 0 a 2-byte reference, 12-bit ring position and length - 3).
fn record_text(d: &Node) -> Option<Vec<u8>> {
    let data = base64_decode(&d.text());
    let len = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let mut ring = [0u8; 4096];
    let mut at = 0xfee;
    let mut out = Vec::with_capacity(len);
    let mut i = 4;
    while out.len() < len {
        let flags = *data.get(i)?;
        i += 1;
        for bit in 0..8 {
            if out.len() >= len {
                break;
            }
            if flags >> bit & 1 == 1 {
                let b = *data.get(i)?;
                i += 1;
                out.push(b);
                ring[at] = b;
                at = (at + 1) & 0xfff;
            } else {
                let (lo, hi) = (*data.get(i)? as usize, *data.get(i + 1)? as usize);
                i += 2;
                let from = lo | (hi & 0xf0) << 4;
                for k in 0..(hi & 0x0f) + 3 {
                    let b = ring[(from + k) & 0xfff];
                    out.push(b);
                    ring[at] = b;
                    at = (at + 1) & 0xfff;
                }
            }
        }
    }
    Some(out)
}

fn record_path(eaid: &str, key: &str) -> String {
    format!("{DIR}/{eaid}/HCK/{key}.kbin")
}

/// The card's id in a request (`data/eaid`), when usable as a directory name.
fn eaid(req: Option<&Node>, module: &str) -> Option<String> {
    let id = req?.path(&format!("{module}/data/eaid"))?.text();
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

/// `usergamedatadetail_send`: each record kept under its key.
fn send(module: &str, req: Option<&Node>) -> Node {
    let data = req.and_then(|r| r.path(&format!("{module}/data")));
    let columns = data.and_then(|d| d.find("columns")).map(|n| n.text()).unwrap_or_default();
    let mut kept = 0;
    if let (Some(eaid), Some(record)) = (eaid(req, module), data.and_then(|d| d.find("record"))) {
        let _ = std::fs::create_dir_all(format!("{DIR}/{eaid}/HCK"));
        for d in record.children.iter().filter(|c| c.name == "d") {
            let Some(text) = record_text(d) else {
                crate::log!("easrv: HCK record not decoded: {}", d.text());
                continue;
            };
            let key = String::from_utf8_lossy(text.split(|&b| b == b',').next().unwrap_or_default()).trim().to_string();
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
                continue;
            }
            let mut kept_record = Node::new("record").child(Node::str("columns", &columns)).child(Node::value("text", Value::Bin(text)));
            if let Some(bin) = d.find("bin") {
                kept_record = kept_record.child(bin.clone());
            }
            if std::fs::write(record_path(&eaid, &key), kept_record.to_binary()).is_ok() {
                kept += 1;
            }
        }
    }
    crate::log!("easrv: HCK {kept} records kept");
    Node::new(module).child(s32("result", 0))
}

/// A kept record as a `d` answer, its text with the `columns` asked (an unknown one empty).
fn record_d(kept: &Node, columns: &str) -> Node {
    let kept_columns = kept.find("columns").map(|n| n.text()).unwrap_or_default();
    let names: Vec<&str> = kept_columns.split(',').collect();
    let text = kept.find("text").map(|n| n.raw.clone()).unwrap_or_default();
    let values: Vec<&[u8]> = text.split(|&b| b == b',').collect();
    let mut out = Vec::new();
    for column in columns.split(',') {
        if let Some(v) = names.iter().position(|n| *n == column).and_then(|i| values.get(i)) {
            out.extend_from_slice(v);
        }
        out.push(b',');
    }
    let d = Node::str("d", &base64_encode(&out));
    match kept.find("bin") {
        Some(bin) => d.child(bin.clone()),
        None => d,
    }
}

/// `usergamedatadetail_recv`: the asked keys' records (`data/vkeycsv`), in order.
fn recv(module: &str, req: Option<&Node>) -> Node {
    let eaid = eaid(req, module).unwrap_or_default();
    let data = req.and_then(|r| r.path(&format!("{module}/data")));
    let keys = data.and_then(|d| d.find("vkeycsv")).map(|n| n.text()).unwrap_or_default();
    let columns = data.and_then(|d| d.find("columns")).map(|n| n.text()).unwrap_or_default();
    let records: Vec<Node> = keys
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .take(50)
        .map(|k| match std::fs::read(record_path(&eaid, k)).ok().and_then(|b| Node::from_binary(&b)) {
            Some(kept) => record_d(&kept, &columns),
            None => Node::str("d", "<NODATA>"),
        })
        .collect();
    let num = records.len() as u32;
    wrapped(module, Node::new("player").child(s32("result", 0)).child(u32("record_num", num)).child(Node::new("record").children(records)))
}

/// The `<response>` child of a `HCK` request, `None`: not one of the game's own.
pub fn handle(module: &str, method: &str, req: Option<&Node>) -> Option<Node> {
    Some(match (module, method) {
        // the operator's settings by name, base64 text: `SETTING` the shop-to-shop match's
        // (`ONLINE_MATCH_SETTING`, five `|`-separated numbers: 6-20, 0-1, 0-2, 30-199, ..),
        // others (`OSHIRASE` news, `TABOONAME` banned names...) none
        ("netconfig", "get") => {
            let name = req.and_then(|r| r.path("netconfig/data/config_name")).map(|n| n.text()).unwrap_or_default();
            let value = if name == "SETTING" { "10|0|0|60|0" } else { "" };
            wrapped(module, Node::new("player").child(Node::str("config_value", &base64_encode(value.as_bytes()))))
        }
        // stored procedures (leagues, tournaments, rankings): no result (base64 text; `dbfunc_call`
        // reads it wrapped, `dbfunc_call64` in the module node)
        (_, "dbfunc_call" | "dbfunc_call64") => {
            wrapped(module, Node::new("player").child(Node::str("result", ""))).child(Node::str("result", ""))
        }
        // records by condition (rankings): none
        ("ranking" | "playerdata", "getlist") => {
            wrapped(module, Node::new("player").child(s32("result", 0)).child(u32("record_num", 0)).child(Node::new("record")))
        }
        ("playerdata", "usergamedatadetail_send") => send(module, req),
        ("playerdata", "usergamedatadetail_recv") => recv(module, req),
        // the player's data of the previous games (Winning Eleven 2006's, the location test's):
        // none to take over
        ("takeover", "load") => Node::new(module).children(
            [
                "flg_comp", "flg_play_we06", "flg_play_loca", "negolv_we06", "shogo_point", "tk_item_rank1", "tk_item_rank2",
                "tk_item_rank3", "tk_item_rank10", "tk_item_rank100", "tk_item_final", "tk_item_entry", "negolv_loca",
            ]
            .map(|n| s32(n, 0)),
        ),
        // the card's session: its id (`comid`, sent back at the logout), the counts of logins and
        // logouts
        ("playerdata", "login" | "logout") => {
            let comid = format!("{:08X}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
            Node::new(module).child(Node::str("status", "valid")).child(Node::str("comid", &comid)).child(s32("login", 1)).child(s32("logout", 0))
        }
        _ => return None,
    })
}
