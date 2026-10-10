//! The Swarm: one run of the game per stage, as its adapter (`SwarmInput.exe`) does.
//!
//! Each run reads `data\ui\scripts\work\pc_start_mode.lua`, written before it starts: a run
//! token, title or campaign boot, the completed stages, the high scores. When a stage ends the
//! game logs `PC campaign advance: <token>|<from>|<to>|<played>|native|<p1>|<p2>` and
//! `PC terminal results: return to title <token>` in `My Games\The Swarm\game.log`, then exits.
//! The next run starts with the next map and the players' state on its command line
//! (`-skiplogos -s levels\<map>\<map>.map <played> 7 -player1 ... -player2 ... -tips ...`): the
//! `native` fields of the record are the players' health count, game time, hand and tips, read
//! from the game's memory, plus their rolling and alien counters. With no advance record (game
//! over, final stage), back to the title.
//!
//! Here: the payload of the first run writes the startup file (fresh token, attract boot); the
//! payload watches the log for its token's terminal marker, then writes the next run's startup
//! file and asks the launcher to start the game again ([`wal_payload_common::request_relaunch`]).
//! The payload of a relaunched run (`WAL_RELAUNCH=1`) reads its token and completed stages back
//! from that file. The startup file is a profile file (a real file in the run directory, not
//! the dump's).
//!
//! High scores: shown from `pc-highscores.xml`, else the factory `pc-highscores-defaults.xml`
//! (new scores are not saved).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

use wal_payload_common::log;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::Shell::{CSIDL_PERSONAL, SHGetFolderPathW};

const MAPS: [&str; 7] = ["gum", "boss", "metro", "street_square", "undersiege", "square", "final_arena"];
/// The two players' objects (pointers, 0 when not playing).
const PLAYERS: usize = 0x9FFAE8;

const ADVANCE: &str = "PC campaign advance: ";
const RESUME: &str = "PC resume selection: ";
const TERMINAL: &str = "PC terminal results: return to title ";

#[derive(Clone, Copy, Default)]
struct Carry {
    health: i32,
    game_time: f32,
    hand: i32,
    rolling: i32,
    alien: i32,
    tips: [u8; 4],
}

struct Advance {
    map: String,
    played: u32,
    completed: u32,
    tips: String,
    players: [String; 2],
}

pub(crate) struct Campaign {
    dir: PathBuf,
    log_path: PathBuf,
    token: String,
    completed: u32,
    carry: [Option<Carry>; 2],
    log_pos: u64,
    log_tail: String,
    log_read: Option<std::time::Instant>,
    /// lines of this run's token: advance records, resume selections, terminal marker seen
    advances: Vec<String>,
    resumes: Vec<String>,
    terminal: bool,
    /// next run: its token, the relaunch arguments sent
    next_token: String,
    sent: Option<String>,
}

static STATE: Mutex<Option<Campaign>> = Mutex::new(None);

fn start_file(dir: &std::path::Path) -> PathBuf {
    dir.join("data").join("ui").join("scripts").join("work").join("pc_start_mode.lua")
}

fn new_token() -> String {
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (std::process::id() as u64) << 32;
    let mut token = String::new();
    for _ in 0..2 {
        // splitmix64
        seed = seed.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        let _ = write!(token, "{:016x}", z ^ (z >> 31));
    }
    token
}

fn documents() -> Option<PathBuf> {
    let mut buf = [0u16; 260];
    let hr = unsafe { SHGetFolderPathW(std::ptr::null_mut(), CSIDL_PERSONAL as i32, std::ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if hr != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

/// `UI.pcHighScores={...}` from the saved high scores, else the factory ones.
fn high_scores_lua(dir: &std::path::Path) -> String {
    let xml = std::fs::read_to_string(dir.join("pc-highscores.xml"))
        .or_else(|_| std::fs::read_to_string(dir.join("pc-highscores-defaults.xml")))
        .unwrap_or_default();
    let attr = |tag: &str, name: &str| {
        let start = tag.find(&format!("{name}=\""))? + name.len() + 2;
        Some(tag[start..start + tag[start..].find('"')?].to_string())
    };
    let rows: Vec<String> = xml
        .split("<Score ")
        .skip(1)
        .filter_map(|tag| {
            let name = attr(tag, "name")?;
            let score: u64 = attr(tag, "score")?.parse().ok()?;
            let dna: u64 = attr(tag, "dna")?.parse().ok()?;
            // the adapter's own checks: 3 letters or digits
            name.chars().all(|c| c.is_ascii_alphanumeric()).then(|| format!("{{name=\"{name}\",score={score},dna={dna}}}"))
        })
        .take(10)
        .collect();
    format!("UI.pcHighScores={{{}}}\r\n", rows.join(","))
}

fn write_start_file(dir: &std::path::Path, title: bool, token: &str, advance: Option<&Advance>) {
    let mut lua = format!("UI.pcCompletedMask=0\r\nUI.pcBootTitle={title}\r\nUI.pcRunToken=\"{token}\"\r\n");
    if let Some(a) = advance {
        let _ = write!(
            lua,
            "UI.pcCompletedMask={}\r\nUI.pcBootMap=\"{}\"\r\nUI.pcBootChapter={}\r\n",
            a.completed, a.map, a.played
        );
    }
    lua.push_str(&high_scores_lua(dir));
    let path = start_file(dir);
    if let Err(e) = std::fs::write(&path, lua) {
        log!("swarm: writing {}: {e}", path.display());
    }
}

/// Token and completed stages of the startup file (written by the previous run's payload).
fn read_start_file(dir: &std::path::Path) -> Option<(String, u32)> {
    let lua = std::fs::read_to_string(start_file(dir)).ok()?;
    let token = lua.lines().find_map(|l| l.strip_prefix("UI.pcRunToken=\"")?.strip_suffix('"'))?.to_string();
    let completed = lua.lines().filter_map(|l| l.strip_prefix("UI.pcCompletedMask=")?.trim().parse().ok()).last()?;
    Some((token, completed))
}

pub(crate) fn init() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else { return };
    let Some(docs) = documents() else {
        log!("swarm: no Documents folder, no stage change");
        return;
    };
    let log_path = docs.join("My Games").join("The Swarm").join("game.log");
    let (token, completed) = match wal_payload_common::relaunched().then(|| read_start_file(&dir)).flatten() {
        Some(state) => state,
        None => {
            let token = new_token();
            write_start_file(&dir, false, &token, None);
            (token, 0)
        }
    };
    log!("swarm: run token {token}, completed stages {completed:#x}");
    *STATE.lock().unwrap() = Some(Campaign {
        dir,
        log_path,
        token,
        completed,
        carry: [None; 2],
        log_pos: 0,
        log_tail: String::new(),
        log_read: None,
        advances: Vec::new(),
        resumes: Vec::new(),
        terminal: false,
        next_token: new_token(),
        sent: None,
    });
}

/// Called by the input adapter's loop.
pub(crate) fn poll() {
    if let Some(c) = STATE.lock().unwrap().as_mut() {
        c.capture_carry();
        if c.log_read.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(200)) {
            c.log_read = Some(std::time::Instant::now());
            c.read_log();
        }
        if c.terminal {
            c.relaunch();
        }
    }
}

fn read<T: Copy + Default>(address: usize) -> Option<T> {
    let mut v = T::default();
    let mut n = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const _,
            &mut v as *mut T as *mut _,
            size_of::<T>(),
            &mut n,
        )
    };
    (ok != 0 && n == size_of::<T>()).then_some(v)
}

impl Campaign {
    /// The players' state while they play: the last valid one is carried to the next stage.
    fn capture_carry(&mut self) {
        for p in 0..2 {
            let Some(obj) = read::<u32>(PLAYERS + 4 * p).filter(|&o| o != 0).map(|o| o as usize) else { continue };
            let (Some(health), Some(game_time), Some(hand), Some(rolling), Some(alien), Some(tips)) = (
                read::<i32>(obj + 3856),
                read::<f32>(obj + 3888),
                read::<i32>(obj + 4132),
                read::<i32>(obj + 4268),
                read::<i32>(obj + 4272),
                read::<[u8; 4]>(obj + 2657),
            ) else {
                continue;
            };
            if tips.iter().all(|&t| t <= 1)
                && health >= 0
                && game_time.is_finite()
                && game_time >= 0.0
                && hand >= 0
                && rolling >= 0
                && alien >= 0
            {
                self.carry[p] = Some(Carry { health, game_time, hand, rolling, alien, tips });
            }
        }
    }

    /// New lines of the game's log: this run's records.
    fn read_log(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.log_path) else { return };
        let len = f.metadata().map_or(0, |m| m.len());
        if len < self.log_pos {
            self.log_pos = 0;
            self.log_tail.clear();
        }
        if len == self.log_pos || f.seek(SeekFrom::Start(self.log_pos)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return;
        }
        self.log_pos += buf.len() as u64;
        self.log_tail.push_str(&String::from_utf8_lossy(&buf));
        let Some(end) = self.log_tail.rfind('\n') else { return };
        let lines: String = self.log_tail.drain(..=end).collect();
        for line in lines.lines().map(|l| l.trim_end_matches('\r')) {
            if let Some(i) = line.find(ADVANCE) {
                self.advances.push(line[i + ADVANCE.len()..].to_string());
            } else if let Some(i) = line.find(RESUME) {
                self.resumes.push(line[i + RESUME.len()..].to_string());
            } else if line.ends_with(&format!("{TERMINAL}{}", self.token)) {
                log!("swarm: stage over");
                self.terminal = true;
            }
        }
    }

    /// The next run, from the log records and the carried state, as the adapter: the latest
    /// valid record of this run.
    fn next(&self) -> Option<Advance> {
        let mut result = None;
        for r in &self.resumes {
            let f: Vec<&str> = r.split('|').collect();
            if self.completed == 31 && f.len() == 3 && f[0] == self.token && f[1] == "square" {
                if let Ok(score) = f[2].parse::<u32>() {
                    result = Some(Advance {
                        map: "square".into(),
                        played: 6,
                        completed: 31,
                        tips: "1 1 1 1".into(),
                        players: [format!("25 5000 {score} 0 0 2 0 0"), "-".into()],
                    });
                }
            }
        }
        for r in &self.advances {
            if let Some(a) = self.resolve(r) {
                result = Some(a);
            }
        }
        result
    }

    /// An advance record with the `native` fields from the carried state.
    fn resolve(&self, record: &str) -> Option<Advance> {
        let mut f: Vec<String> = record.split('|').map(str::to_string).collect();
        if f.len() != 7 || f[4] != "native" {
            return None;
        }
        let mut tips = [0u8; 4];
        for p in 0..2 {
            if f[5 + p] == "-" {
                continue;
            }
            let c = self.carry[p]?;
            for (t, v) in tips.iter_mut().zip(c.tips) {
                *t |= v;
            }
            let mut packet: Vec<String> = f[5 + p].split(',').map(str::to_string).collect();
            if packet.len() != 6 || packet[3] != "native" || packet[4] != "native" {
                return None;
            }
            packet[3] = c.health.to_string();
            packet[4] = c.game_time.to_string();
            packet[5] = c.hand.to_string();
            f[5 + p] = packet.join(",");
        }
        f[4] = tips.map(|t| t.to_string()).join(" ");
        let mut a = self.parse(&f)?;
        for p in 0..2 {
            if a.players[p] != "-" {
                let c = self.carry[p]?;
                let _ = write!(a.players[p], " {} {}", c.rolling, c.alien);
            }
        }
        Some(a)
    }

    fn parse(&self, f: &[String]) -> Option<Advance> {
        if f[0] != self.token || self.completed > 127 {
            return None;
        }
        let from = MAPS.iter().position(|m| *m == f[1])?;
        let to = MAPS.iter().position(|m| *m == f[2])?;
        if from == to || self.completed & 1 << from != 0 {
            return None;
        }
        let completed = self.completed | 1 << from;
        let played: u32 = f[3].parse().ok()?;
        if completed & 1 << to != 0 || (to == 6 && completed & 0x3F != 0x3F) || played != completed.count_ones() + 1 {
            return None;
        }
        let players = [player(&f[5])?, player(&f[6])?];
        if players.iter().all(|p| p == "-") {
            return None;
        }
        let tips: Vec<String> = f[4].split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect();
        if tips.is_empty() || tips.len() > 4 || tips.iter().any(|t| t.parse::<u32>().is_err()) {
            return None;
        }
        Some(Advance { map: f[2].clone(), played, completed, tips: tips.join(" "), players })
    }

    /// Next run's startup file and arguments, sent to the launcher on each change (the carried
    /// state can still change until the game exits).
    fn relaunch(&mut self) {
        let next = self.next();
        let args = match &next {
            Some(a) => {
                let mut args = format!("-skiplogos -s levels\\{0}\\{0}.map {1} 7", a.map, a.played);
                for (p, packet) in a.players.iter().enumerate() {
                    if packet != "-" {
                        let _ = write!(args, " -player{} {packet}", p + 1);
                    }
                }
                let _ = write!(args, " -tips {}", a.tips);
                args
            }
            None => String::new(),
        };
        if self.sent.as_deref() == Some(args.as_str()) {
            return;
        }
        write_start_file(&self.dir, next.is_none(), &self.next_token, next.as_ref());
        match &next {
            Some(a) => log!("swarm: next run: chapter {} {}, completed stages {:#x}: {args}", a.played, a.map, a.completed),
            None => log!("swarm: next run: title"),
        }
        wal_payload_common::request_relaunch(&args);
        self.sent = Some(args);
    }
}

/// A player's record field, `-` or 6 non-negative numbers, as the adapter formats them.
fn player(field: &str) -> Option<String> {
    if field == "-" {
        return Some(field.to_string());
    }
    let parts: Vec<&str> = field.split(',').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let v: f64 = p.parse().ok()?;
        if !v.is_finite() || !(0.0..=2147483647.0).contains(&v) || ([0, 2, 3, 5].contains(&i) && v != v.floor()) || (i == 1 && v <= 0.0) {
            return None;
        }
        // "0.#########"
        let s = format!("{v:.9}");
        out.push(s.trim_end_matches('0').trim_end_matches('.').to_string());
    }
    Some(out.join(" "))
}
