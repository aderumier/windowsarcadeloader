//! Dance Dance Revolution 2013 (model `MDX`): its `game` module, as the open-source bemaniutils
//! backend (`bemani/backend/ddr/ddr2013.py` and `common.py`, public domain) answers it.
//!
//! Each card's player is kept in `dev/nvram/easrv/<refid>/MDX.kbin`: a `player` node holding
//! the `profile` (scalars as attributes; `last`, `title`, `title_gr` nodes; the arrays the game
//! saves), the `scores` (one `s` per song and chart: best score, plays, rank, combo type, best
//! combo, the five latest scores, the best play's `trace`) and the `courses`.
//!
//! - `common`: 256 event flags (all final), the songs' play counts (700 zeros);
//! - `shop` (`stop` 0), `lock` (`now_login` 0), `tax_info` (phase 0), `log`, `message`,
//!   `ranking`, `recorder`, `old` (no previous version): done;
//! - `new` (`refid`, `name`, `area`): a new player; `load`: its profile (`none` 0 without);
//!   `save`: the profile's changes (play counts, experience, stars, options, flags, the groove
//!   radar, rivals by code);
//! - `load_m` (`refid`, or a rival's `code`), `save_m` (`mid`, `mtype`, `data`, `trace`),
//!   `score` (the five latest), `trace` (the best play's steps);
//! - `hiscore`, `area_hiscore` (`shop_area`): the best score of every chart among the players;
//! - `friend` (`code`): a rival; `load_daily`; `load_c`, `save_c`: courses.

use crate::cards::DIR;
use crate::kbin::{Node, Value};

/// The players' region (Tokyo).
const REGION: i64 = 13;
const MAX_SONGS: usize = 700;

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn attr_i(node: &Node, name: &str, default: i64) -> i64 {
    attr(node, name).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn set_attr(node: &mut Node, name: &str, value: impl ToString) {
    match node.attrs.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) => *v = value.to_string(),
        None => node.attrs.push((name.to_string(), value.to_string())),
    }
}

fn child_mut<'a>(node: &'a mut Node, name: &str) -> &'a mut Node {
    let i = match node.children.iter().position(|c| c.name == name) {
        Some(i) => i,
        None => {
            node.children.push(Node::new(name));
            node.children.len() - 1
        }
    };
    &mut node.children[i]
}

fn set_child(node: &mut Node, child: Node) {
    match node.children.iter_mut().find(|c| c.name == child.name) {
        Some(c) => *c = child,
        None => node.children.push(child),
    }
}

/// A kept array (`len` elements, `default` when missing or shorter).
fn array(node: &Node, name: &str, len: usize, default: i64) -> Vec<i64> {
    let mut v = node.find(name).map(|n| n.ints()).unwrap_or_default();
    v.resize(len, default);
    v.truncate(len);
    v
}

fn u8n(name: &str, v: i64) -> Node {
    Node::value(name, Value::U8(v as u8))
}
fn u16n(name: &str, v: i64) -> Node {
    Node::value(name, Value::U16(v as u16))
}
fn u32n(name: &str, v: i64) -> Node {
    Node::value(name, Value::U32(v as u32))
}

fn path(refid: &str) -> String {
    format!("{DIR}/{refid}/MDX.kbin")
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

fn save(refid: &str, player: &Node) {
    let _ = std::fs::create_dir_all(format!("{DIR}/{refid}"));
    if let Err(e) = std::fs::write(path(refid), player.to_binary()) {
        crate::log!("easrv: MDX {refid}: {e}");
    }
}

/// The player's 8-digit code (its id among the players: rivals are added by it).
fn code_of(refid: &str) -> i64 {
    let h = refid.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    10_000_000 + (h % 90_000_000) as i64
}

/// Every kept player: `(refid, player)`.
fn players() -> Vec<(String, Node)> {
    let Ok(dir) = std::fs::read_dir(DIR) else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let refid = e.file_name().to_string_lossy().into_owned();
            load(&refid).map(|p| (refid, p))
        })
        .collect()
}

/// The player whose code is `code`.
fn by_code(code: i64) -> Option<(String, Node)> {
    players().into_iter().find(|(r, _)| code_of(r) == code)
}

fn profile(player: &Node) -> Node {
    player.find("profile").cloned().unwrap_or_else(|| Node::new("profile"))
}

/// The groove radar's level-ups (`gr1`..`gr5`).
fn radar(name: &str, values: &[i64]) -> Node {
    let mut n = Node::new(name);
    for (i, v) in values.iter().enumerate() {
        n.attrs.push((format!("gr{}", i + 1), v.to_string()));
    }
    n
}

fn titles(p: &Node, out: &mut Node) {
    for name in ["title", "title_gr"] {
        if let Some(t) = p.find(name) {
            out.children.push(t.clone());
        }
    }
}

/// The `load` answer: the player's profile.
fn format_profile(refid: &str, player: &Node) -> Node {
    let p = profile(player);
    let i = |name: &str| attr_i(&p, name, 0);
    let mut g = Node::new("game")
        .child(Node::str("seq", ""))
        .child(u32n("code", code_of(refid)))
        .child(Node::str("name", attr(&p, "name").unwrap_or_default()))
        .child(u8n("area", attr_i(&p, "area", REGION)))
        .child(u32n("cnt_s", i("single_plays")))
        .child(u32n("cnt_d", i("double_plays")))
        .child(u32n("cnt_b", i("battle_plays")));
    for m in 0..6 {
        g = g.child(u32n(&format!("cnt_m{m}"), i(&format!("cnt_m{m}"))));
    }
    g = g
        .child(u32n("exp", i("exp")))
        .child(u32n("exp_o", i("exp_o")))
        .child(u32n("star", i("star")))
        .child(u32n("star_c", i("star_c")))
        .child(u8n("combo", i("combo")))
        .child(u8n("timing_diff", i("early_late")))
        .child(Node::new("chara").attr("my", attr_i(&p, "chara", 30)))
        .child(Node::array("chara_opt", "u16", &array(&p, "chara_opt", 96, 208)));
    if let Some(t) = p.find("title_gr") {
        g.children.push(t.clone());
    }
    if p.attrs.iter().any(|(k, _)| k == "weight") {
        g = g.child(Node::new("workout").attr("weight", i("weight")).attr("day", 0).attr("disp", 1));
    }
    g = g
        .child(Node::new("daycount").attr("playcount", i("today_plays")))
        .child(Node::new("dailycombo").attr("daily_combo", 0).attr("daily_combo_lv", 0));
    let last = p.find("last").cloned().unwrap_or_else(|| Node::new("last"));
    let l = |name: &str, d: i64| attr_i(&last, name, d);
    g = g.child(
        Node::new("last")
            .attr("rival1", l("rival1", -1))
            .attr("rival2", l("rival2", -1))
            .attr("rival3", l("rival3", -1))
            .attr("fri", l("rival1", -1))
            .attr("style", l("style", 0))
            .attr("mode", l("mode", 0))
            .attr("cate", l("cate", 0))
            .attr("sort", l("sort", 0))
            .attr("mid", l("mid", 0))
            .attr("mtype", l("mtype", 0))
            .attr("cid", l("cid", 0))
            .attr("ctype", l("ctype", 0))
            .attr("sid", l("sid", 0)),
    );
    let mut stars = Node::new("result_star");
    for (k, v) in array(&p, "result_stars", 9, 0).iter().enumerate() {
        stars.attrs.push((format!("slot{}", k + 1), v.to_string()));
    }
    g = g
        .child(stars)
        .child(Node::new("target").attr("flag", i("target_flag")).attr("setnum", i("target_setnum")))
        .child(radar("gr_s", &array(&p, "gr_s", 5, 0)))
        .child(radar("gr_d", &array(&p, "gr_d", 5, 0)))
        .child(Node::array("opt", "s16", &array(&p, "opt", 16, 0)))
        .child(Node::array("opt_ex", "s16", &array(&p, "opt_ex", 16, 0)))
        .child(Node::array("flag", "u8", &array(&p, "flag", 256, 1)))
        .child(Node::array("rank", "u16", &array(&p, "rank", 100, 0)));
    // rivals
    for (pos, code) in array(&p, "friends", 10, 0).iter().enumerate() {
        if *code == 0 {
            continue;
        }
        if let Some((fid, f)) = by_code(*code) {
            g.children.push(friend_node(&fid, &f, Node::new("friend").attr("pos", pos).attr("vs", 0).attr("up", 0)));
        }
    }
    let mut area = Node::new("play_area");
    for (k, v) in array(&p, "play_area", 55, 0).iter().enumerate() {
        area.attrs.push((format!("play_cnt{k}"), v.to_string()));
    }
    g.child(area)
}

/// A rival's data in `node` (`friend` in the profile, `game` for `friend`).
fn friend_node(refid: &str, player: &Node, mut node: Node) -> Node {
    let p = profile(player);
    node = node
        .child(u32n("code", code_of(refid)))
        .child(Node::str("name", attr(&p, "name").unwrap_or_default()))
        .child(u8n("area", attr_i(&p, "area", REGION)))
        .child(u32n("exp", attr_i(&p, "exp", 0)))
        .child(u32n("star", attr_i(&p, "star", 0)));
    titles(&p, &mut node);
    node.child(radar("gr_s", &array(&p, "gr_s", 5, 0))).child(radar("gr_d", &array(&p, "gr_d", 5, 0)))
}

/// The `save` request's changes to the profile.
fn unformat_profile(req: &Node, player: &mut Node) {
    let mut p = profile(player);
    let last = req.find("last");
    let mode = last.map(|l| attr_i(l, "mode", 0)).unwrap_or(0);
    let double = last.is_some_and(|l| attr_i(l, "style", 0) == 1);
    let side = if double { "d" } else { "s" };
    let plays = if double { "double_plays" } else { "single_plays" };
    let v = attr_i(&p, plays, 0) + 1;
    set_attr(&mut p, plays, v);
    let v = attr_i(&p, &format!("cnt_m{mode}"), 0) + 1;
    set_attr(&mut p, &format!("cnt_m{mode}"), v);
    let v = attr_i(&p, "today_plays", 0) + 1;
    set_attr(&mut p, "today_plays", v);
    if let Some(gr) = req.find("gr") {
        let values: Vec<i64> = (1..=5).map(|k| attr_i(gr, &format!("gr{k}"), 0)).collect();
        set_child(&mut p, Node::array(&format!("gr_{side}"), "s32", &values));
    }
    if let Some(t) = req.find("title") {
        set_attr(child_mut(&mut p, "title"), side, t.int());
        set_attr(child_mut(&mut p, "title_gr"), side, t.int());
    }
    if let Some(r) = req.find("result_star") {
        let values: Vec<i64> = (1..=9).map(|k| attr_i(r, &format!("slot{k}"), 0)).collect();
        set_child(&mut p, Node::array("result_stars", "s32", &values));
    }
    if let Some(t) = req.find("target") {
        set_attr(&mut p, "target_flag", attr_i(t, "flag", 0));
        set_attr(&mut p, "target_setnum", attr_i(t, "setnum", 0));
    }
    if let Some(l) = last {
        let kept = child_mut(&mut p, "last");
        for k in ["rival1", "rival2", "rival3", "style", "mode", "cate", "sort", "mid", "mtype", "cid", "ctype", "sid"] {
            set_attr(kept, k, attr_i(l, k, 0));
        }
    }
    if let Some(c) = req.find("chara") {
        let v = attr_i(c, "my", 30);
        set_attr(&mut p, "chara", v);
    }
    for (name, kind, len) in [("chara_opt", "u16", 96), ("opt", "s16", 16)] {
        if let Some(n) = req.find(name) {
            let mut v = n.ints();
            v.truncate(len);
            set_child(&mut p, Node::array(name, kind, &v));
        }
    }
    if let Some(e) = req.find("exp") {
        let v = attr_i(&p, "exp", 0) + e.int();
        set_attr(&mut p, "exp", v);
    }
    if let Some(s) = req.find("star") {
        let v = attr_i(&p, "star", 0) + s.int();
        set_attr(&mut p, "star", v);
    }
    if let Some(s) = req.find("star_c") {
        let v = attr_i(&p, "star_c", 0) + s.int();
        set_attr(&mut p, "star_c", v);
    }
    let mut flags = array(&p, "flag", 256, 1);
    for f in req.children.iter().filter(|c| c.name == "flag") {
        let (no, data) = (attr_i(f, "no", -1), attr_i(f, "data", 0));
        if (0..256).contains(&no) {
            flags[no as usize] = data;
        }
    }
    set_child(&mut p, Node::array("flag", "u8", &flags));
    if let Some(w) = req.children.iter().filter(|c| c.name == "weight").last() {
        set_attr(&mut p, "weight", w.int());
    }
    let mut friends = array(&p, "friends", 10, 0);
    for f in req.children.iter().filter(|c| c.name == "friend") {
        let pos = attr_i(f, "pos", -1);
        if (0..10).contains(&pos) {
            friends[pos as usize] = attr_i(f, "code", 0);
        }
    }
    set_child(&mut p, Node::array("friends", "u32", &friends));
    let shop_area = attr_i(req, "shop_area", -1);
    if (0..55).contains(&shop_area) {
        let mut areas = array(&p, "play_area", 55, 0);
        areas[shop_area as usize] += 1;
        set_child(&mut p, Node::array("play_area", "u32", &areas));
    }
    set_child(player, p);
}

/// The combo types, worst to best: none 0, good 4, great 1, perfect 2, marvelous 3.
fn halo_order(halo: i64) -> i64 {
    match halo {
        4 => 1,
        1 => 2,
        2 => 3,
        3 => 4,
        _ => 0,
    }
}

/// `save_m`: the chart's best play and its latest scores kept.
fn save_score(req: &Node, player: &mut Node) {
    let mid = attr_i(req, "mid", -1);
    let chart = attr_i(req, "mtype", -1);
    let Some(data) = req.find("data") else { return };
    if mid < 0 || chart < 0 {
        return;
    }
    let points = attr_i(data, "score", 0);
    let combo = attr_i(data, "combo", 0);
    let rank = attr_i(data, "rank", 7);
    let halo = if points == 1_000_000 {
        3
    } else if attr_i(data, "perf_fc", 0) != 0 {
        2
    } else if attr_i(data, "great_fc", 0) != 0 {
        1
    } else if attr_i(data, "good_fc", 0) != 0 {
        4
    } else {
        0
    };
    let scores = child_mut(player, "scores");
    let i = match scores.children.iter().position(|s| attr_i(s, "mid", -1) == mid && attr_i(s, "chart", -1) == chart) {
        Some(i) => i,
        None => {
            scores.children.push(Node::new("s").attr("mid", mid).attr("chart", chart));
            scores.children.len() - 1
        }
    };
    let s = &mut scores.children[i];
    let best = attr_i(s, "score", -1);
    let high = points > best;
    set_attr(s, "score", points.max(best));
    let v = attr_i(s, "count", 0) + 1;
    set_attr(s, "count", v);
    // rank 1 (AAA) is the best
    let old_rank = attr_i(s, "rank", 7);
    set_attr(s, "rank", if best < 0 { rank } else { rank.min(old_rank) });
    let old_halo = attr_i(s, "halo", 0);
    set_attr(s, "halo", if halo_order(halo) > halo_order(old_halo) { halo } else { old_halo });
    let v = combo.max(attr_i(s, "combo", 0));
    set_attr(s, "combo", v);
    let mut recent = array(s, "recent", 0, 0);
    recent = s.find("recent").map(|n| n.ints()).unwrap_or(recent);
    recent.insert(0, points);
    recent.truncate(5);
    set_child(s, Node::array("recent", "u32", &recent));
    if high && let Some(trace) = req.find("trace") {
        let mut t = trace.clone();
        t.name = "trace".into();
        set_child(s, t);
    }
}

/// The best play of every chart among `list`: `(mid, chart, refid, score node)`.
fn hiscores(list: &[(String, Node)]) -> Vec<(i64, i64, String, Node)> {
    let mut best: Vec<(i64, i64, String, Node)> = Vec::new();
    for (refid, player) in list {
        for s in player.find("scores").map(|n| n.children.clone()).unwrap_or_default() {
            let (mid, chart) = (attr_i(&s, "mid", -1), attr_i(&s, "chart", -1));
            match best.iter_mut().find(|b| b.0 == mid && b.1 == chart) {
                Some(b) if attr_i(&b.3, "score", 0) >= attr_i(&s, "score", 0) => {}
                Some(b) => *b = (mid, chart, refid.clone(), s),
                None => best.push((mid, chart, refid.clone(), s)),
            }
        }
    }
    best
}

fn hiscore_answer(list: &[(String, Node)]) -> Node {
    let mut game = Node::new("game");
    for (mid, chart, refid, s) in hiscores(list) {
        let p = list.iter().find(|(r, _)| *r == refid).map(|(_, p)| profile(p)).unwrap_or_else(|| Node::new("profile"));
        let ty = Node::new("type")
            .attr("diff", chart)
            .child(Node::str("name", attr(&p, "name").unwrap_or_default()))
            .child(u32n("score", attr_i(&s, "score", 0)))
            .child(u16n("area", attr_i(&p, "area", REGION)))
            .child(u8n("rank", attr_i(&s, "rank", 7)))
            .child(u8n("combo_type", attr_i(&s, "halo", 0)))
            .child(u32n("code", code_of(&refid)));
        let i = match game.children.iter().position(|m| attr_i(m, "reclink_num", -1) == mid) {
            Some(i) => i,
            None => {
                game.children.push(Node::new("music").attr("reclink_num", mid));
                game.children.len() - 1
            }
        };
        game.children[i].children.push(ty);
    }
    game
}

/// The `<response>` child of an `MDX` request, `None`: not one of the game's own.
pub fn handle(module: &str, method: &str, req: Option<&Node>) -> Option<Node> {
    if module != "game" {
        return None;
    }
    let empty = Node::new("game");
    let g = req.and_then(|r| r.find("game")).cloned().unwrap_or_else(|| empty.clone());
    let refid = attr(&g, "refid").unwrap_or_default().to_string();
    let code = attr(&g, "code").and_then(|c| c.trim().parse::<i64>().ok());
    Some(match method {
        "common" => {
            let mut game = Node::new("game");
            for id in 0..256 {
                game.children.push(
                    Node::new("flag").attr("id", id).attr("t", 0).attr("s1", 0).attr("s2", 0).attr("area", REGION).attr("is_final", 1),
                );
            }
            game.child(Node::array("cnt_music", "u32", &[0; MAX_SONGS]))
        }
        "shop" => empty.attr("stop", 0),
        "lock" => empty.attr("now_login", 0),
        "tax_info" => empty.child(Node::new("tax_info").attr("tax_phase", 0)),
        "new" => {
            if valid_id(&refid) && load(&refid).is_none() {
                let p = Node::new("profile").attr("name", attr(&g, "name").unwrap_or_default().trim()).attr("area", attr_i(&g, "area", REGION));
                save(&refid, &Node::new("player").child(p));
            }
            empty
        }
        "load" => match load(&refid) {
            Some(player) => format_profile(&refid, &player),
            None => empty.attr("none", 0),
        },
        "save" => {
            if let Some(mut player) = load(&refid) {
                unformat_profile(&g, &mut player);
                save(&refid, &player);
            }
            empty
        }
        "load_m" => {
            let player = match code {
                Some(c) => by_code(c).map(|(_, p)| p),
                None => load(&refid),
            };
            let mut game = Node::new("game");
            for s in player.and_then(|p| p.find("scores").cloned()).map(|n| n.children).unwrap_or_default() {
                let mid = attr_i(&s, "mid", -1);
                let ty = Node::new("type")
                    .attr("diff", attr_i(&s, "chart", 0))
                    .child(u32n("score", attr_i(&s, "score", 0)))
                    .child(u16n("count", attr_i(&s, "count", 0)))
                    .child(u8n("rank", attr_i(&s, "rank", 7)))
                    .child(u8n("combo_type", attr_i(&s, "halo", 0)));
                let i = match game.children.iter().position(|m| attr_i(m, "reclink", -1) == mid) {
                    Some(i) => i,
                    None => {
                        game.children.push(Node::new("music").attr("reclink", mid));
                        game.children.len() - 1
                    }
                };
                game.children[i].children.push(ty);
            }
            game
        }
        "save_m" => {
            if let Some(mut player) = load(&refid) {
                save_score(&g, &mut player);
                save(&refid, &player);
            }
            empty
        }
        "score" => {
            let (mid, chart) = (attr_i(&g, "mid", -1), attr_i(&g, "type", -1));
            let recent = load(&refid)
                .and_then(|p| p.find("scores").cloned())
                .and_then(|s| s.children.into_iter().find(|s| attr_i(s, "mid", -1) == mid && attr_i(s, "chart", -1) == chart))
                .and_then(|s| s.find("recent").map(|n| n.ints()))
                .unwrap_or_default();
            let mut game = empty;
            for k in 0..5 {
                game = game.attr(&format!("sc{}", k + 1), recent.get(k).copied().unwrap_or(0));
            }
            game
        }
        "trace" => {
            let chart = attr_i(&g, "type", -1);
            let trace = code.and_then(by_code).and_then(|(_, p)| {
                if let Some(mid) = attr(&g, "mid").and_then(|m| m.parse::<i64>().ok()) {
                    p.find("scores")?.children.iter().find(|s| attr_i(s, "mid", -1) == mid && attr_i(s, "chart", -1) == chart)?.find("trace").cloned()
                } else {
                    let cid = attr_i(&g, "cid", -1);
                    p.find("courses")?.children.iter().find(|c| attr_i(c, "id", -1) == cid * 4 + chart)?.find("trace").cloned()
                }
            });
            match trace {
                Some(t) => {
                    let steps = t.ints();
                    empty.child(u32n("size", steps.len() as i64)).child(Node::array("trace", "u8", &steps))
                }
                None => empty,
            }
        }
        "hiscore" => hiscore_answer(&players()),
        "area_hiscore" => {
            let area = attr_i(&g, "shop_area", REGION);
            let list: Vec<_> = players().into_iter().filter(|(_, p)| attr_i(&profile(p), "area", REGION) == area).collect();
            hiscore_answer(&list)
        }
        "friend" => match code.and_then(by_code) {
            Some((fid, f)) => friend_node(&fid, &f, empty.attr("data", 1)),
            None => empty,
        },
        "load_daily" => {
            let player = match code {
                Some(c) => by_code(c).map(|(_, p)| p),
                None => load(&refid),
            };
            match player {
                Some(p) => empty
                    .child(Node::new("daycount").attr("playcount", attr_i(&profile(&p), "today_plays", 0)))
                    .child(Node::new("dailycombo").attr("daily_combo", 0).attr("daily_combo_lv", 0)),
                None => empty,
            }
        }
        "load_c" => {
            let player = match code {
                Some(c) => by_code(c).map(|(_, p)| p),
                None => load(&refid),
            };
            let mut course = vec![0i64; 3200];
            for c in player.and_then(|p| p.find("courses").cloned()).map(|n| n.children).unwrap_or_default() {
                let index = attr_i(&c, "id", -1) * 8;
                if (0..=3200 - 8).contains(&index) {
                    let i = index as usize;
                    let score = attr_i(&c, "score", 0);
                    course[i] = score / 10000;
                    course[i + 1] = score % 10000;
                    course[i + 2] = attr_i(&c, "combo", 0);
                    course[i + 3] = attr_i(&c, "rank", 7);
                    course[i + 5] = attr_i(&c, "stage", 0);
                    course[i + 6] = attr_i(&c, "combo_type", 0);
                }
            }
            empty.child(Node::array("course", "u16", &course))
        }
        "save_c" => {
            if let (Some(mut player), Some(data)) = (load(&refid), g.find("data")) {
                let id = attr_i(&g, "cid", 0) * 4 + attr_i(&g, "ctype", 0);
                let courses = child_mut(&mut player, "courses");
                let i = match courses.children.iter().position(|c| attr_i(c, "id", -1) == id) {
                    Some(i) => i,
                    None => {
                        courses.children.push(Node::new("c").attr("id", id));
                        courses.children.len() - 1
                    }
                };
                let c = &mut courses.children[i];
                let points = attr_i(data, "score", 0);
                let high = points > attr_i(c, "score", -1);
                for k in ["score", "combo", "stage", "combo_type"] {
                    let v = attr_i(data, k, 0).max(attr_i(c, k, 0));
                    set_attr(c, k, v);
                }
                let rank = attr_i(data, "rank", 7);
                let v = rank.min(attr_i(c, "rank", 7));
                set_attr(c, "rank", v);
                if high && let Some(trace) = g.find("trace") {
                    let mut t = trace.clone();
                    t.name = "trace".into();
                    set_child(c, t);
                }
                save(&refid, &player);
            }
            empty
        }
        // log, message, ranking, recorder, old (no previous version's player)
        _ => empty,
    })
}
