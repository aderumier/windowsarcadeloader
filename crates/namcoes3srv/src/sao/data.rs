//! ARTEMiS' SAO data (`titles/sao/data/1/*.csv`, WTFPL), built into the server: the master data
//! the game downloads at boot (`master.rs`) and the static tables its handlers look up (ARTEMiS'
//! `read.py` loads the same files into its `sao_static_*` tables).
//!
//! A cell is typed as ARTEMiS' `load_data_csv` types it: digits an integer, `true` / `false` a
//! boolean, `yyyy/mm/dd[ h:mm:ss]` a date, anything else a string; each encoder then converts it
//! as Python does (`str()` of an integer, `int()` of a string...).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::value::V;

static FILES: &[(&str, &str)] = &[
    ("AcLoginBonuses", include_str!("data/AcLoginBonuses.csv")),
    ("AdBanners", include_str!("data/AdBanners.csv")),
    ("AppointLeaderEffect", include_str!("data/AppointLeaderEffect.csv")),
    ("AppointLeaderEffectType", include_str!("data/AppointLeaderEffectType.csv")),
    ("AppointLeaderParam", include_str!("data/AppointLeaderParam.csv")),
    ("Awakening", include_str!("data/Awakening.csv")),
    ("BattleCamera", include_str!("data/BattleCamera.csv")),
    ("BeginnerMissionConditions", include_str!("data/BeginnerMissionConditions.csv")),
    ("BeginnerMissionRewards", include_str!("data/BeginnerMissionRewards.csv")),
    ("BeginnerMissions", include_str!("data/BeginnerMissions.csv")),
    ("BeginnerMissionSeatConditions", include_str!("data/BeginnerMissionSeatConditions.csv")),
    ("BeginnerMissionSeatRewards", include_str!("data/BeginnerMissionSeatRewards.csv")),
    ("BnidSerialCodeRewards", include_str!("data/BnidSerialCodeRewards.csv")),
    ("BnidSerialCodes", include_str!("data/BnidSerialCodes.csv")),
    ("BossAttack", include_str!("data/BossAttack.csv")),
    ("BuffItem", include_str!("data/BuffItem.csv")),
    ("BuyTicket", include_str!("data/BuyTicket.csv")),
    ("Cap", include_str!("data/Cap.csv")),
    ("CharaAttack", include_str!("data/CharaAttack.csv")),
    ("CharaComment", include_str!("data/CharaComment.csv")),
    ("Chara", include_str!("data/Chara.csv")),
    ("CharaFriendlyRank", include_str!("data/CharaFriendlyRank.csv")),
    ("ChatEventStory", include_str!("data/ChatEventStory.csv")),
    ("ChatMainStory", include_str!("data/ChatMainStory.csv")),
    ("ChatSideStory", include_str!("data/ChatSideStory.csv")),
    ("ComebackEvent", include_str!("data/ComebackEvent.csv")),
    ("CompositionEvent", include_str!("data/CompositionEvent.csv")),
    ("CompositionParam", include_str!("data/CompositionParam.csv")),
    ("CostumeType", include_str!("data/CostumeType.csv")),
    ("DebugValidator", include_str!("data/DebugValidator.csv")),
    ("DefragMatchAIPattern", include_str!("data/DefragMatchAIPattern.csv")),
    ("DefragMatchBossTable", include_str!("data/DefragMatchBossTable.csv")),
    ("DefragMatchChat", include_str!("data/DefragMatchChat.csv")),
    ("DefragMatchCpuSupportLogs", include_str!("data/DefragMatchCpuSupportLogs.csv")),
    ("DefragMatchCpuUnits", include_str!("data/DefragMatchCpuUnits.csv")),
    ("DefragMatchGrade", include_str!("data/DefragMatchGrade.csv")),
    ("DefragMatchPeriodBonuses", include_str!("data/DefragMatchPeriodBonuses.csv")),
    ("DefragMatchQuest", include_str!("data/DefragMatchQuest.csv")),
    ("DefragMatchRandomBonusConditions", include_str!("data/DefragMatchRandomBonusConditions.csv")),
    ("DefragMatchRandomBonusTables", include_str!("data/DefragMatchRandomBonusTables.csv")),
    ("DefragMatchRareDrops", include_str!("data/DefragMatchRareDrops.csv")),
    ("DefragMatchs", include_str!("data/DefragMatchs.csv")),
    ("DefragMatchSeed", include_str!("data/DefragMatchSeed.csv")),
    ("DefragMatchSpecialEffects", include_str!("data/DefragMatchSpecialEffects.csv")),
    ("Enemy", include_str!("data/Enemy.csv")),
    ("EnemyKind", include_str!("data/EnemyKind.csv")),
    ("EnemySet", include_str!("data/EnemySet.csv")),
    ("EpisodeAppends", include_str!("data/EpisodeAppends.csv")),
    ("EpisodeChapter", include_str!("data/EpisodeChapter.csv")),
    ("Episode", include_str!("data/Episode.csv")),
    ("EpisodePart", include_str!("data/EpisodePart.csv")),
    ("Equipment", include_str!("data/Equipment.csv")),
    ("EquipmentLevel", include_str!("data/EquipmentLevel.csv")),
    ("Event", include_str!("data/Event.csv")),
    ("EventItems", include_str!("data/EventItems.csv")),
    ("EventMonsters", include_str!("data/EventMonsters.csv")),
    ("EventScenes", include_str!("data/EventScenes.csv")),
    ("ExBonusCondition", include_str!("data/ExBonusCondition.csv")),
    ("ExBonusTable", include_str!("data/ExBonusTable.csv")),
    ("ExTowerQuests", include_str!("data/ExTowerQuests.csv")),
    ("ExTowers", include_str!("data/ExTowers.csv")),
    ("FirstTicketPurchaseCampaigns", include_str!("data/FirstTicketPurchaseCampaigns.csv")),
    ("FixedFormChat", include_str!("data/FixedFormChat.csv")),
    ("FixedFormChatText", include_str!("data/FixedFormChatText.csv")),
    ("Fragment", include_str!("data/Fragment.csv")),
    ("GamePlayPrice", include_str!("data/GamePlayPrice.csv")),
    ("GashaFreeCampaigns", include_str!("data/GashaFreeCampaigns.csv")),
    ("GashaMedalBonuses", include_str!("data/GashaMedalBonuses.csv")),
    ("GashaMedals", include_str!("data/GashaMedals.csv")),
    ("GashaMedalSettings", include_str!("data/GashaMedalSettings.csv")),
    ("GashaMedalShopItems", include_str!("data/GashaMedalShopItems.csv")),
    ("GashaMedalShops", include_str!("data/GashaMedalShops.csv")),
    ("GashaMedalTypes", include_str!("data/GashaMedalTypes.csv")),
    ("GenericCampaignPeriods", include_str!("data/GenericCampaignPeriods.csv")),
    ("GimmickAttack", include_str!("data/GimmickAttack.csv")),
    ("HeroLog", include_str!("data/HeroLog.csv")),
    ("HeroLogLevel", include_str!("data/HeroLogLevel.csv")),
    ("HeroLogRole", include_str!("data/HeroLogRole.csv")),
    ("HeroLogTrustRank", include_str!("data/HeroLogTrustRank.csv")),
    ("Item", include_str!("data/Item.csv")),
    ("ItemType", include_str!("data/ItemType.csv")),
    ("LinkedSiteRegCampaignRewards", include_str!("data/LinkedSiteRegCampaignRewards.csv")),
    ("LinkedSiteRegCampaigns", include_str!("data/LinkedSiteRegCampaigns.csv")),
    ("MenuDisplayEnemy", include_str!("data/MenuDisplayEnemy.csv")),
    ("MissionClearText", include_str!("data/MissionClearText.csv")),
    ("Mission", include_str!("data/Mission.csv")),
    ("MissionDifficulty", include_str!("data/MissionDifficulty.csv")),
    ("MissionTable", include_str!("data/MissionTable.csv")),
    ("MonsterAction", include_str!("data/MonsterAction.csv")),
    ("MonsterAttack", include_str!("data/MonsterAttack.csv")),
    ("NavigatorChara", include_str!("data/NavigatorChara.csv")),
    ("NavigatorComment", include_str!("data/NavigatorComment.csv")),
    ("PayingYuiMedalBonuses", include_str!("data/PayingYuiMedalBonuses.csv")),
    ("PlayCampaignRewards", include_str!("data/PlayCampaignRewards.csv")),
    ("PlayCampaigns", include_str!("data/PlayCampaigns.csv")),
    ("PlayerRank", include_str!("data/PlayerRank.csv")),
    ("PlayerTraceTable", include_str!("data/PlayerTraceTable.csv")),
    ("Property", include_str!("data/Property.csv")),
    ("PropertyTable", include_str!("data/PropertyTable.csv")),
    ("PropertyType", include_str!("data/PropertyType.csv")),
    ("QuestDropBoostCampaigns", include_str!("data/QuestDropBoostCampaigns.csv")),
    ("QuestEpisodeAppendRewards", include_str!("data/QuestEpisodeAppendRewards.csv")),
    ("QuestExistUnit", include_str!("data/QuestExistUnit.csv")),
    ("QuestParam", include_str!("data/QuestParam.csv")),
    ("QuestRareDrop", include_str!("data/QuestRareDrop.csv")),
    ("QuestScene", include_str!("data/QuestScene.csv")),
    ("QuestSpecialRareDrops", include_str!("data/QuestSpecialRareDrops.csv")),
    ("QuestSpecialRareDropSettings", include_str!("data/QuestSpecialRareDropSettings.csv")),
    ("QuestTutorial", include_str!("data/QuestTutorial.csv")),
    ("Rarity", include_str!("data/Rarity.csv")),
    ("ResEarnCampaignShopItems", include_str!("data/ResEarnCampaignShopItems.csv")),
    ("ResEarnCampaignShops", include_str!("data/ResEarnCampaignShops.csv")),
    ("RewardSet", include_str!("data/RewardSet.csv")),
    ("RewardTable", include_str!("data/RewardTable.csv")),
    ("SideQuest", include_str!("data/SideQuest.csv")),
    ("Skill", include_str!("data/Skill.csv")),
    ("SkillCutin", include_str!("data/SkillCutin.csv")),
    ("SkillCutInTextIndex", include_str!("data/SkillCutInTextIndex.csv")),
    ("SkillLevel", include_str!("data/SkillLevel.csv")),
    ("SkillTable", include_str!("data/SkillTable.csv")),
    ("SoundBgm", include_str!("data/SoundBgm.csv")),
    ("SoundSe", include_str!("data/SoundSe.csv")),
    ("Sound_SkillCutinVoice", include_str!("data/Sound_SkillCutinVoice.csv")),
    ("SoundUi", include_str!("data/SoundUi.csv")),
    ("SoundVoice", include_str!("data/SoundVoice.csv")),
    ("SubStateMonster", include_str!("data/SubStateMonster.csv")),
    ("SupportLog", include_str!("data/SupportLog.csv")),
    ("SupportLogType", include_str!("data/SupportLogType.csv")),
    ("SynchroSkill", include_str!("data/SynchroSkill.csv")),
    ("Tips", include_str!("data/Tips.csv")),
    ("Title", include_str!("data/Title.csv")),
    ("TreasureHuntTaskText", include_str!("data/TreasureHuntTaskText.csv")),
    ("TrialTower", include_str!("data/TrialTower.csv")),
    ("UnanalyzedLogGrade", include_str!("data/UnanalyzedLogGrade.csv")),
    ("UnitCollision", include_str!("data/UnitCollision.csv")),
    ("Unit", include_str!("data/Unit.csv")),
    ("UnitGimmick", include_str!("data/UnitGimmick.csv")),
    ("UnitPower", include_str!("data/UnitPower.csv")),
    ("WeaponType", include_str!("data/WeaponType.csv")),
    ("YuiMedalBonusCondition", include_str!("data/YuiMedalBonusCondition.csv")),
    ("YuiMedalBonus", include_str!("data/YuiMedalBonus.csv")),
    ("YuiMedalGetCondition", include_str!("data/YuiMedalGetCondition.csv")),
    ("YuiMedalShopItems", include_str!("data/YuiMedalShopItems.csv")),
    ("YuiMedalShops", include_str!("data/YuiMedalShops.csv")),
];

/// One table: its header (ARTEMiS strips the `// ` of the first column's name), its rows.
pub(crate) struct Table {
    columns: HashMap<String, usize>,
    pub(crate) rows: Vec<Vec<String>>,
}

/// A row of a table.
#[derive(Clone, Copy)]
pub(crate) struct Row<'a> {
    table: &'a Table,
    cells: &'a [String],
}

/// A cell as ARTEMiS types it.
enum Cell<'a> {
    Int(i64),
    Bool(bool),
    /// year, month, day, hour, minute, second
    Date([u32; 6]),
    Str(&'a str),
}

fn date(s: &str) -> Option<[u32; 6]> {
    let (day, time) = match s.split_once(' ') {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let d: Vec<&str> = day.split('/').collect();
    if d.len() != 3 || d[0].len() != 4 || d[1].len() != 2 || d[2].len() != 2 || !day.bytes().all(|b| b == b'/' || b.is_ascii_digit()) {
        return None;
    }
    let mut out = [d[0].parse().ok()?, d[1].parse().ok()?, d[2].parse().ok()?, 0, 0, 0];
    if let Some(t) = time {
        let t: Vec<&str> = t.split(':').collect();
        if t.len() != 3 || !(1..=2).contains(&t[0].len()) || t[1].len() != 2 || t[2].len() != 2 || !t.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        out[3] = t[0].parse().ok()?;
        out[4] = t[1].parse().ok()?;
        out[5] = t[2].parse().ok()?;
    }
    Some(out)
}

fn cell(s: &str) -> Cell<'_> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().map_or(Cell::Str(s), Cell::Int);
    }
    if s.eq_ignore_ascii_case("true") {
        return Cell::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Cell::Bool(false);
    }
    date(s).map_or(Cell::Str(s), Cell::Date)
}

/// A cell for an integer field (`encode_int`: `int()` of it, 0 when that fails).
pub(crate) fn as_int(s: &str) -> i64 {
    match cell(s) {
        Cell::Int(n) => n,
        Cell::Bool(b) => b as i64,
        Cell::Date(_) => 0,
        Cell::Str(s) => s.trim().parse().unwrap_or(0),
    }
}

/// A cell for a string field (`encode_str`: `str()` of it).
fn as_text(s: &str) -> String {
    match cell(s) {
        Cell::Int(n) => n.to_string(),
        Cell::Bool(b) => if b { "True" } else { "False" }.to_string(),
        Cell::Date([y, mo, d, h, mi, se]) => format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{se:02}"),
        Cell::Str(s) => s.to_string(),
    }
}

/// A cell for a date field (`encode_date_str`: `%Y%m%d%H%M%S`). ARTEMiS fails the whole answer
/// on a cell that is no date; the Unix epoch here.
fn as_date(s: &str) -> String {
    match cell(s) {
        Cell::Date([y, mo, d, h, mi, se]) => format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{se:02}"),
        _ => super::EPOCH.to_string(),
    }
}

/// CSV records, read as Python's `csv` module reads a file opened in text mode (universal
/// newlines: `\r\n` and `\r` are `\n`): its parser's states, its non-strict handling of a quote
/// inside a quoted field followed by another character (ARTEMiS' `Property.csv` has
/// `"<color="red">..."`: the rest of the field read as is, quotes included). Empty records are
/// skipped (`DictReader`).
fn records(text: &str) -> Vec<Vec<String>> {
    #[derive(PartialEq)]
    enum State {
        Start,
        Field,
        Quoted,
        QuoteInQuoted,
    }
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let (mut out, mut record, mut cur) = (Vec::new(), Vec::new(), String::new());
    let mut state = State::Start;
    let mut end_record = |record: &mut Vec<String>, cur: &mut String| {
        record.push(std::mem::take(cur));
        if !(record.len() == 1 && record[0].is_empty()) {
            out.push(std::mem::take(record));
        }
        record.clear();
    };
    for c in text.chars() {
        state = match (state, c) {
            (State::Start, '"') => State::Quoted,
            (State::Start | State::Field | State::QuoteInQuoted, ',') => {
                record.push(std::mem::take(&mut cur));
                State::Start
            }
            (State::Start | State::Field | State::QuoteInQuoted, '\n') => {
                end_record(&mut record, &mut cur);
                State::Start
            }
            (State::Quoted, '"') => State::QuoteInQuoted,
            (State::QuoteInQuoted, '"') => {
                cur.push('"');
                State::Quoted
            }
            (State::Quoted, c) => {
                cur.push(c);
                State::Quoted
            }
            (_, c) => {
                cur.push(c);
                State::Field
            }
        };
    }
    if !cur.is_empty() || !record.is_empty() {
        end_record(&mut record, &mut cur);
    }
    out
}

fn parse(text: &str) -> Table {
    let mut records = records(text).into_iter();
    let header = records.next().unwrap_or_default();
    let columns = header.iter().enumerate().map(|(i, k)| (k.strip_prefix("// ").unwrap_or(k).to_string(), i)).collect();
    Table { columns, rows: records.collect() }
}

/// The table of a CSV file (`HeroLog`); empty when there is none.
pub(crate) fn table(name: &str) -> &'static Table {
    static TABLES: OnceLock<HashMap<&'static str, Table>> = OnceLock::new();
    static EMPTY: OnceLock<Table> = OnceLock::new();
    let tables = TABLES.get_or_init(|| FILES.iter().map(|&(n, text)| (n, parse(text))).collect());
    tables.get(name).unwrap_or_else(|| {
        crate::log(&format!("sao: no data file {name}.csv"));
        EMPTY.get_or_init(|| Table { columns: HashMap::new(), rows: Vec::new() })
    })
}

impl Table {
    pub(crate) fn iter(&self) -> impl Iterator<Item = Row<'_>> {
        self.rows.iter().map(move |cells| Row { table: self, cells })
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn row(&self, i: usize) -> Option<Row<'_>> {
        self.rows.get(i).map(|cells| Row { table: self, cells })
    }

    /// The first row whose column `col` is `value` (a static table's id lookup).
    pub(crate) fn find(&self, col: &str, value: i64) -> Option<Row<'_>> {
        self.iter().find(|r| r.int(col) == value)
    }

    /// The rows whose column `col` is `value`.
    pub(crate) fn filter(&self, col: &str, value: i64) -> Vec<Row<'_>> {
        self.iter().filter(|r| r.int(col) == value).collect()
    }
}

impl<'a> Row<'a> {
    pub(crate) fn raw(&self, col: &str) -> &'a str {
        self.table.columns.get(col).and_then(|&i| self.cells.get(i)).map_or("", |s| s.as_str())
    }

    /// A column as Python's `int()` of the typed cell (0 when empty or no number).
    pub(crate) fn int(&self, col: &str) -> i64 {
        as_int(self.raw(col))
    }

    /// The reward of a row (`CommonRewardType`, `CommonRewardId`, `CommonRewardNum`).
    pub(crate) fn reward(&self) -> super::Reward {
        super::Reward { kind: self.int("CommonRewardType"), id: self.int("CommonRewardId"), num: self.int("CommonRewardNum") }
    }
}

/// A master data answer: the command's CSV rows as its list's structures (`master.rs`), or
/// `None` for another command.
pub(crate) fn master(cmd: u16) -> Option<V> {
    let &(_, file, mapping) = super::master::MASTER.iter().find(|m| m.0 == cmd)?;
    let answer = super::wire::message(cmd + 1)?;
    let (list_name, _) = super::wire::fields(answer).into_iter().find(|(_, t)| *t == 'S')?;
    // the list's structure: its field types
    let structure = super::schema::MESSAGES.iter().find(|m| m.0 == answer).and_then(|m| {
        m.3.split(", ").find(|f| f.ends_with(&format!(" {list_name}"))).and_then(|f| f.split(']').next()).map(|t| t.trim_start_matches('['))
    })?;
    let types: HashMap<&str, char> = super::wire::fields(structure).into_iter().collect();
    let columns: Vec<(&str, &str, bool)> = mapping
        .split(", ")
        .filter_map(|m| m.split_once('='))
        .map(|(f, c)| match c.strip_suffix(":d") {
            Some(c) => (f, c, true),
            None => (f, c, false),
        })
        .collect();
    let rows: Vec<V> = table(file)
        .iter()
        .map(|row| {
            let mut v = V::obj();
            for &(field, col, is_date) in &columns {
                let raw = row.raw(col);
                let value = match (types.get(field), is_date) {
                    (_, true) => V::Str(as_date(raw)),
                    (Some('s'), false) => V::Str(as_text(raw)),
                    _ => V::Int(as_int(raw)),
                };
                v.set(field, value);
            }
            v
        })
        .collect();
    Some(V::obj().with("result", 1).with(list_name, V::List(rows)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells() {
        assert_eq!(as_int("12"), 12);
        assert_eq!(as_int("-3"), -3);
        assert_eq!(as_int("true"), 1);
        assert_eq!(as_int(""), 0);
        assert_eq!(as_text("007"), "7");
        assert_eq!(as_text("2020/01/01"), "2020-01-01 00:00:00");
        assert_eq!(as_date("2021/01/01 23:59:59"), "20210101235959");
        assert_eq!(records("h\r\n1,\"a,b\",\"c\"\"d\"\r\n2,\"x\r\ny\"\r\n"), vec![vec!["h"], vec!["1", "a,b", "c\"d"], vec!["2", "x\ny"]]);
        assert_eq!(records("\"<c=\"r\">x\",1"), vec![vec!["<c=r\">x\"", "1"]]);
    }

    #[test]
    fn master_player_ranks() {
        let v = master(0xd500).unwrap();
        let list = v.get("player_rank_data_list").list();
        assert!(list.len() > 10);
        assert_eq!(list[1].i("total_exp"), 100);
        assert_eq!(list[1].i("storage"), 1505);
        for &(cmd, _, _) in super::super::master::MASTER {
            assert!(master(cmd).is_some(), "{cmd:#x}");
        }
    }
}
