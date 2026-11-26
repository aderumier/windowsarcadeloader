//! The game data (`LoadGameData`): every mobile suit and navigator released, the ranks
//! (echelons) and their matching tables, the versus rules, the Triad CPU courses. The values are
//! EXVS2-POC's (`Command/LoadGameData`, the echelon settings of its database migrations).

use crate::graphene::{Obj, V};

/// The echelons: id, online match rank S / T / E, mobile suit adjustment, down / up default
/// experience, win / lose correction rates, experience width, downgrade threshold.
pub(crate) const ECHELONS: &[[i32; 11]] = &[
    [0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0],
    [1, 0, 0, 0, 1, 0, 0, 0, 0, 99, 0],
    [2, 0, 0, 0, 1, 0, 0, 0, 0, 100, 0],
    [3, 1, 1, 1, 1, 0, 0, 0, 0, 150, 0],
    [4, 1, 1, 1, 1, 0, 0, 0, 0, 300, 0],
    [5, 2, 1, 2, 1, 0, 0, 0, 0, 300, 0],
    [6, 2, 2, 2, 1, 0, 0, 0, 0, 500, 0],
    [7, 2, 2, 2, 1, 0, 0, 0, 0, 750, 0],
    [8, 3, 2, 2, 1, 250, 0, 80, 90, 500, 0],
    [9, 3, 3, 3, 1, 250, 0, 80, 90, 500, -600],
    [10, 4, 3, 4, 1, 250, 0, 80, 90, 500, -400],
    [11, 4, 3, 4, 1, 250, 0, 80, 90, 500, -400],
    [12, 4, 3, 5, 1, 250, 0, 80, 90, 500, -400],
    [13, 5, 3, 5, 1, 500, 0, 80, 90, 800, -550],
    [14, 6, 4, 6, 1, 700, 0, 80, 90, 1000, -400],
    [15, 7, 4, 6, 1, 700, 0, 80, 90, 1000, -400],
    [16, 7, 4, 6, 1, 700, 0, 80, 90, 1000, -400],
    [17, 7, 4, 6, 1, 700, 0, 80, 90, 1000, -400],
    [18, 7, 4, 6, 1, 1200, 0, 80, 90, 1500, -500],
    [19, 8, 5, 7, 0, 1200, 0, 80, 90, 1500, -400],
    [20, 9, 5, 7, 0, 1200, 0, 80, 90, 1500, -400],
    [21, 9, 5, 7, 0, 1200, 0, 80, 90, 1500, -400],
    [22, 9, 5, 7, 0, 1200, 0, 80, 90, 1500, -400],
    [23, 10, 5, 7, 0, 1500, 0, 80, 90, 2000, -500],
    [24, 11, 6, 7, 0, 1000, 0, 90, 95, 2000, -600],
    [25, 12, 6, 7, 0, 1000, 0, 90, 95, 2500, -400],
    [26, 12, 6, 7, 0, 1000, 0, 90, 95, 2500, -400],
    [27, 12, 6, 7, 0, 1000, 0, 90, 95, 2500, -400],
    [28, 12, 6, 7, 0, 4000, 0, 90, 95, 5000, -500],
    [29, 13, 7, 7, 0, 4000, 0, 90, 95, 5500, -500],
    [30, 13, 7, 7, 0, 4000, 0, 90, 95, 5500, -500],
    [31, 13, 7, 7, 0, 4000, 0, 90, 95, 5500, -500],
    [32, 13, 7, 7, 0, 4000, 0, 90, 95, 5500, -500],
    [33, 13, 7, 7, 0, 6000, 0, 90, 95, 7500, -550],
    [34, 13, 7, 7, 0, 7000, 0, 90, 95, 8500, -500],
    [35, 13, 7, 7, 0, 7000, 0, 90, 95, 8500, -500],
    [36, 13, 7, 7, 0, 7000, 0, 90, 95, 8500, -500],
    [37, 13, 7, 7, 0, 7000, 0, 90, 95, 8500, -500],
    [38, 13, 7, 7, 0, 10000, 0, 90, 95, 11500, -550],
    [39, 13, 7, 7, 0, 8000, 0, 100, 100, 11000, -500],
    [40, 13, 7, 7, 0, 8000, 0, 100, 100, 11000, -500],
    [41, 13, 7, 7, 0, 8000, 0, 100, 100, 11000, -500],
    [42, 13, 7, 7, 0, 8000, 0, 100, 100, 11000, -500],
    [43, 13, 7, 7, 0, 16000, 0, 100, 100, 19000, -550],
    [44, 13, 7, 7, 0, 13000, 0, 100, 100, 16000, -450],
    [45, 13, 7, 7, 0, 13000, 0, 100, 100, 16000, -450],
    [46, 13, 7, 7, 0, 13000, 0, 100, 100, 16000, -450],
    [47, 13, 7, 7, 0, 13000, 0, 100, 100, 16000, -450],
    [48, 13, 7, 7, 0, 18000, 0, 100, 100, 21000, -550],
    [49, 13, 7, 7, 0, 18000, 0, 100, 100, 21000, -400],
    [50, 13, 7, 7, 0, 18000, 0, 100, 100, 21000, -400],
    [51, 13, 7, 7, 0, 18000, 0, 100, 100, 21000, -400],
    [52, 13, 7, 7, 0, 18000, 0, 100, 100, 21000, -400],
    [53, 13, 7, 7, 0, 26000, 0, 100, 100, 29000, -500],
    [54, 13, 7, 7, 0, 0, 0, 100, 100, 500, -400],
    [55, 13, 7, 7, 0, 0, 0, 100, 100, 0, 0],
];

/// An echelon's settings.
pub(crate) fn echelon(id: u64) -> Option<&'static [i32; 11]> {
    ECHELONS.iter().find(|e| e[0] as u64 == id)
}

/// The solo (shuffle) matching table: online match rank, try, sequence, make a room.
const MATCHING_S: &[[u32; 4]] = &[
    [0, 1, 1, 1],
    [1, 1, 1, 0],
    [1, 2, 3, 1],
    [2, 1, 1, 0],
    [2, 2, 3, 1],
    [3, 1, 1, 0],
    [3, 2, 2, 1],
    [4, 1, 1, 0],
    [4, 2, 3, 1],
    [5, 1, 1, 0],
    [5, 2, 2, 1],
    [9, 1, 1, 0],
    [9, 2, 3, 1],
    [13, 1, 1, 0],
    [13, 2, 2, 1],
];

/// The team matching table: the first try alone, then a room after sequence 3 (ranks 0..3 and
/// 6) or 4 (4, 5, 7).
const MATCHING_T: &[[u32; 4]] = &[
    [0, 1, 1, 0],
    [0, 2, 3, 1],
    [1, 1, 1, 0],
    [1, 2, 3, 1],
    [2, 1, 1, 0],
    [2, 2, 3, 1],
    [3, 1, 1, 0],
    [3, 2, 3, 1],
    [4, 1, 1, 0],
    [4, 2, 4, 1],
    [5, 1, 1, 0],
    [5, 2, 4, 1],
    [6, 1, 1, 0],
    [6, 2, 3, 1],
    [7, 1, 1, 0],
    [7, 2, 4, 1],
];

/// The versus adjustments (solo and team alike): rank, max, min, num.
const VS_ADJUST: &[[i32; 4]] = &[
    [1, 60, 60, 5],
    [2, 59, 55, 3],
    [3, 54, 31, 2],
    [4, 30, 21, 1],
    [5, 20, -3, 0],
    [6, -4, -10, -1],
    [7, -11, -20, -3],
    [8, -21, -59, -5],
    [9, -60, -60, -10],
];

/// The Triad CPU courses open (since 2017-12-31).
const CPU_COURSES: &[(u32, u32)] = &[(1, 13), (16, 28), (31, 41), (46, 54), (100, 103)];
const COURSES_OPENED_AT: u64 = 1514739500;

fn matching(table: &[[u32; 4]]) -> V {
    V::List(
        table
            .iter()
            .map(|r| {
                vec![
                    ("online_match_rank", V::U(r[0] as u64)),
                    ("matching_try_num", V::U(r[1] as u64)),
                    ("seq_num", V::U(r[2] as u64)),
                    ("make_room_flag", V::U(r[3] as u64)),
                ]
            })
            .collect(),
    )
}

fn vs_adjust() -> V {
    V::List(
        VS_ADJUST
            .iter()
            .map(|r| vec![("rank_id", V::U(r[0] as u64)), ("max", V::I(r[1] as i64)), ("min", V::I(r[2] as i64)), ("num", V::I(r[3] as i64))])
            .collect(),
    )
}

pub(crate) fn load_game_data() -> Obj {
    let all_ms: Vec<u64> = (1..=294).collect();
    let echelons = ECHELONS
        .iter()
        .map(|e| {
            vec![
                ("echelon_id", V::U(e[0] as u64)),
                ("online_match_rank_s", V::U(e[1] as u64)),
                ("online_match_rank_t", V::U(e[2] as u64)),
                ("online_match_rank_e", V::U(e[3] as u64)),
                ("ms_adjust_flag", V::U(e[4] as u64)),
                ("down_default_exp", V::I(e[5] as i64)),
                ("up_default_exp", V::I(e[6] as i64)),
                ("win_correction_rate", V::U(e[7] as u64)),
                ("lose_correction_rate", V::U(e[8] as u64)),
            ]
        })
        .collect();
    let courses = CPU_COURSES
        .iter()
        .flat_map(|&(from, to)| from..to)
        .map(|id| vec![("course_id", V::U(id as u64)), ("opened_at", V::U(COURSES_OPENED_AT))])
        .collect();
    let ex_matching = (1..=5u64).map(|i| vec![("matching_try_num", V::U(i)), ("seq_num", V::U(2000 + i)), ("make_room_flag", V::U((i == 5) as u64))]).collect();
    vec![
        ("release_ms_id", V::Ints(all_ms.clone())),
        ("displayable_ms_id", V::Ints(all_ms)),
        ("release_guest_nav_id", V::Ints((0..43).collect())),
        ("echelon_tables", V::List(echelons)),
        ("team_echelon_coefficient", V::U(0)),
        ("echelon_matching_tables_s", matching(MATCHING_S)),
        ("echelon_matching_tables_t", matching(MATCHING_T)),
        ("vs_adjust_s", vs_adjust()),
        ("vs_adjust_t", vs_adjust()),
        (
            "on_vs_info",
            V::M(vec![("rule_time_limit_team", V::U(210)), ("rule_time_limit_shuffle", V::U(210)), ("rule_damage_level_team", V::U(2)), ("rule_damage_level_shuffle", V::U(2))]),
        ),
        // the replays from every echelon (the network's: 9, advanced 19)
        ("replay_under_echelon_id", V::U(0)),
        ("advanced_replay_under_echelon_id", V::U(0)),
        ("release_cpu_course", V::List(courses)),
        ("release_cpu_scene", V::Ints(vec![1, 2, 3, 4, 5])),
        ("weekly_rank_info", V::M(vec![("weekly_rank_no", V::U(0)), ("weekly_rank_course_id", V::U(1))])),
        ("ex_matching_tables", V::List(ex_matching)),
    ]
}
