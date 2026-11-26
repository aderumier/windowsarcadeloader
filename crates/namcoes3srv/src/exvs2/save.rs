//! The battle results (`SaveVsmResult`: versus, the cabinet's offline battles; `SaveVscResult`:
//! the Triad CPU battles) written to the player, as EXVS2-POC's save commands do
//! (`Command/SaveBattle`): GP, echelon, navigators' familiarity, the mobile suit's use count,
//! the tag team's skill points, then the win / loss record (versus) or the Triad data (score,
//! wanted, ribbons, courses).

use super::{Player, Server, add, flag, find, ints, list, message, now, set, signed, store};
use crate::graphene::{Obj, V, string, uint};
use crate::log;

/// The last echelon, the two special echelons (captain, brigadier) and the general's.
const MAX_ECHELON: u64 = 55;
const S_CAPTAIN_ECHELON: u64 = 23;
const S_BRIGADIER_ECHELON: u64 = 38;
const GENERAL_ECHELON: u64 = 54;

pub(super) fn battle(server: &Server, triad: bool, body: &Obj) {
    let (session, pilot_id) = (string(body, "session_id"), uint(body, "pilot_id"));
    let Some(mut player) = find(server, |p| p.session == session && p.id as u64 == pilot_id) else {
        log(&format!("exvs2: battle result of an unknown session (pilot {pilot_id})"));
        return;
    };
    let result = message(body, "result");
    let gp = uint(&result, "gp") as i64;
    add(&mut player.user, "gp", gp);
    echelon(&mut player, &result);
    navigators(&mut player, &result);
    mastery(&mut player, uint(&result, "skill_point_mobile_suit_id"));
    team(&mut player, uint(&result, "tag_team_id"), uint(&result, "tag_skill_point"));
    let win = flag(&result, "win_flag");
    if triad {
        triad_info(&mut player, &result, win);
    } else {
        let shuffle = flag(body, "shuffle_flag");
        let (w, l) = if win { (1, 0) } else { (0, 1) };
        add(&mut player.record, "total_win", w);
        add(&mut player.record, "total_lose", l);
        let (wins, loses) = if shuffle { ("shuffle_win", "shuffle_lose") } else { ("team_win", "team_lose") };
        add(&mut player.record, wins, w);
        add(&mut player.record, loses, l);
    }
    player.last_played = now();
    store(server, &player);
    log(&format!(
        "exvs2: pilot {} {} {}: GP +{gp}, echelon {} ({} exp)",
        player.id,
        if triad { "Triad" } else { "versus" },
        if win { "won" } else { "lost" },
        uint(&player.record, "echelon_id"),
        signed(&player.record, "echelon_exp")
    ));
}

/// The echelon's experience: up to the next echelon at the echelon's width, down to the previous
/// one at its downgrade threshold (EXVS2-POC downgrades above the threshold instead: its test is
/// inverted), the special echelons' missions.
fn echelon(player: &mut Player, result: &Obj) {
    let exp = signed(result, "echelon_exp");
    if exp == 0 {
        return;
    }
    let record = &mut player.record;
    set(record, "s_echelon_flag", V::U(flag(result, "s_echelon_flag") as u64));
    let id = uint(record, "echelon_id");
    if id == MAX_ECHELON {
        return;
    }
    let Some(current) = super::data::echelon(id) else { return };
    if flag(record, "s_echelon_mission_flag") {
        let progress = uint(result, "s_echelon_progress");
        set(record, "s_echelon_progress", V::U(progress));
        if progress < 3 {
            return;
        }
        set(record, "s_echelon_mission_flag", V::U(0));
        let special = match id {
            S_CAPTAIN_ECHELON => Some("s_captain_flag"),
            S_BRIGADIER_ECHELON => Some("s_brigadier_flag"),
            _ => None,
        };
        if let Some(special) = special {
            set(record, "s_echelon_progress", V::U(0));
            set(record, "s_echelon_flag", V::U(1));
            set(record, special, V::U(1));
        }
        return;
    }
    let total = signed(record, "echelon_exp") + exp;
    let (width, threshold) = (current[9] as i64, current[10] as i64);
    let (id, total) = if total >= width {
        if id == GENERAL_ECHELON {
            (id, 500)
        } else {
            let up = super::data::echelon(id + 1).map_or(0, |e| e[6] as i64);
            (id + 1, up)
        }
    } else if threshold == 0 {
        (id, total.max(0))
    } else if total <= threshold {
        let down = super::data::echelon(id.saturating_sub(1)).map_or(0, |e| e[5] as i64);
        (id.saturating_sub(1), down)
    } else {
        (id, total)
    };
    set(record, "echelon_id", V::U(id));
    set(record, "echelon_exp", V::I(total));
}

/// The navigators' familiarity: the guest navigator's and the battle navigator's (both to the
/// guest navigator when it is the same one), for the navigators the player has.
fn navigators(player: &mut Player, result: &Obj) {
    let (guest, battle) = (uint(result, "guest_nav_id"), uint(result, "battle_nav_id"));
    let (guest_inc, battle_inc) = (uint(result, "guest_nav_familiarity"), uint(result, "battle_nav_familiarity"));
    let mut navs = list(&player.user, "guest_navs");
    if navs.is_empty() {
        return;
    }
    for nav in navs.iter_mut() {
        let id = uint(nav, "guest_nav_id");
        let inc = if guest == battle {
            if id == guest { guest_inc + battle_inc } else { 0 }
        } else if id == guest {
            guest_inc
        } else if id == battle {
            battle_inc
        } else {
            0
        };
        add(nav, "guest_nav_familiarity", inc as i64);
    }
    set(&mut player.user, "guest_navs", V::List(navs));
}

/// The mobile suit's use count (the pilot data's mobile suits).
fn mastery(player: &mut Player, ms: u64) {
    let mut skills = list(&player.pilot, "ms_skills");
    match skills.iter_mut().find(|m| uint(m, "mst_mobile_suit_id") == ms) {
        Some(m) => add(m, "ms_used_num", 1),
        None => skills.push(vec![("mst_mobile_suit_id", V::U(ms)), ("ms_used_num", V::U(1)), ("costume_id", V::U(0))]),
    }
    set(&mut player.pilot, "ms_skills", V::List(skills));
}

/// The player's tag team's skill points.
fn team(player: &mut Player, team: u64, points: u64) {
    let mut teams = list(&player.pilot, "tag_teams");
    if let Some(t) = teams.iter_mut().find(|t| uint(t, "id") == team) {
        add(t, "skill_point", points as i64);
        set(&mut player.pilot, "tag_teams", V::List(teams));
    }
}

/// Triad: the total score, scenes played, wanted defeated (won), the ribbons and courses
/// released, the course's play / clear counts and high score (the course's score: EXVS2-POC
/// starts a new course's high score with the scene's).
fn triad_info(player: &mut Player, result: &Obj, win: bool) {
    let pilot = &mut player.pilot;
    add(pilot, "total_triad_score", uint(result, "scene_score") as i64);
    add(pilot, "total_triad_scene_play_num", 1);
    if win {
        add(pilot, "total_triad_wanted_defeat_num", uint(result, "total_wanted_defeat_num") as i64);
    }
    let mut ribbons = ints(pilot, "cpu_ribbons");
    for r in ints(result, "released_ribbon_id") {
        if !ribbons.contains(&r) {
            ribbons.push(r);
        }
    }
    set(pilot, "cpu_ribbons", V::Ints(ribbons));
    let mut scenes = list(pilot, "cpu_scenes");
    let new_course = |id: u64| vec![("course_id", V::U(id)), ("released_at", V::U(now())), ("total_play_num", V::U(0)), ("total_clear_num", V::U(0)), ("highscore", V::U(0))];
    for id in ints(result, "released_course_id") {
        if !scenes.iter().any(|s| uint(s, "course_id") == id) {
            scenes.push(new_course(id));
        }
    }
    let course = uint(result, "course_id");
    if !scenes.iter().any(|s| uint(s, "course_id") == course) {
        scenes.push(new_course(course));
    }
    let clear = flag(result, "course_clear_flag");
    let score = uint(result, "course_score");
    if let Some(s) = scenes.iter_mut().find(|s| uint(s, "course_id") == course) {
        if score >= uint(s, "highscore") {
            set(s, "highscore", V::U(score));
        }
        if clear {
            add(s, "total_play_num", 1);
            if win {
                add(s, "total_clear_num", 1);
            }
        }
    }
    set(pilot, "cpu_scenes", V::List(scenes));
}
