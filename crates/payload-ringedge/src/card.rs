//! The magnetic card reader/writer (Sanwa CRP-1231BR: Initial D; `WAL_RINGEDGE_CARD_MODEL=lr`:
//! the CRP-1231LR). On the cabinet
//! `picodaemon.exe` drives the reader on a serial port (COM2) and shares its state with the
//! game (shared memory `zmShare...`); the game never opens the port itself.
//!
//! `WAL_RINGEDGE_CARD_DAEMON=picodaemon.exe`: the game's payload starts that daemon (with this
//! payload, through `wal-loader`), killed with the game (a job object); in the daemon the
//! payload is the reader on its serial ports.
//!
//! Protocol: the host sends `02 <len> <command> 00 00 00 <parameters...> 03 <xor>` (`len`
//! counts the bytes after it, the checksum included; the checksum is the XOR of `len` .. `03`),
//! the reader acknowledges (`06`, `15` on a bad checksum); then the host polls with `05` and
//! the reader answers `02 <len> <command> <position> <error> <state> <data...> 03 <xor>`. A
//! command runs over several polls (state `33` running, `30` done, `34` waiting for a card).
//! Card position (LR): `30` none, `31` read/write head, `32` thermal (print) head, `33`
//! dispenser, `34` ejected; BR: bit 7 shutter closed (else bit 6), bit 5 cards in the
//! dispenser, then `00` none, `01` in the slot, `18` head, `07` thermal head, `1C` dispenser;
//! the BR has a shutter (`D0`) and keeps an inserted card in its slot until a read takes it in.
//! A card is 3 magnetic tracks of 69 bytes.
//!
//! The card: `WAL_RINGEDGE_CARD_FILE` (default `card.bin` in the save folder
//! `WAL_RINGEDGE_SAVE`, default `sv`), the 3 tracks one after another, and its RFID chip
//! ([`crate::rfid`], the BR's `D1` command) in the same name with `.ic` (`card.ic`), written
//! back when the card is ejected. It is inserted when the game waits for a card (a blank card
//! when the files do not exist yet: the game issues a new card on it; the BR: no card, the
//! game sells one from its dispenser).

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Mutex;

use wal_payload_common::{log, serial};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};

/// This process is the card daemon: the reader on every COM port it may open.
pub(crate) fn daemon() -> bool {
    let Ok(name) = std::env::var("WAL_RINGEDGE_CARD_DAEMON") else { return false };
    let exe = std::env::current_exe().ok().and_then(|e| e.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    if !exe.eq_ignore_ascii_case(name.trim()) {
        return false;
    }
    wal_payload_common::start_helper("ringedge card daemon");
    let lr = std::env::var("WAL_RINGEDGE_CARD_MODEL").is_ok_and(|m| m.trim().eq_ignore_ascii_case("lr"));
    READER.lock().unwrap().br = !lr;
    for n in 1..=9 {
        serial::install(&format!("COM{n}"), reply);
    }
    log!("card: reader emulated for {exe}, card {}", card_path().display());
    true
}

/// In the game: starts the card daemon beside it.
pub(crate) fn start_daemon() {
    let Ok(name) = std::env::var("WAL_RINGEDGE_CARD_DAEMON") else { return };
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return };
    let payload = dir.join("wal_ringedge.dll");
    let line = format!("\"{}\" \"{}\" \"{}\"", dir.join("wal-loader.exe").display(), payload.display(), dir.join(name.trim()).display());
    let mut wide: Vec<u16> = std::ffi::OsStr::new(&line).encode_wide().chain([0]).collect();
    let cwd: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32);
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(std::ptr::null(), wide.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_NO_WINDOW, std::ptr::null(), cwd.as_ptr(), &si, &mut pi) == 0 {
            log!("card: cannot start {line}");
            return;
        }
        // the job handle stays open until the game exits: the daemon (and the loader) die with it
        AssignProcessToJobObject(job, pi.hProcess);
    }
    log!("card: {} started", name.trim());
}

fn card_path() -> PathBuf {
    let save = std::env::var("WAL_RINGEDGE_SAVE").unwrap_or_else(|_| "sv".into());
    let file = std::env::var("WAL_RINGEDGE_CARD_FILE").unwrap_or_else(|_| "card.bin".into());
    PathBuf::from(save.trim()).join(file.trim())
}

fn chip_path() -> PathBuf {
    card_path().with_extension("ic")
}

const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const ENQ: u8 = 0x05;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const TRACK: usize = 69;
const VERSION: &[u8] = b"AP:S1234-5678,OS:S9012-3456,0000";

/// Card position (the reply's position byte).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Position {
    None = 0x30,
    Head = 0x31,
    Thermal = 0x32,
    Dispenser = 0x33,
    Ejected = 0x34,
}

// error byte
const NO_ERROR: u8 = 0x30;
const READ_ERROR: u8 = 0x31;
const PRINT_ERROR: u8 = 0x35;
const ILLEGAL_ERROR: u8 = 0x38;
const SYSTEM_ERROR: u8 = 0x41;
// state byte
const IDLE: u8 = 0x30;
const ILLEGAL_COMMAND: u8 = 0x32;
const RUNNING: u8 = 0x33;
const WAITING_FOR_CARD: u8 = 0x34;

struct Reader {
    input: Vec<u8>,
    command: u8,
    params: Vec<u8>,
    running: bool,
    /// Polls of the running command.
    step: u32,
    position: Position,
    error: u8,
    state: u8,
    /// The reply's data.
    data: Vec<u8>,
    tracks: [Vec<u8>; 3],
    /// A card is going in (inserted at the next poll).
    inserting: bool,
    /// CRP-1231BR (else LR).
    br: bool,
    shutter_closed: bool,
    /// The card's RFID chip (a card in the reader).
    chip: Option<crate::rfid::Chip>,
}

static READER: Mutex<Reader> = Mutex::new(Reader {
    input: Vec::new(),
    command: 0,
    params: Vec::new(),
    running: false,
    step: 0,
    position: Position::None,
    error: NO_ERROR,
    state: IDLE,
    data: Vec::new(),
    tracks: [Vec::new(), Vec::new(), Vec::new()],
    inserting: false,
    br: true,
    shutter_closed: true,
    chip: None,
});

fn reply(bytes: &[u8]) -> Vec<u8> {
    let mut reader = READER.lock().unwrap();
    reader.input.extend_from_slice(bytes);
    let mut out = Vec::new();
    loop {
        match reader.input.first() {
            None => break,
            Some(&ENQ) => {
                reader.input.remove(0);
                reader.poll();
                out.extend(reader.packet());
            }
            Some(&STX) => {
                let Some(&len) = reader.input.get(1) else { break };
                let len = len as usize;
                if reader.input.len() < len + 2 {
                    break;
                }
                let packet: Vec<u8> = reader.input.drain(..len + 2).collect();
                let sum = packet[1..len + 1].iter().fold(0, |a, b| a ^ b);
                if len < 6 || packet[len] != ETX || sum != packet[len + 1] {
                    log!("card: bad packet {packet:02X?}");
                    out.push(NAK);
                    continue;
                }
                reader.receive(packet[2], &packet[6..len]);
                out.push(ACK);
            }
            Some(_) => {
                reader.input.remove(0);
            }
        }
    }
    out
}

impl Reader {
    fn receive(&mut self, command: u8, params: &[u8]) {
        if command != 0x20 {
            log!("card: command {command:02X} {params:02X?}");
        }
        // the next command after an eject: the card is out
        if self.command == 0x80 {
            self.eject();
        }
        self.command = command;
        self.params = params.to_vec();
        self.error = NO_ERROR;
        self.state = RUNNING;
        self.running = true;
        self.step = 0;
        self.data.clear();
    }

    fn position_byte(&self) -> u8 {
        if !self.br {
            return self.position as u8;
        }
        let place = match self.position {
            Position::None => 0x00,
            Position::Ejected => 0x01,
            Position::Head => 0x18,
            Position::Thermal => 0x07,
            Position::Dispenser => 0x1C,
        };
        (if self.shutter_closed { 0x80 } else { 0x40 }) | 0x20 | place
    }

    fn packet(&self) -> Vec<u8> {
        let mut body = vec![self.command, self.position_byte(), self.error, self.state];
        body.extend_from_slice(&self.data);
        let len = (body.len() + 2) as u8;
        let mut out = vec![STX, len];
        out.extend_from_slice(&body);
        out.push(ETX);
        out.push(body.iter().fold(len ^ ETX, |a, b| a ^ b));
        out
    }

    fn has_card(&self) -> bool {
        // the BR's slot holds the card
        self.position != Position::None && (self.br || self.position != Position::Ejected)
    }

    fn fail(&mut self, error: u8) {
        self.error = error;
        self.running = false;
    }

    fn done(&mut self) {
        self.error = NO_ERROR;
        self.state = IDLE;
        self.running = false;
    }

    /// The card goes in: read from its file.
    fn insert(&mut self) {
        let path = card_path();
        self.tracks = [Vec::new(), Vec::new(), Vec::new()];
        match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() && bytes.len() % TRACK == 0 => {
                for (track, chunk) in self.tracks.iter_mut().zip(bytes.chunks(TRACK)) {
                    *track = chunk.to_vec();
                }
                log!("card: {} inserted", path.display());
            }
            Ok(bytes) => log!("card: {} has a wrong size ({}): blank card inserted", path.display(), bytes.len()),
            Err(_) => log!("card: no {}: blank card inserted", path.display()),
        }
        self.chip = Some(crate::rfid::Chip::load(&chip_path()));
        self.position = if self.br { Position::Ejected } else { Position::Head };
    }

    /// The card comes out: written to its file.
    fn eject(&mut self) {
        if self.position == Position::None {
            return;
        }
        self.position = Position::Ejected;
        self.save();
        log!("card: ejected");
        self.tracks = [Vec::new(), Vec::new(), Vec::new()];
        self.chip = None;
    }

    /// The card in the reader written to its files (at each write: the game may quit with the
    /// card inside, as a cabinet keeps it during a game).
    fn save(&mut self) {
        let bytes: Vec<u8> = self.tracks.iter().flatten().copied().collect();
        if !bytes.is_empty() {
            let path = card_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(&path, &bytes) {
                Ok(()) => log!("card: {} written ({} bytes)", path.display(), bytes.len()),
                Err(e) => log!("card: {}: {e}", path.display()),
            }
        }
        if let Some(chip) = self.chip.as_mut() {
            chip.save(&chip_path());
        }
    }

    /// Waits for a card: one goes in at once. The BR without a card file (stripe or chip) yet:
    /// none (a blank card inserted is an unknown card for Initial D: the player buys one from
    /// the dispenser).
    fn wait_for_card(&mut self) {
        let first = self.state != WAITING_FOR_CARD;
        self.state = WAITING_FOR_CARD;
        if self.br && !card_path().exists() && !chip_path().exists() {
            if first {
                log!("card: no {}: no card inserted (a new card: the game's dispenser)", card_path().display());
            }
        } else {
            self.inserting = true;
        }
        self.step = self.step.wrapping_sub(1);
    }

    /// A poll: the card moves, the running command advances.
    fn poll(&mut self) {
        if !self.running && self.state != ILLEGAL_COMMAND {
            self.state = IDLE;
        }
        // an ejected card is taken (the BR keeps it while it reads or works its shutter)
        if self.position == Position::Ejected && !(self.br && matches!(self.command, 0x33 | 0xD0)) {
            self.position = Position::None;
            self.inserting = false;
        }
        if self.inserting && self.position == Position::None {
            self.inserting = false;
            self.insert();
            if self.running && self.state == WAITING_FOR_CARD {
                self.state = RUNNING;
                self.shutter_closed = true;
            }
        }
        if !self.running {
            return;
        }
        let step = self.step;
        match self.command {
            // initialize: the card out (the BR: to its slot, [0x31]: shutter closed after)
            0x10 if self.br => {
                if self.params.first() == Some(&0x31) {
                    self.shutter_closed = true;
                }
                if self.position != Position::None {
                    self.position = Position::Ejected;
                }
                self.done();
            }
            0x10 => {
                self.eject();
                self.done();
            }
            // shutter: [0x30] close, [0x31] open
            0xD0 if self.br => {
                self.shutter_closed = self.params.first() != Some(&0x31);
                self.done();
            }
            // D1 (BR): the reader's RFID part, the card's chip; its reply data starts with a
            // status byte, 0B success (no data: picodaemon's error 87)
            0xD1 => {
                let params = self.params.clone();
                let card = self.has_card();
                let data = match self.chip.as_mut() {
                    Some(chip) if card => chip.command(&params),
                    // no card: the reader's own sub-commands accepted, the chip's fail (1A)
                    _ if matches!(params.first(), Some(0x48 | 0xF7 | 0x28 | 0x38)) => vec![0x0B, 0x00, 0x00],
                    _ => vec![0x1A, 0x00],
                };
                if params.first() != Some(&0x48) {
                    log!("card: D1 {params:02X?} -> {:02X?}", &data[..data.len().min(12)]);
                }
                if self.chip.as_ref().is_some_and(|c| c.changed) {
                    let path = chip_path();
                    if let Some(chip) = self.chip.as_mut() {
                        chip.save(&path);
                    }
                }
                self.data.extend_from_slice(&data);
                self.done();
            }
            // status, LED, print settings, fonts, retries, battery
            0x20 | 0x78 | 0x7A | 0xC0 | 0xC1 | 0xE1 | 0xF5 => self.done(),
            // read: [mode, bit mode, tracks]; mode 0x32 only takes the card in
            0x33 => {
                if step == 1 {
                    if !self.has_card() {
                        self.wait_for_card();
                    } else if self.params.first() == Some(&0x32) {
                        self.position = Position::Head;
                    } else {
                        let tracks: &[usize] = match self.params.get(2) {
                            Some(0x30) => &[0],
                            Some(0x31) => &[1],
                            Some(0x32) => &[2],
                            Some(0x33) => &[0, 1],
                            Some(0x34) => &[0, 2],
                            Some(0x35) => &[1, 2],
                            Some(0x36) => &[0, 1, 2],
                            _ => return self.fail(ILLEGAL_ERROR),
                        };
                        if tracks.iter().any(|&t| self.tracks[t].is_empty()) {
                            return self.fail(READ_ERROR);
                        }
                        for &t in tracks {
                            let track = self.tracks[t].clone();
                            self.data.extend_from_slice(&track);
                        }
                    }
                }
                if step >= 1 && self.state != WAITING_FOR_CARD {
                    self.done();
                }
            }
            // all the tracks
            0x35 => {
                if step == 1 {
                    if !self.has_card() {
                        return self.fail(ILLEGAL_ERROR);
                    }
                    if self.tracks.iter().any(Vec::is_empty) {
                        return self.fail(READ_ERROR);
                    }
                    let all: Vec<u8> = self.tracks.iter().flatten().copied().collect();
                    self.data.extend_from_slice(&all);
                }
                if step > 1 {
                    self.done();
                }
            }
            // cancel
            0x40 => {
                self.state = ILLEGAL_COMMAND;
                self.running = false;
            }
            // write: [mode, bit mode, tracks, data...]
            0x53 => {
                if step == 1 {
                    if !self.has_card() || self.params.first() == Some(&0x31) || self.params.len() < 3 {
                        return self.fail(ILLEGAL_ERROR);
                    }
                    let tracks: &[usize] = match self.params[2] {
                        0x30 => &[0],
                        0x31 => &[1],
                        0x32 => &[2],
                        0x33 => &[0, 1],
                        0x34 => &[0, 2],
                        0x35 => &[1, 2],
                        0x36 => &[0, 1, 2],
                        _ => return self.fail(ILLEGAL_ERROR),
                    };
                    let data = self.params[3..].to_vec();
                    if tracks.len() > 1 && data.len() < TRACK * (tracks.len() - 1) + 1 {
                        return self.fail(SYSTEM_ERROR);
                    }
                    for (i, &t) in tracks.iter().enumerate() {
                        let end = if i + 1 == tracks.len() { data.len() } else { (i + 1) * TRACK };
                        self.tracks[t] = data[(i * TRACK).min(data.len())..end.min(data.len())].to_vec();
                    }
                    self.save();
                }
                if step > 1 {
                    self.done();
                }
            }
            // print an image
            0x7B => {
                if self.has_card() {
                    self.done();
                } else {
                    self.fail(PRINT_ERROR);
                }
            }
            // print a line, erase the print: through the thermal head
            0x7C | 0x7D => {
                match step {
                    0 if !self.has_card() => return self.fail(PRINT_ERROR),
                    0 => self.position = Position::Thermal,
                    1 => self.position = Position::Head,
                    _ => {}
                }
                if step > 1 {
                    self.done();
                }
            }
            // barcode
            0x7E => {
                if step == 1 && !self.has_card() {
                    return self.fail(ILLEGAL_ERROR);
                }
                if step > 1 {
                    self.done();
                }
            }
            // eject
            0x80 => {
                if step == 2 {
                    self.eject();
                }
                if step > 2 {
                    self.done();
                }
            }
            // head cleaning: a cleaning card goes in (no stripe, no chip: nothing saved)
            0xA0 => {
                match step {
                    1 if !self.has_card() => {
                        self.tracks = [Vec::new(), Vec::new(), Vec::new()];
                        self.chip = None;
                        self.position = Position::Head;
                        log!("card: cleaning card inserted");
                    }
                    2 => self.position = Position::Thermal,
                    3 => self.eject(),
                    _ => {}
                }
                if step > 3 {
                    self.done();
                }
            }
            // new card from the dispenser: a blank card
            0xB0 => {
                match step {
                    1 if self.has_card() => {
                        self.state = ILLEGAL_COMMAND;
                        self.running = false;
                        return;
                    }
                    1 => {
                        self.tracks = [Vec::new(), Vec::new(), Vec::new()];
                        self.chip = Some(crate::rfid::Chip::blank());
                        self.position = Position::Dispenser;
                        log!("card: new card dispensed");
                    }
                    2 => self.position = Position::Head,
                    _ => {}
                }
                if step > 2 {
                    self.done();
                }
            }
            // version, clock
            0xF0 => {
                self.data.extend_from_slice(VERSION);
                self.done();
            }
            0xF1 => {
                self.data.extend_from_slice(clock().as_bytes());
                self.done();
            }
            // the LR has no shutter
            0xD0 => {
                self.state = ILLEGAL_COMMAND;
                self.running = false;
            }
            // anything else (D2 <30|31>: a setting of the BR): done (picodaemon reports an
            // "illegal command" state as an error)
            other => {
                log!("card: command {other:02X} accepted");
                self.done();
            }
        }
        self.step = self.step.wrapping_add(1);
    }
}

/// Local time as `yymmddHHMMSS`.
fn clock() -> String {
    let t = unsafe {
        let mut t = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        t
    };
    format!("{:02}{:02}{:02}{:02}{:02}{:02}", t.wYear % 100, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}
