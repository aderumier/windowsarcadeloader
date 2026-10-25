//! Player data (`playerdata.usergamedata_send` / `usergamedata_recv`): a game's records per card
//! (Steel Chronicle Victroopers: `rl1_bas`, `rl1_cus1`... 32 of them), kept in
//! `dev/nvram/easrv/<refid>/<record>.kbin` as the game sent them.
//!
//! - `usergamedata_recv` (`data/refid`, `data/recv_csv`: `<name>,<mask>,...`): `result` 0 and
//!   `player/record` with each asked record's `d` node it has (base64 text, its `bin1` child
//!   base64 binary; its text without the first two fields), in the asked order, and
//!   `player/record_num` (0 records: a new player;
//!   `result` 1 is an error: the game ends the session).
//! - `usergamedata_send` (`data/refid`, `data/record/d...`): each `d` stored under its record
//!   name (`d`'s text decodes to `<mask>,<name>,<data>...`).

use crate::kbin::{Node, Value};

use crate::cards::DIR;

pub(crate) fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let Some(v) = ALPHABET.iter().position(|&a| a == c) else { continue };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn record_path(refid: &str, name: &str) -> String {
    format!("{DIR}/{refid}/{name}.kbin")
}

/// The record a `d` node holds: its decoded text's second field.
fn record_name(d: &Node) -> Option<String> {
    let text = String::from_utf8_lossy(&base64_decode(&d.text())).into_owned();
    let name = text.split(',').nth(1)?.trim();
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')).then(|| name.to_string())
}

/// A kept record as answered: its text without the first two fields (`<mask>,<name>`), as
/// Asphyxia's and bemaniutils' servers answer it (the games read the data fields only), its
/// `bin1` kept.
fn record_answer(d: &Node) -> Node {
    let text = base64_decode(&d.text());
    let fields: Vec<&[u8]> = text.split(|&b| b == b',').collect();
    let data = fields.get(2..).unwrap_or_default().join(&b',');
    Node::str("d", &crate::hck::base64_encode(&data)).children(d.children.iter().cloned())
}

/// The `playerdata.<method>` answer (the `<response>`'s child).
pub fn handle(method: &str, req: Option<&Node>) -> Node {
    let data = req.and_then(|r| r.path("playerdata/data"));
    // the player: `refid` (Steel Chronicle Victroopers), `eaid` (Metal Gear Arcade)
    let refid = ["refid", "eaid"]
        .iter()
        .find_map(|k| data.and_then(|d| d.find(k)).map(|n| n.text()).filter(|t| !t.is_empty()))
        .unwrap_or_default();
    let mut answer = Node::new("playerdata");
    match method {
        "usergamedata_recv" => {
            let csv = data.and_then(|d| d.find("recv_csv")).map(|n| n.text()).unwrap_or_default();
            let names: Vec<String> = csv.split(',').step_by(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let stored: Vec<Option<Node>> = names
                .iter()
                .map(|n| std::fs::read(record_path(&refid, n)).ok().and_then(|b| Node::from_binary(&b)))
                .collect();
            let mut record = Node::new("record");
            let mut count = 0u32;
            for d in stored.into_iter().flatten() {
                record = record.child(record_answer(&d));
                count += 1;
            }
            answer = answer
                .child(Node::value("result", Value::S32(0)))
                .child(Node::new("player").child(record).child(Node::value("record_num", Value::U32(count))));
        }
        "usergamedata_send" => {
            let _ = std::fs::create_dir_all(format!("{DIR}/{refid}"));
            if let Some(record) = data.and_then(|d| d.find("record")) {
                for d in record.children.iter().filter(|c| c.name == "d") {
                    match record_name(d) {
                        Some(name) => {
                            let _ = std::fs::write(record_path(&refid, &name), d.to_binary());
                        }
                        None => crate::log!("easrv: playerdata record without a known name: {}", d.text()),
                    }
                }
            }
            answer = answer.child(Node::value("result", Value::S32(0)));
        }
        // the data of the game's previous version (`lt2_*` records), other players' records by
        // condition (`condrecv`: rankings, rivals): none here
        "usergamedata_inheritance" | "usergamedata_condrecv" => {
            answer = answer
                .child(Node::value("result", Value::S32(0)))
                .child(Node::new("player").child(Node::new("record")).child(Node::value("record_num", Value::U32(0))));
        }
        // a score's rank among the players (Metal Gear Arcade: ess.dll reads `result` s32,
        // `rank/rank` s32, `rank/updatetime` u64): first, no other player here
        "usergamedata_scorerank" => {
            answer = answer.child(Node::value("result", Value::S32(0))).child(
                Node::new("rank").child(Node::value("rank", Value::S32(1))).child(Node::value("updatetime", Value::U64(0))),
            );
        }
        _ => answer = answer.child(Node::value("result", Value::S32(0))),
    }
    answer
}
