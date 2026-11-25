//! Street Fighter V Type Arcade in English (`WAL_SFV_LANGUAGE`, default `english`). The arcade
//! build only ships Japanese text for its cabinet screens, and its Scaleform font library is
//! `fonts_<culture>`: the English one (`fonts_en`) has no Japanese glyph, so culture `en` left
//! every arcade text as empty boxes. The culture stays `ja` (`fonts_jp`: Latin and Japanese
//! glyphs) and the Japanese texts are replaced:
//!
//! - The FText table: the game's `Game_Steam` localization (`Game.locres` in `C:\Patch\patch.pak`,
//!   else `pakchunk0-WindowsNoEditor.pak`, AES-256 entries, zlib blocks) holds an English table
//!   (the console game's) next to the Japanese one. A `ja` table is written to
//!   `Saved\WalLocalization\ja\Game.locres` with the English texts (the source hashes of the
//!   Japanese table), and the saved `Game.ini`'s `[Internationalization] LocalizationPaths`
//!   points there instead of `Game_Steam` (a plain value: 4.12's saved configs take no
//!   `!...=ClearArray`). The English table's placeholders (`ID_CMN_Char_D_Z41`, the arcade's
//!   later characters, stages and endings) and its few Japanese texts take `sfv_english.tsv`'s
//!   translation of the Japanese text.
//! - The literals: the cabinet screens' texts are UTF-16 string literals, in the executable
//!   (matchmaking, rematch, versus rules) and in `NESiCAUtilLib.dll` (the NESiCA card dialogs,
//!   the attract ticker). Each `.rdata` literal `sfv_english.tsv` translates has its code
//!   references (`lea reg, [rip+literal]`) and its relocated pointers moved to an English copy
//!   allocated next to the module (within the 2 GB of the displacements); the executable before
//!   its startup code, the DLL as it loads (`LdrRegisterDllNotification`, before its `DllMain`).
//!   The player name NG word list (`あいぬ` ... `ＺＡＭＥＮ`) is left alone.
//!
//! Texts drawn in textures (the ranking titles, the attract posters) stay Japanese.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

use aes::Aes256;
use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc, VirtualProtect};

/// Japanese text, tab, English text per line (`\n`, `\t`, `\\` escaped).
const TRANSLATIONS: &str = include_str!("sfv_english.tsv");

/// The game's pak AES-256 key.
const PAK_KEY: &[u8; 32] = b"_aS4mfZK8M5s5KWC2Lz2VsFnGKI7azgl";
const PAK_MAGIC: u32 = 0x5a6f_12e1;

pub(crate) fn english() -> bool {
    std::env::var("WAL_SFV_LANGUAGE").map_or(true, |v| !v.eq_ignore_ascii_case("japanese"))
}

fn translations() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        TRANSLATIONS
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(ja, en)| (unescape(ja), unescape(en)))
            .collect()
    })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

fn japanese_unit(u: u16) -> bool {
    matches!(u, 0x3000..=0x30ff | 0x4e00..=0x9fff | 0xff00..=0xffef)
}

fn japanese(s: &str) -> bool {
    s.encode_utf16().any(japanese_unit)
}

pub(crate) fn init(exe: usize) {
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    let saved = format!("{local}\\StreetFighterV\\Saved");
    let game_ini = format!("{saved}\\Config\\WindowsNoEditor\\Game.ini");
    if !english() {
        set_localization_path(&game_ini, None);
        log!("sfv: Japanese texts");
        return;
    }
    let dir = format!("{saved}\\WalLocalization");
    match write_table(&format!("{dir}\\ja")) {
        Ok(n) => {
            log!("sfv: English table written ({n} texts)");
            set_localization_path(&game_ini, Some(&dir.replace('\\', "/")));
        }
        Err(e) => {
            log!("sfv: English table: {e}: the game's Japanese texts");
            set_localization_path(&game_ini, None);
        }
    }
    unsafe { redirect(exe, "StreetFighterV.exe") };
    let nesica = unsafe { GetModuleHandleW(wide("NESiCAUtilLib.dll").as_ptr()) } as usize;
    if nesica != 0 {
        unsafe { redirect(nesica, "NESiCAUtilLib.dll") };
    } else {
        watch_loads();
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ---- the localization table ----

fn exe_path() -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The paks holding `Game_Steam`, the NESiCAxLive delivery first (the newer table).
fn paks() -> Vec<String> {
    let exe = exe_path();
    let drive = exe.get(..2).unwrap_or("C:").to_string();
    let mut out = vec![format!("{drive}\\Patch\\patch.pak")];
    // <root>\StreetFighterV\Binaries\Win64\StreetFighterV.exe
    let mut root = std::path::PathBuf::from(&exe);
    for _ in 0..3 {
        root.pop();
    }
    out.push(root.join("Content\\Paks\\pakchunk0-WindowsNoEditor.pak").to_string_lossy().into_owned());
    out
}

struct Entry {
    offset: u64,
    size: u64,
    blocks: Vec<(u64, u64)>,
    encrypted: bool,
}

fn read_u32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// An Unreal `FString`: a positive length is Latin-1 bytes, a negative one UTF-16 units, the
/// terminating NUL counted.
fn read_fstring(r: &mut impl Read) -> std::io::Result<String> {
    let n = read_u32(r)? as i32;
    let mut s = if n < 0 {
        let mut b = vec![0u8; (-n as usize) * 2];
        r.read_exact(&mut b)?;
        String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        let mut b = vec![0u8; n as usize];
        r.read_exact(&mut b)?;
        b.iter().map(|&c| c as char).collect()
    };
    if s.ends_with('\0') {
        s.pop();
    }
    Ok(s)
}

fn pak_entries(file: &mut std::fs::File) -> std::io::Result<(u32, Vec<(String, Entry)>)> {
    let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
    file.seek(SeekFrom::End(-44))?;
    let magic = read_u32(file)?;
    let version = read_u32(file)?;
    let index = read_u64(file)?;
    let index_size = read_u64(file)?;
    if magic != PAK_MAGIC || index_size > 256 << 20 {
        return Err(invalid("not a pak"));
    }
    file.seek(SeekFrom::Start(index))?;
    let mut buf = vec![0u8; index_size as usize];
    file.read_exact(&mut buf)?;
    let mut r = std::io::Cursor::new(buf);
    let mount = read_fstring(&mut r)?;
    let count = read_u32(&mut r)?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = read_fstring(&mut r)?;
        let offset = read_u64(&mut r)?;
        let _packed = read_u64(&mut r)?;
        let size = read_u64(&mut r)?;
        let compression = read_u32(&mut r)?;
        r.seek(SeekFrom::Current(20))?;
        let mut blocks = Vec::new();
        let mut encrypted = false;
        if version >= 3 {
            if compression != 0 {
                for _ in 0..read_u32(&mut r)? {
                    blocks.push((read_u64(&mut r)?, read_u64(&mut r)?));
                }
            }
            let mut flag = [0u8];
            r.read_exact(&mut flag)?;
            encrypted = flag[0] != 0;
            read_u32(&mut r)?;
        }
        if compression != 0 && blocks.is_empty() {
            return Err(invalid("compressed entry without blocks"));
        }
        out.push((format!("{mount}{name}"), Entry { offset, size, blocks, encrypted }));
    }
    Ok((version, out))
}

fn decrypt(data: &mut [u8]) {
    let cipher = Aes256::new(GenericArray::from_slice(PAK_KEY));
    for block in data.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
}

fn read_entry(file: &mut std::fs::File, version: u32, entry: &Entry) -> std::io::Result<Vec<u8>> {
    let invalid = |what: String| std::io::Error::new(std::io::ErrorKind::InvalidData, what);
    let padded = |n: u64| if entry.encrypted { n.div_ceil(16) * 16 } else { n };
    let mut out = Vec::with_capacity(entry.size as usize);
    if entry.blocks.is_empty() {
        // the entry's header (offset, sizes, compression, hash, version 3's flag and block size)
        let header = 8 * 3 + 4 + 20 + if version >= 3 { 5 } else { 0 };
        file.seek(SeekFrom::Start(entry.offset + header))?;
        let mut data = vec![0u8; padded(entry.size) as usize];
        file.read_exact(&mut data)?;
        if entry.encrypted {
            decrypt(&mut data);
        }
        data.truncate(entry.size as usize);
        return Ok(data);
    }
    for &(start, end) in &entry.blocks {
        // version 3's block offsets are the file's
        let len = end.checked_sub(start).ok_or_else(|| invalid("bad block".into()))?;
        file.seek(SeekFrom::Start(start))?;
        let mut data = vec![0u8; padded(len) as usize];
        file.read_exact(&mut data)?;
        if entry.encrypted {
            decrypt(&mut data);
        }
        let block = miniz_oxide::inflate::decompress_to_vec_zlib(&data[..len as usize]).map_err(|e| invalid(format!("zlib: {e:?}")))?;
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// The `Game_Steam` tables of the first pak holding them: (English, Japanese).
fn steam_tables() -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no pak");
    for pak in paks() {
        let mut file = match std::fs::File::open(&pak) {
            Ok(f) => f,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let (version, entries) = match pak_entries(&mut file) {
            Ok(e) => e,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let find = |culture: &str| entries.iter().find(|(name, _)| name.ends_with(&format!("Localization/Game_Steam/{culture}/Game.locres")));
        let (Some((_, en)), Some((_, ja))) = (find("en"), find("ja")) else { continue };
        let tables = (read_entry(&mut file, version, en)?, read_entry(&mut file, version, ja)?);
        log!("sfv: Game_Steam tables from {pak}");
        return Ok(tables);
    }
    Err(last)
}

/// (namespace, key, source hash, text) of a 4.12 `.locres` (no magic: namespaces, keys).
type Text = (String, String, u32, String);

fn parse_locres(data: &[u8]) -> std::io::Result<Vec<Text>> {
    let mut r = std::io::Cursor::new(data);
    let mut out = Vec::new();
    for _ in 0..read_u32(&mut r)? {
        let namespace = read_fstring(&mut r)?;
        for _ in 0..read_u32(&mut r)? {
            let key = read_fstring(&mut r)?;
            let hash = read_u32(&mut r)?;
            out.push((namespace.clone(), key, hash, read_fstring(&mut r)?));
        }
    }
    Ok(out)
}

fn write_fstring(out: &mut Vec<u8>, s: &str) {
    if s.is_ascii() {
        out.extend_from_slice(&(s.len() as i32 + 1).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    } else {
        let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
        out.extend_from_slice(&(-(units.len() as i32)).to_le_bytes());
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
}

/// The English table's placeholders: the key's id (`ID_CMN_Char_D_Z41`) for a text it lacks.
fn placeholder(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("ID_") && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn write_table(dir: &str) -> std::io::Result<usize> {
    let (en, ja) = steam_tables()?;
    let en = parse_locres(&en)?;
    let ja = parse_locres(&ja)?;
    let english: HashMap<(&str, &str), &str> = en.iter().map(|(n, k, _, t)| ((n.as_str(), k.as_str()), t.as_str())).collect();
    let tr = translations();
    let mut texts: Vec<(&str, &str, u32, &str)> = Vec::with_capacity(en.len());
    let mut translated = 0;
    let mut untranslated = 0;
    for (n, k, hash, text) in &ja {
        let value = match english.get(&(n.as_str(), k.as_str())) {
            Some(e) if !placeholder(e) && !japanese(e) => *e,
            _ => match tr.get(text) {
                Some(t) => {
                    translated += 1;
                    t.as_str()
                }
                None => {
                    if japanese(text) {
                        untranslated += 1;
                    }
                    text.as_str()
                }
            },
        };
        texts.push((n, k, *hash, value));
    }
    // the console game's texts the arcade table lacks
    let known: std::collections::HashSet<(&str, &str)> = ja.iter().map(|(n, k, _, _)| (n.as_str(), k.as_str())).collect();
    for (n, k, hash, text) in &en {
        if !known.contains(&(n.as_str(), k.as_str())) {
            texts.push((n, k, *hash, text));
        }
    }
    let mut namespaces: Vec<&str> = Vec::new();
    let mut by_namespace: HashMap<&str, Vec<(&str, u32, &str)>> = HashMap::new();
    for &(n, k, hash, text) in &texts {
        by_namespace
            .entry(n)
            .or_insert_with(|| {
                namespaces.push(n);
                Vec::new()
            })
            .push((k, hash, text));
    }
    let mut out = Vec::with_capacity(8 << 20);
    out.extend_from_slice(&(namespaces.len() as u32).to_le_bytes());
    for n in namespaces {
        let keys = &by_namespace[n];
        write_fstring(&mut out, n);
        out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for &(k, hash, text) in keys {
            write_fstring(&mut out, k);
            out.extend_from_slice(&hash.to_le_bytes());
            write_fstring(&mut out, text);
        }
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(format!("{dir}\\Game.locres"), out)?;
    log!("sfv: {translated} texts translated by the payload, {untranslated} left Japanese");
    Ok(texts.len())
}

/// The saved `Game.ini`'s `[Internationalization] LocalizationPaths`: the payload's table, or
/// none (the project's `Game_Steam`).
fn set_localization_path(path: &str, value: Option<&str>) {
    let ini = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    let mut section = false;
    let mut written = false;
    for line in ini.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.eq_ignore_ascii_case("[Internationalization]");
        } else if section && trimmed.trim_start_matches(['+', '-', '!']).to_ascii_lowercase().starts_with("localizationpaths=") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
        if section && trimmed.starts_with('[') {
            if let Some(v) = value {
                out.push_str(&format!("LocalizationPaths={v}\r\n"));
            }
            written = true;
        }
    }
    if !written && let Some(v) = value {
        out.push_str(&format!("\r\n[Internationalization]\r\nLocalizationPaths={v}\r\n"));
    }
    if out == ini {
        return;
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, out) {
        log!("sfv: {path}: {e}");
    }
}

// ---- the literals ----

struct Section {
    name: [u8; 8],
    rva: usize,
    size: usize,
}

unsafe fn sections(base: usize) -> (Vec<Section>, usize, (usize, usize)) {
    unsafe {
        let pe = base + (*((base + 0x3c) as *const u32)) as usize;
        let count = *((pe + 6) as *const u16) as usize;
        let optional = pe + 24;
        let optional_size = *((pe + 20) as *const u16) as usize;
        let image_size = *((optional + 56) as *const u32) as usize;
        // PE32+: the data directories from +112, the base relocations the sixth
        let relocs = (*((optional + 112 + 5 * 8) as *const u32) as usize, *((optional + 112 + 5 * 8 + 4) as *const u32) as usize);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let s = optional + optional_size + 40 * i;
            out.push(Section {
                name: *(s as *const [u8; 8]),
                size: *((s + 8) as *const u32) as usize,
                rva: *((s + 12) as *const u32) as usize,
            });
        }
        (out, image_size, relocs)
    }
}

fn named(s: &Section, name: &[u8]) -> bool {
    s.name.starts_with(name) && s.name.get(name.len()).is_none_or(|&c| c == 0)
}

/// Memory for the English copies within 2 GB of the module (its displacements).
fn alloc_near(base: usize, image_size: usize, size: usize) -> Option<usize> {
    let start = (base + image_size + 0xffff) & !0xffff;
    (0..0x4000).map(|i| start + i * 0x10000).find_map(|at| {
        let p = unsafe { VirtualAlloc(at as *const _, size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) } as usize;
        (p != 0).then_some(p)
    })
}

/// The module's `.rdata` UTF-16 literals the table translates moved to English copies.
unsafe fn redirect(base: usize, module: &str) {
    let (sections, image_size, relocs) = unsafe { sections(base) };
    let (Some(rdata), Some(text)) = (sections.iter().find(|s| named(s, b".rdata")), sections.iter().find(|s| named(s, b".text"))) else {
        log!("sfv: {module}: no .rdata / .text");
        return;
    };
    let tr = translations();
    let units = unsafe { std::slice::from_raw_parts((base + rdata.rva) as *const u16, rdata.size / 2) };
    // the NG word list's whole strings, from あいぬ to ＺＡＭＥＮ
    let whole = |s: &str| {
        let w: Vec<u16> = s.encode_utf16().collect();
        (1..units.len().saturating_sub(w.len())).find(|&i| units[i - 1] == 0 && units[i..].starts_with(&w) && units[i + w.len()] == 0)
    };
    let ng = match (whole("あいぬ"), whole("ＺＡＭＥＮ")) {
        (Some(a), Some(z)) if a < z => a..z + 8,
        _ => 0..0,
    };
    // literal rva -> its English text
    let mut found: Vec<(usize, Vec<u16>)> = Vec::new();
    let mut start = 0;
    for (i, &u) in units.iter().enumerate() {
        if u != 0 {
            continue;
        }
        let s = &units[start..i];
        let at = start;
        start = i + 1;
        if s.len() < 2 || ng.contains(&at) || !s.iter().copied().any(japanese_unit) {
            continue;
        }
        let Ok(ja) = String::from_utf16(s) else { continue };
        if let Some(en) = tr.get(&ja) {
            found.push((rdata.rva + at * 2, en.encode_utf16().chain([0]).collect()));
        }
    }
    if found.is_empty() {
        log!("sfv: {module}: no translated literal");
        return;
    }
    let total: usize = found.iter().map(|(_, en)| en.len() * 2).sum();
    let Some(block) = alloc_near(base, image_size, total) else {
        log!("sfv: {module}: no memory near the module");
        return;
    };
    let mut targets: HashMap<usize, usize> = HashMap::with_capacity(found.len());
    let mut at = block;
    for (rva, en) in &found {
        unsafe { std::ptr::copy_nonoverlapping(en.as_ptr(), at as *mut u16, en.len()) };
        targets.insert(*rva, at);
        at += en.len() * 2;
    }
    // lea r64, [rip+disp32]: REX.W 8D, ModRM mod 00 r/m 101
    let code = unsafe { std::slice::from_raw_parts((base + text.rva) as *const u8, text.size) };
    let mut code_refs = 0;
    for i in 0..code.len().saturating_sub(7) {
        if code[i] & 0xf8 != 0x48 || code[i + 1] != 0x8d || code[i + 2] & 0xc7 != 0x05 {
            continue;
        }
        let disp = i32::from_le_bytes([code[i + 3], code[i + 4], code[i + 5], code[i + 6]]) as isize;
        let next = text.rva + i + 7;
        let Some(&new) = targets.get(&(next.wrapping_add_signed(disp))) else { continue };
        let Ok(new_disp) = i32::try_from(new as isize - (base + next) as isize) else { continue };
        // each displacement's own page protection kept (the loader's entry page is writable)
        unsafe { write_protected(base + text.rva + i + 3, &new_disp.to_le_bytes()) };
        code_refs += 1;
    }
    // the relocated pointers (tables of texts)
    let mut pointers = 0;
    let (reloc_rva, reloc_size) = relocs;
    let mut off = 0;
    while reloc_rva != 0 && off + 8 <= reloc_size {
        let page = unsafe { *((base + reloc_rva + off) as *const u32) } as usize;
        let size = unsafe { *((base + reloc_rva + off + 4) as *const u32) } as usize;
        if size < 8 {
            break;
        }
        for j in 0..(size - 8) / 2 {
            let entry = unsafe { *((base + reloc_rva + off + 8 + j * 2) as *const u16) };
            // IMAGE_REL_BASED_DIR64
            if entry >> 12 != 10 {
                continue;
            }
            let p = base + page + (entry & 0xfff) as usize;
            let value = unsafe { (p as *const usize).read_unaligned() };
            let Some(&new) = value.checked_sub(base).and_then(|rva| targets.get(&rva)) else { continue };
            unsafe { write_protected(p, &new.to_le_bytes()) };
            pointers += 1;
        }
        off += size;
    }
    log!("sfv: {module}: {} literals in English ({code_refs} code references, {pointers} pointers)", found.len());
}

unsafe fn write_protected(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const _, bytes.len(), old, &mut old);
    }
}

/// `NESiCAUtilLib.dll` translated as it loads (the NESYS plugin loads it after the payload).
fn watch_loads() {
    #[repr(C)]
    struct Loaded {
        flags: u32,
        full_name: *const UnicodeString,
        base_name: *const UnicodeString,
        base: usize,
    }
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum: u16,
        buffer: *const u16,
    }
    type Callback = unsafe extern "system" fn(u32, *const Loaded, *mut core::ffi::c_void);
    type Register = unsafe extern "system" fn(u32, Callback, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> i32;
    const LOADED: u32 = 1;

    unsafe extern "system" fn notified(reason: u32, data: *const Loaded, _context: *mut core::ffi::c_void) {
        let Some(data) = (unsafe { data.as_ref() }).filter(|_| reason == LOADED) else { return };
        let Some(name) = (unsafe { data.base_name.as_ref() }) else { return };
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name.buffer, name.length as usize / 2) });
        if name.eq_ignore_ascii_case("NESiCAUtilLib.dll") {
            unsafe { redirect(data.base, "NESiCAUtilLib.dll") };
        }
    }

    let ntdll = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr().cast()) };
    let Some(register) = (unsafe { GetProcAddress(ntdll, c"LdrRegisterDllNotification".as_ptr().cast()) }) else {
        log!("sfv: no LdrRegisterDllNotification: NESiCAUtilLib.dll left Japanese");
        return;
    };
    let register: Register = unsafe { std::mem::transmute(register) };
    let mut cookie = std::ptr::null_mut();
    unsafe { register(0, notified, std::ptr::null_mut(), &mut cookie) };
}
