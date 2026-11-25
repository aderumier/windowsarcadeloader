//! The English mod's texts of the card database (`resource\Battle\GDCardData_zhs.wdb`: card
//! names, attacks, skills, missions...): the mod patches the game's message tables and
//! pictures on disk, the card database's texts are the loader's, from `app\English\English.tsv`
//! (`original<TAB>English` per line, `\n`, `\t`, `\\` escaped, its `[game]` section).
//!
//! The database's texts are UTF-16 strings (packed, without alignment) the records point to by
//! their offset from the end of the file's header (0xBC). An
//! English copy of the file is made at start (`WindowsLoader\sdbh\GDCardData_en.wdb`): each
//! text of the table appended at the end, the records' offsets of the Chinese one moved to it;
//! the game opening the database opens the copy.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

const DATABASE: &str = "GDCardData_zhs.wdb";
/// The records' string offsets count from here (the header's end).
const BASE: usize = 0xBC;

static PATCHED: OnceLock<Option<Vec<u8>>> = OnceLock::new();
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `[game]` texts of the table.
fn table(path: &std::path::Path) -> HashMap<Vec<u16>, Vec<u16>> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return map };
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        if section != "game" || line.starts_with('#') {
            continue;
        }
        let Some((original, english)) = line.split_once('\t') else { continue };
        if english.is_empty() {
            continue;
        }
        map.insert(unescape(original).encode_utf16().collect(), unescape(english).encode_utf16().collect());
    }
    map
}

/// The database with the table's texts: the strings appended, the records' offsets moved.
fn patch(original: &[u8], texts: &HashMap<Vec<u16>, Vec<u16>>) -> (Vec<u8>, usize) {
    let mut out = original.to_vec();
    let read_string = |at: usize| -> Option<Vec<u16>> {
        let mut s = Vec::new();
        let mut i = at;
        while i + 1 < original.len() {
            let c = u16::from_le_bytes([original[i], original[i + 1]]);
            if c == 0 {
                return Some(s);
            }
            s.push(c);
            i += 2;
            if s.len() > 4096 {
                return None;
            }
        }
        None
    };
    let mut moved: HashMap<u32, u32> = HashMap::new();
    let mut count = 0;
    for k in (BASE..original.len().saturating_sub(4)).step_by(4) {
        let v = u32::from_le_bytes([original[k], original[k + 1], original[k + 2], original[k + 3]]);
        let at = BASE + v as usize;
        // a string's start: inside the file, after the previous string's NUL
        if v == 0 || at >= original.len() || at <= k + 4 || original[at - 1] != 0 {
            continue;
        }
        let new = match moved.get(&v) {
            Some(&n) => n,
            None => {
                let Some(s) = read_string(at).filter(|s| !s.is_empty()) else { continue };
                let Some(english) = texts.get(&s) else { continue };
                let n = (out.len() - BASE) as u32;
                for c in english.iter().chain(std::iter::once(&0)) {
                    out.extend_from_slice(&c.to_le_bytes());
                }
                moved.insert(v, n);
                n
            }
        };
        out[k..k + 4].copy_from_slice(&new.to_le_bytes());
        count += 1;
    }
    log!("sdbh: {count} references of the card database moved to the English texts");
    (out, moved.len())
}

pub fn init(exe: usize) {
    let root = super::ddtel::game_root();
    let tsv = root.join("app").join("English").join("English.tsv");
    let source = root.join("app").join("resource").join("Battle").join(DATABASE);
    let patched = PATCHED.get_or_init(|| {
        let texts = table(&tsv);
        if texts.is_empty() {
            log!("sdbh: no English text table ({}): the card database stays as it is", tsv.display());
            return None;
        }
        let original = std::fs::read(&source).ok()?;
        let (data, moved) = patch(&original, &texts);
        let path = super::save_dir().join("GDCardData_en.wdb");
        let _ = std::fs::create_dir_all(super::save_dir());
        if std::fs::read(&path).ok().as_deref() != Some(&data) {
            if let Err(e) = std::fs::write(&path, &data) {
                log!("sdbh: English card database not written: {}: {e}", path.display());
                return None;
            }
        }
        log!("sdbh: card database in English: {moved} texts of {} in the table", texts.len());
        Some(path.to_string_lossy().into_owned().into_bytes())
    });
    if patched.is_none() {
        return;
    }
    match unsafe { iat::hook_module(exe, "kernel32.dll", "CreateFileA", create_file_a as *const () as usize) } {
        Some(o) => CREATE_FILE_A.store(o, Ordering::Relaxed),
        None => log!("sdbh: the game does not import CreateFileA: the card database stays in Chinese"),
    }
}

type P = *mut std::ffi::c_void;

unsafe extern "system" fn create_file_a(path: *const u8, access: u32, share: u32, sa: P, disposition: u32, flags: u32, template: P) -> P {
    let real: unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    if !path.is_null() {
        let name = unsafe { std::ffi::CStr::from_ptr(path.cast()) }.to_bytes();
        if name.len() >= DATABASE.len() && name[name.len() - DATABASE.len()..].eq_ignore_ascii_case(DATABASE.as_bytes())
            && let Some(Some(ansi)) = PATCHED.get()
        {
            let mut copy = ansi.clone();
            copy.push(0);
            return unsafe { real(copy.as_ptr(), access, share, sa, disposition, flags, template) };
        }
    }
    unsafe { real(path, access, share, sa, disposition, flags, template) }
}
