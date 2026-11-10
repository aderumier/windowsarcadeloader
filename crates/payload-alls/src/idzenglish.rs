//! `WAL_ALLS_IDZ_ENGLISH=1`: Initial D ARCADE STAGE Zero (1.30, Ver.2) in English on the Japanese
//! keychip (its Aime cards and game server kept), from the export version's files, an English
//! patch in the dump's `english\` folder (`WAL_ALLS_IDZ_ENGLISH_DIR`, from the game's folder;
//! default `..\..\english`, version 1.30's `app\package\`; the community "D0 English patch", made
//! for version 1.30's export version: its `data\` folder as it ships) and the translation tables
//! (`strings.rs`). The game's opens under `data\` are redirected, the dump unchanged:
//! - `TxtMessage\data_jp\txbn\X`: the export texts (`data_ex\txbn\X`, the patch's when it has
//!   it) with their texts left in Japanese translated (`Y:\idz_english\txbn`)
//! - `flash\...\X.tcd` / `Xtex.pac`: the export pair (`X_ex.tcd` / `X_extex.pac`: the English
//!   pictures and their layout) when the dump has it, the patch's file first
//! - any other file the patch has: the patch's (`playertex.pac`...); the cars' tuning texts
//!   (`CAR\info_csv\*_car_tuning_info.txt`, UTF-8 or Shift-JIS; the patch's, else the dump's) as
//!   copies in `Y:\idz_english\info_csv`, translated, their Latin letters in full width (the font
//!   of these texts has no half width letters)
//!
//! The texts still in Japanese in those files are replaced by their English translation
//! (`strings.rs`'s tables, by exact text): each `txbn` file, made at start in
//! `Y:\idz_english\txbn\X.txbn` from its export source, is opened in place of the Japanese
//! one (the export source when it does not parse).

mod strings;
mod txbn;

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::{iat, log};

static PATCH: OnceLock<PathBuf> = OnceLock::new();
/// The texts made with their translations, by file name (lower case).
static TRANSLATED: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
/// The cars' tuning texts in full width (`TUNING_DIR`), by lower case file name.
static TUNING: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
const TUNING_DIR: &str = "Y:\\idz_english\\info_csv";
const TUNING_JP: &str = "data/CAR/info_csv";
const TXBN_JP: &str = "data/TxtMessage/data_jp/txbn";
const TRANSLATED_DIR: &str = "Y:\\idz_english\\txbn";
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static FOPEN: AtomicUsize = AtomicUsize::new(0);
static FOPEN_S: AtomicUsize = AtomicUsize::new(0);
/// The redirections logged (each file once).
static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `path`'s part from its `data\` folder (`data/...`, lower case, `/`), when it has one.
fn data_relative(path: &str) -> Option<(usize, String)> {
    let norm = path.replace('\\', "/");
    let lower = norm.to_ascii_lowercase();
    let at = if lower.starts_with("data/") { 0 } else { lower.find("/data/").map(|p| p + 1)? };
    Some((at, norm[at..].to_string()))
}

/// The file to open in place of `rel` (`data/...`): the patch's, or the export one.
fn replacement(rel: &str, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let patch = PATCH.get()?;
    let lower = rel.to_ascii_lowercase();
    // the export variant's name (the texts' folder, the pictures' `_ex` pair)
    if let Some(name) = lower.strip_prefix("data/txtmessage/data_jp/txbn/")
        && let Some(made) = TRANSLATED.get().and_then(|t| t.get(name))
    {
        return Some(made.clone());
    }
    if let Some(name) = lower.strip_prefix("data/car/info_csv/")
        && let Some(made) = TUNING.get().and_then(|t| t.get(name))
    {
        return Some(made.clone());
    }
    let export = if lower.starts_with("data/txtmessage/data_jp/txbn/") {
        Some(format!("data/TxtMessage/data_ex/txbn/{}", &rel["data/txtmessage/data_jp/txbn/".len()..]))
    } else if lower.starts_with("data/flash/") && lower.ends_with("tex.pac") && !lower.ends_with("_extex.pac") {
        Some(format!("{}_extex.pac", &rel[..rel.len() - "tex.pac".len()]))
    } else if lower.starts_with("data/flash/") && lower.ends_with(".tcd") && !lower.ends_with("_ex.tcd") {
        Some(format!("{}_ex.tcd", &rel[..rel.len() - ".tcd".len()]))
    } else {
        None
    };
    if let Some(export) = export {
        let in_patch = patch.join(&export);
        if exists(&in_patch) {
            return Some(in_patch);
        }
        if exists(Path::new(&export)) {
            return Some(PathBuf::from(export));
        }
    }
    let in_patch = patch.join(rel);
    exists(&in_patch).then_some(in_patch)
}

fn redirect_str(path: &str) -> Option<String> {
    let (_, rel) = data_relative(path)?;
    let to = replacement(&rel, |p| p.is_file())?.to_string_lossy().replace('/', "\\");
    let mut logged = LOGGED.lock().unwrap();
    if !logged.contains(&rel) {
        log!("idz english: {rel} -> {to}");
        logged.push(rel);
    }
    Some(to)
}

fn redirect_a(path: *const c_char) -> Option<CString> {
    if path.is_null() {
        return None;
    }
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    CString::new(redirect_str(&p)?).ok()
}

unsafe extern "system" fn create_file_a(path: *const c_char, access: u32, share: u32, sa: *const c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let real: unsafe extern "system" fn(*const c_char, u32, u32, *const c_void, u32, u32, *mut c_void) -> *mut c_void = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    let to = redirect_a(path);
    unsafe { real(to.as_ref().map_or(path, |c| c.as_ptr()), access, share, sa, disposition, flags, template) }
}

unsafe extern "system" fn create_file_w(path: *const u16, access: u32, share: u32, sa: *const c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let real: unsafe extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, *mut c_void) -> *mut c_void = unsafe { std::mem::transmute(CREATE_FILE_W.load(Ordering::Relaxed)) };
    let to = (!path.is_null())
        .then(|| {
            let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, len) })
        })
        .and_then(|p| redirect_str(&p))
        .map(|t| t.encode_utf16().chain([0]).collect::<Vec<u16>>());
    unsafe { real(to.as_ref().map_or(path, |w| w.as_ptr()), access, share, sa, disposition, flags, template) }
}

unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut c_void {
    let real: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void = unsafe { std::mem::transmute(FOPEN.load(Ordering::Relaxed)) };
    let to = redirect_a(path);
    unsafe { real(to.as_ref().map_or(path, |c| c.as_ptr()), mode) }
}

unsafe extern "C" fn fopen_s(out: *mut *mut c_void, path: *const c_char, mode: *const c_char) -> i32 {
    let real: unsafe extern "C" fn(*mut *mut c_void, *const c_char, *const c_char) -> i32 = unsafe { std::mem::transmute(FOPEN_S.load(Ordering::Relaxed)) };
    let to = redirect_a(path);
    unsafe { real(out, to.as_ref().map_or(path, |c| c.as_ptr()), mode) }
}

/// `data`'s texts with their Japanese translated, and how many.
fn translate_txbn(data: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let mut file = txbn::Txbn::parse(data)?;
    let (mut done, mut left) = (0, 0);
    for e in &mut file.entries {
        if txbn::has_japanese(&e.text) {
            match strings::translate(&e.text) {
                Some(en) => {
                    e.text = en.to_string();
                    done += 1;
                }
                None => left += 1,
            }
        }
    }
    Ok((file.serialize(), done, left))
}

/// Makes each Japanese text file's translated export one (`TRANSLATED_DIR`).
fn make_translated() {
    let names: Vec<String> = match std::fs::read_dir(TXBN_JP) {
        Ok(dir) => dir.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with(".txbn")).collect(),
        Err(e) => {
            log!("idz english: {TXBN_JP}: {e}: texts not translated");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(TRANSLATED_DIR) {
        log!("idz english: {TRANSLATED_DIR}: {e}: texts not translated");
        return;
    }
    let mut made = HashMap::new();
    let (mut done, mut left) = (0, 0);
    for name in names {
        let Some(source) = replacement(&format!("{TXBN_JP}/{name}"), |p| p.is_file()) else { continue };
        let result = std::fs::read(&source).map_err(|e| e.to_string()).and_then(|d| translate_txbn(&d));
        match result {
            Ok((out, d, l)) => {
                let to = Path::new(TRANSLATED_DIR).join(&name);
                if std::fs::read(&to).ok().as_ref() != Some(&out)
                    && let Err(e) = std::fs::write(&to, &out)
                {
                    log!("idz english: {}: {e}", to.display());
                    continue;
                }
                done += d;
                left += l;
                made.insert(name.to_ascii_lowercase(), to);
            }
            Err(e) => log!("idz english: {}: {e}: not translated", source.display()),
        }
    }
    log!("idz english: {} text files, {done} texts translated, {left} still in Japanese", made.len());
    let _ = TRANSLATED.set(made);
}

/// `text` in full width (the font of the cars' tuning texts has no half width letters), its
/// `$n` line breaks and `%` format codes kept.
fn full_width(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' | '%' if chars.peek().is_some_and(char::is_ascii_alphabetic) => {
                out.push(c);
                out.push(chars.next().unwrap_or_default());
            }
            ' ' => out.push('\u{3000}'),
            '!'..='~' => out.push(char::from_u32(c as u32 - 0x21 + 0xff01).unwrap_or(c)),
            _ => out.push(c),
        }
    }
    out
}

/// A tuning file (one CSV line per part; title, description, question and message the texts)
/// with its texts' Latin letters in full width.
fn full_width_tuning(text: &str) -> String {
    const TEXTS: [usize; 4] = [3, 4, 5, 6];
    text.split_inclusive('\n')
        .map(|line| {
            if line.starts_with("//") {
                return line.to_string();
            }
            line.split(',')
                .enumerate()
                .map(|(i, f)| {
                    if !TEXTS.contains(&i) {
                        return f.to_string();
                    }
                    // a Japanese text: its translation (Ver.2's files have no English)
                    let english = if txbn::has_japanese(f) { strings::translate(f).unwrap_or(f) } else { f };
                    full_width(english)
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect()
}

/// `data` as Shift-JIS text (the system's code page 932).
fn from_sjis(data: &[u8]) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if data.is_empty() {
        return String::new();
    }
    let n = unsafe { MultiByteToWideChar(932, 0, data.as_ptr(), data.len() as i32, std::ptr::null_mut(), 0) };
    let mut wide = vec![0u16; n.max(0) as usize];
    let n = unsafe { MultiByteToWideChar(932, 0, data.as_ptr(), data.len() as i32, wide.as_mut_ptr(), n) };
    String::from_utf16_lossy(&wide[..n.max(0) as usize])
}

/// `text` in Shift-JIS (the system's code page 932).
fn to_sjis(text: &str) -> Vec<u8> {
    use windows_sys::Win32::Globalization::WideCharToMultiByte;
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Vec::new();
    }
    let n = unsafe { WideCharToMultiByte(932, 0, wide.as_ptr(), wide.len() as i32, std::ptr::null_mut(), 0, std::ptr::null(), std::ptr::null_mut()) };
    let mut out = vec![0u8; n.max(0) as usize];
    let n = unsafe { WideCharToMultiByte(932, 0, wide.as_ptr(), wide.len() as i32, out.as_mut_ptr(), n, std::ptr::null(), std::ptr::null_mut()) };
    out.truncate(n.max(0) as usize);
    out
}

/// Makes the cars' tuning texts the game reads (the patch's, else the dump's; UTF-8 or Shift-JIS,
/// whatever their name says: some of Ver.2's `utf8_jp_*` are Shift-JIS) translated and in full
/// width (`TUNING_DIR`).
fn make_tuning() {
    let Ok(dir) = std::fs::read_dir(TUNING_JP) else { return };
    if std::fs::create_dir_all(TUNING_DIR).is_err() {
        return;
    }
    let mut made = HashMap::new();
    for name in dir.flatten().map(|f| f.file_name().to_string_lossy().into_owned()).filter(|n| n.to_ascii_lowercase().ends_with("_car_tuning_info.txt")) {
        let source = PATCH.get().map(|p| p.join(TUNING_JP).join(&name)).filter(|p| p.is_file()).unwrap_or_else(|| Path::new(TUNING_JP).join(&name));
        let Ok(data) = std::fs::read(&source) else { continue };
        let out = match String::from_utf8(data) {
            Ok(text) => full_width_tuning(&text).into_bytes(),
            Err(e) => to_sjis(&full_width_tuning(&from_sjis(e.as_bytes()))),
        };
        let to = Path::new(TUNING_DIR).join(&name);
        if std::fs::read(&to).ok().as_ref() != Some(&out) && std::fs::write(&to, &out).is_err() {
            continue;
        }
        made.insert(name.to_ascii_lowercase(), to);
    }
    log!("idz english: {} tuning files in full width", made.len());
    let _ = TUNING.set(made);
}

pub(crate) fn init() {
    if std::env::var("WAL_ALLS_IDZ_ENGLISH").map_or(true, |v| v.trim() != "1") {
        return;
    }
    // the patch's folder (`WAL_ALLS_IDZ_ENGLISH_DIR`, from the game's folder): the dump's
    // `english\` (`..\..\english` from version 1.30's `app\package\`)
    let dir = PathBuf::from(std::env::var("WAL_ALLS_IDZ_ENGLISH_DIR").map_or_else(|_| "..\\..\\english".to_string(), |d| d.trim().to_string()));
    if !dir.join("data").is_dir() {
        log!("idz english: no data folder in {}: export texts and pictures only", dir.display());
    }
    let _ = PATCH.set(dir.join(""));
    make_translated();
    make_tuning();
    let hooks: [(&str, &str, usize, &AtomicUsize); 4] = [
        ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize, &CREATE_FILE_A),
        ("kernel32.dll", "CreateFileW", create_file_w as *const () as usize, &CREATE_FILE_W),
        ("msvcr110.dll", "fopen", fopen as *const () as usize, &FOPEN),
        ("msvcr110.dll", "fopen_s", fopen_s as *const () as usize, &FOPEN_S),
    ];
    for (dll, name, f, slot) in hooks {
        match unsafe { iat::hook(dll, name, f) } {
            Some(o) => slot.store(o, Ordering::Relaxed),
            None => log!("idz english: {name} not imported"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping() {
        let _ = PATCH.set(PathBuf::from("P"));
        let have = |files: &'static [&'static str]| move |p: &Path| files.iter().any(|f| Path::new(f) == p);
        let files = have(&["P/data/TxtMessage/data_ex/txbn/advice.txbn", "data/TxtMessage/data_ex/txbn/cmn.txbn", "data/flash/data_jp/menu/sel_start/sel_start_extex.pac", "P/data/flash/data_jp/common/player/playertex.pac"]);
        assert_eq!(replacement("data/TxtMessage/data_jp/txbn/advice.txbn", &files), Some(PathBuf::from("P/data/TxtMessage/data_ex/txbn/advice.txbn")));
        assert_eq!(replacement("data/TxtMessage/data_jp/txbn/cmn.txbn", &files), Some(PathBuf::from("data/TxtMessage/data_ex/txbn/cmn.txbn")));
        assert_eq!(replacement("data/flash/data_jp/menu/sel_start/sel_starttex.pac", &files), Some(PathBuf::from("data/flash/data_jp/menu/sel_start/sel_start_extex.pac")));
        assert_eq!(replacement("data/flash/data_jp/common/player/playertex.pac", &files), Some(PathBuf::from("P/data/flash/data_jp/common/player/playertex.pac")));
        assert_eq!(replacement("data/flash/data_jp/menu/sel_car/sel_cartex.pac", &files), None);
        let made = HashMap::from([("cmn.txbn".to_string(), PathBuf::from("Y/cmn.txbn"))]);
        let _ = TRANSLATED.set(made);
        assert_eq!(replacement("data/TxtMessage/data_jp/txbn/CMN.txbn", &files), Some(PathBuf::from("Y/cmn.txbn")));
        assert_eq!(data_relative("X:\\package\\data\\CAR\\a.txt").map(|r| r.1), Some("data/CAR/a.txt".into()));
    }

    #[test]
    fn tuning_full_width() {
        let line = "1,0,10,Mechanical LSD,Suppresses the wheelspin$nwhen cornering!,x,%s x,140,,0xffff,01,,-1,D_TUNE_LSD,;\n";
        assert_eq!(
            full_width_tuning(&format!("//c,d\n{line}")),
            "//c,d\n1,0,10,Ｍｅｃｈａｎｉｃａｌ\u{3000}ＬＳＤ,Ｓｕｐｐｒｅｓｓｅｓ\u{3000}ｔｈｅ\u{3000}ｗｈｅｅｌｓｐｉｎ$nｗｈｅｎ\u{3000}ｃｏｒｎｅｒｉｎｇ！,ｘ,%s\u{3000}ｘ,140,,0xffff,01,,-1,D_TUNE_LSD,;\n"
        );
    }

    #[test]
    fn translated() {
        let mut file = txbn::Txbn { entries: Vec::new() };
        for text in ["車は追加購入で全車種所有することができるぞ！", "未翻訳", "English"] {
            let mut fields = [26, 9, 0, 0, 1, 1, 0, 0xffffff, 0, u32::MAX, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            fields[10] = 1;
            file.entries.push(txbn::Entry { fields, text: text.into() });
        }
        let (out, done, left) = translate_txbn(&file.serialize()).unwrap();
        assert_eq!((done, left), (1, 1));
        let back = txbn::Txbn::parse(&out).unwrap();
        assert_eq!(back.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["You can buy extra cars and own every model!", "未翻訳", "English"]);
        assert!(translate_txbn(b"TXBN").is_err());
    }
}
