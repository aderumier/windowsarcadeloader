//! Winning Eleven 2012's own modules (`KCK`): `userdata`, the e-Amusement card's player, and
//! `userenv` (the cabinet's leagues and competitions: none).
//!
//! - `login`: the card's session (`session_id`, 16 characters), whether its player is new
//!   (`is_new`), never forbidden;
//! - `load`: the player's data as the game last saved it (the `userdata` tree of its `save`),
//!   or `is_new` for a card without player (the game then creates the team), every group the
//!   game reads there;
//! - `save`: the player's data kept, in `dev/nvram/easrv/<refid>/KCK.kbin`;
//! - `takeover`: no data of the earlier series (Winning Eleven 2010, 2008) to take over;
//! - `logout`: nothing to do.

use crate::cards::DIR;
use crate::kbin::{Node, Value};

fn path(refid: &str) -> String {
    format!("{DIR}/{refid}/KCK.kbin")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn load(refid: &str) -> Option<Node> {
    if !valid_id(refid) {
        return None;
    }
    std::fs::read(path(refid)).ok().and_then(|b| Node::from_binary(&b))
}

fn store(refid: &str, data: &Node) {
    if !valid_id(refid) {
        return;
    }
    let _ = std::fs::create_dir_all(format!("{DIR}/{refid}"));
    if let Err(e) = std::fs::write(path(refid), data.to_binary()) {
        crate::log!("easrv: KCK {refid}: {e}");
    }
}

fn refid(req: Option<&Node>) -> String {
    req.and_then(|r| r.path("userdata/refid")).map(|n| n.text()).unwrap_or_default()
}

fn s32(name: &str, v: i32) -> Node {
    Node::value(name, Value::S32(v))
}

fn bool(name: &str, v: bool) -> Node {
    Node::value(name, Value::Bool(v))
}

/// The groups the game reads from the player's data, each required (the values in them read 0
/// when absent: the payload's reader, `we2012.rs`): a new player's are empty.
const GROUPS: &[&str] = &[
    "system", "base", "setting", "league", "league_section", "league_vshum", "league_virtual_vshum", "tag", "tag_vshum", "tag_result",
    "freekick", "freekick_result", "formation", "player", "secret_team", "rival", "rival_message", "record", "iconchat", "compe",
    "compe_join_licence", "compe_player", "compe_result", "compe_history_best", "tag_compe_history_best",
];

/// The team's players: always 32 (`player/num`, `player/player0`..`player31`).
const PLAYERS: usize = 32;
/// The rivals: 5 (`rival/rival0`..`rival4`).
const RIVALS: usize = 5;

/// The player's data with every group the game reads (empty when the data has none: the
/// team's 32 players without their values).
fn complete(mut data: Node) -> Node {
    for group in GROUPS {
        if data.find(group).is_none() {
            let node = match *group {
                "player" => Node::new("player").child(s32("num", PLAYERS as i32)).children((0..PLAYERS).map(|i| Node::new(&format!("player{i}")))),
                "rival" => Node::new("rival").children((0..RIVALS).map(|i| Node::new(&format!("rival{i}")))),
                _ => Node::new(group),
            };
            data = data.child(node);
        }
    }
    // what the game's reader (the payload) answers for any other group or list entry missing
    if data.find("empty").is_none() {
        data = data.child(Node::new("empty"));
    }
    data
}

pub fn handle(module: &str, method: &str, req: Option<&Node>) -> Option<Node> {
    // the cabinet's environment (leagues, competitions, rankings): none, every value read 0
    if module == "userenv" && method == "get" {
        return Some(Node::new("userenv").attr("status", 0).child(Node::new("empty")));
    }
    if module != "userdata" {
        return None;
    }
    let refid = refid(req);
    let answer = match method {
        "login" => {
            let session: String = refid.chars().chain(std::iter::repeat('0')).take(16).collect();
            Node::new("userdata").children([
                Node::str("session_id", &session),
                bool("is_new", load(&refid).is_none()),
                bool("is_forbid", false),
                s32("forbid_cause", 0),
            ])
        }
        "load" => complete(match load(&refid) {
            Some(data) => data,
            None => Node::new("userdata").children([s32("mainte_counter", 0), bool("is_new", true)]),
        }),
        "save" => {
            if let Some(data) = req.and_then(|r| r.find("userdata")) {
                // the load's answer: the saved tree, an old player
                let mut kept = Node::new("userdata").children([s32("mainte_counter", 0), bool("is_new", false)]);
                for child in &data.children {
                    if !matches!(child.name.as_str(), "refid" | "session_id" | "mainte_counter" | "is_new" | "empty") {
                        kept = kept.child(child.clone());
                    }
                }
                store(&refid, &kept);
            }
            Node::new("userdata").children([bool("is_success", true), s32("deny_cause", 0)])
        }
        // no data of the earlier series to take over
        "takeover" => Node::new("userdata").children([bool("is_enable", false), bool("is_prev_series", false), bool("is_locatest", false), Node::new("empty")]),
        _ => Node::new("userdata"),
    };
    Some(answer.attr("status", 0))
}
