//! Hello! Pop'n Music (model `JMP`): its `game` module, as the open-source Asphyxia CORE plugin
//! `popn-hello@asphyxia` answers it. The answers are the `game` node, `status` 0 (1: refused).
//!
//! - `common`: the music count (`cnt_music` u32 36), the `flag` attributes `id s1 s2 t` 1;
//! - `shop`, `new`: done;
//! - `load` (`refid`): the player's last play (`last`: `chara`, `level`, `music_id`, `style`
//!   attributes) and each character's affection (`chara`: `id`, `love`);
//! - `save` (`refid`, `last`: `chara level love music_id style`): the last play kept, the
//!   character's affection raised by `love`;
//! - `load_m` (`refid`): the scores (`music music_id` / `style id` / `level id score
//!   clear_type`);
//! - `save_m` (`refid`, `music_id style level score clear_type` attributes): the best score and
//!   clear kept.
//!
//! Kept in `dev/nvram/easrv/<refid>/JMP.kbin`: a `player` node holding `last`, the `chara`
//! nodes and the `music` tree, as answered.

use crate::cards::DIR;
use crate::kbin::{Node, Value};

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn num(node: &Node, name: &str) -> Option<i64> {
    attr(node, name)?.trim().parse().ok().filter(|v: &i64| *v >= 0)
}

fn path(refid: &str) -> String {
    format!("{DIR}/{refid}/JMP.kbin")
}

fn load_player(refid: &str) -> Node {
    std::fs::read(path(refid)).ok().and_then(|b| Node::from_binary(&b)).unwrap_or_else(|| {
        Node::new("player").child(Node::new("last").attr("chara", 0).attr("level", 0).attr("music_id", 0).attr("style", 0))
    })
}

fn save_player(refid: &str, player: &Node) {
    let _ = std::fs::create_dir_all(format!("{DIR}/{refid}"));
    if let Err(e) = std::fs::write(path(refid), player.to_binary()) {
        crate::log!("easrv: JMP {refid}: {e}");
    }
}

/// The child `name` of `node` whose attribute `key` is `value`, created when missing.
fn keyed<'a>(node: &'a mut Node, name: &str, key: &str, value: &str) -> &'a mut Node {
    let i = match node.children.iter().position(|c| c.name == name && attr(c, key) == Some(value)) {
        Some(i) => i,
        None => {
            node.children.push(Node::new(name).attr(key, value));
            node.children.len() - 1
        }
    };
    &mut node.children[i]
}

fn set_attr(node: &mut Node, name: &str, value: impl ToString) {
    match node.attrs.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) => *v = value.to_string(),
        None => node.attrs.push((name.to_string(), value.to_string())),
    }
}

/// The `<response>` child of a `JMP` request, `None`: not one of the game's own.
pub fn handle(module: &str, method: &str, req: Option<&Node>) -> Option<Node> {
    if module != "game" {
        return None;
    }
    let game = req.and_then(|r| r.find("game"));
    let refid = game.and_then(|g| attr(g, "refid")).filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric()));
    let ok = Node::new("game").attr("status", 0);
    let deny = Node::new("game").attr("status", 1);
    Some(match method {
        "common" => ok
            .child(Node::new("flag").attr("id", 1).attr("s1", 1).attr("s2", 1).attr("t", 1))
            .child(Node::value("cnt_music", Value::U32(36))),
        "shop" => ok,
        "new" => match refid {
            Some(refid) => {
                save_player(refid, &load_player(refid));
                ok
            }
            None => deny,
        },
        "load" => match refid {
            Some(refid) => {
                let player = load_player(refid);
                ok.children(player.children.into_iter().filter(|c| c.name == "last" || c.name == "chara"))
            }
            None => deny,
        },
        "load_m" => match refid {
            Some(refid) => ok.children(load_player(refid).children.into_iter().filter(|c| c.name == "music")),
            None => deny,
        },
        "save" => match (refid, game.and_then(|g| g.find("last"))) {
            (Some(refid), Some(last)) => {
                let values: Option<Vec<i64>> = ["chara", "level", "love", "music_id", "style"].iter().map(|k| num(last, k)).collect();
                if let Some(&[chara, level, love, music_id, style]) = values.as_deref() {
                    let mut player = load_player(refid);
                    let chara_node = keyed(&mut player, "chara", "id", &chara.to_string());
                    let total = chara_node.attrs.iter().find(|(k, _)| k == "love").and_then(|(_, v)| v.parse::<i64>().ok()).unwrap_or(0) + love;
                    set_attr(chara_node, "love", total);
                    player.children.retain(|c| c.name != "last");
                    player.children.insert(
                        0,
                        Node::new("last").attr("chara", chara).attr("level", level).attr("music_id", music_id).attr("style", style),
                    );
                    save_player(refid, &player);
                }
                ok
            }
            (None, _) => deny,
            _ => ok,
        },
        "save_m" => match (refid, game) {
            (Some(refid), Some(game)) => {
                let values: Option<Vec<i64>> = ["music_id", "style", "level", "score", "clear_type"].iter().map(|k| num(game, k)).collect();
                if let Some(&[music_id, style, level, score, clear]) = values.as_deref() {
                    let mut player = load_player(refid);
                    let music = keyed(&mut player, "music", "music_id", &music_id.to_string());
                    let style = keyed(music, "style", "id", &style.to_string());
                    let level = keyed(style, "level", "id", &level.to_string());
                    let best = |name: &str, v: i64| attr(level, name).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0).max(v);
                    let (score, clear) = (best("score", score), best("clear_type", clear));
                    set_attr(level, "score", score);
                    set_attr(level, "clear_type", clear);
                    save_player(refid, &player);
                }
                ok
            }
            (None, _) => deny,
            _ => ok,
        },
        _ => return None,
    })
}
