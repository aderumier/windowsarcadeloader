//! What Konami's ARK engine games share (Steel Chronicle Victroopers `sc.rs`, Silent Scope: Bone
//! Eater `ndd.rs`): their TrueType collections for Wine's GDI+, and the test menu settings file
//! (`testmode-v.xml`, its CRC-32 beside it).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

type AddMemoryFont = unsafe extern "system" fn(*mut c_void, *const u8, i32) -> i32;
static ORIG_ADD_FONT: AtomicUsize = AtomicUsize::new(0);

/// The game module `module`'s fonts added face by face ([`add_memory_font`]).
pub(crate) fn split_font_collections(module: usize) {
    match unsafe { iat::hook_module(module, "gdiplus.dll", "GdipPrivateAddMemoryFont", add_memory_font as *const () as usize) } {
        Some(o) => ORIG_ADD_FONT.store(o, Ordering::Relaxed),
        None => log!("ark: GdipPrivateAddMemoryFont import not found"),
    }
}

/// `GdipPrivateAddMemoryFont`: the game's fonts are TrueType collections (`.ttc`, three faces
/// each), which Wine's GDI+ does not parse (no family added: the game then reads an
/// uninitialized family count and crashes). Each face is added on its own: a copy of the
/// collection with that face's table directory moved to the start (its table offsets count from
/// the file's start, so the copy is a valid TrueType font of that face).
unsafe extern "system" fn add_memory_font(collection: *mut c_void, memory: *const u8, length: i32) -> i32 {
    let original: AddMemoryFont = unsafe { std::mem::transmute(ORIG_ADD_FONT.load(Ordering::Relaxed)) };
    if memory.is_null() || length < 16 {
        return unsafe { original(collection, memory, length) };
    }
    let data = unsafe { std::slice::from_raw_parts(memory, length as usize) };
    if &data[..4] != b"ttcf" {
        return unsafe { original(collection, memory, length) };
    }
    let be32 = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
    let faces = be32(8);
    let mut status = 0;
    for i in 0..faces {
        let Some(dir) = data.get(12 + i * 4..16 + i * 4).map(|_| be32(12 + i * 4)) else { break };
        if dir + 12 > data.len() {
            break;
        }
        let tables = u16::from_be_bytes([data[dir + 4], data[dir + 5]]) as usize;
        let size = 12 + 16 * tables;
        if dir + size > data.len() {
            break;
        }
        // kept for the collection's lifetime (GDI+ keeps a pointer to the font's memory)
        let mut font = data.to_vec();
        font.copy_within(dir..dir + size, 0);
        let font = font.leak();
        status = unsafe { original(collection, font.as_ptr(), font.len() as i32) };
    }
    log!("ark: font collection of {faces} faces added face by face (status {status})");
    status
}

/// CRC-32 (IEEE), the checksum of `testmode-v.crc`.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Sets `<name>...<current __type="s32">value</current>` in the test menu settings.
pub(crate) fn set_current(xml: &mut String, name: &str, value: i32) -> bool {
    let open = format!("<{name}>");
    let Some(start) = xml.find(&open) else { return false };
    let Some(end) = xml[start..].find(&format!("</{name}>")).map(|e| start + e) else { return false };
    let tag = "<current __type=\"s32\">";
    let Some(cur) = xml[start..end].find(tag).map(|c| start + c + tag.len()) else { return false };
    let Some(stop) = xml[cur..end].find('<').map(|e| cur + e) else { return false };
    xml.replace_range(cur..stop, &value.to_string());
    true
}

/// The test menu settings at `xml_path` (and its CRC-32 at `crc_path`) changed by `edit`, on
/// the text read as Latin-1 (Shift-JIS, but the tags are ASCII), the clock marked set and free
/// play from `WAL_KONAMI_FREEPLAY` (`1`: on, `0`: off, unset: the dump's). True when it was
/// changed and written back.
pub(crate) fn edit_settings(xml_path: &str, crc_path: &str, edit: impl FnOnce(&mut String)) -> bool {
    let Ok(text) = std::fs::read(xml_path) else {
        log!("ark: no {xml_path}, settings kept");
        return false;
    };
    let mut xml: String = text.iter().map(|&b| b as char).collect();
    edit(&mut xml);
    // the operator clock set (the PC's clock; unset: 5-2503-0000 "set the clock")
    xml = xml.replace("<is_set __type=\"s32\">0</is_set>", "<is_set __type=\"s32\">1</is_set>");
    match std::env::var("WAL_KONAMI_FREEPLAY").ok().as_deref().map(str::trim) {
        Some("1") => _ = set_current(&mut xml, "freeplay", 1),
        Some("0") => _ = set_current(&mut xml, "freeplay", 0),
        _ => {}
    }
    let bytes: Vec<u8> = xml.chars().map(|c| c as u8).collect();
    if bytes == text {
        return false;
    }
    let crc = crc32(&bytes);
    std::fs::write(xml_path, &bytes).is_ok() && std::fs::write(crc_path, crc.to_le_bytes()).is_ok()
}
