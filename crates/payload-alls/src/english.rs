//! Sega World Drivers Championship's community English translation (`WAL_SWDC_ENGLISH=1`): the
//! release folder ("SWDC ENGLISH", `manifest.json` + `payload/`) copied into the game's folder,
//! any name. Its manifest lists replacement files from the game's `Todoroki\Content`: a patch
//! pak (`Paks\Todoroki-English_P.pak`: Unreal reads a `_P` pak before the others; the menus,
//! messages and textures), the voices' sound banks and the promotional movie.
//!
//! Applied at the game's start as the release's installer does: each original checked against
//! its SHA-256 (another build is left alone), moved to `Content\WAL-English-backup\`, the
//! translation's file copied in its place; the option off puts the originals back and removes
//! the pak. Already applied files (their size, the backup present) are not copied again.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wal_payload_common::json::Json;
use wal_payload_common::log;

const BACKUP: &str = "WAL-English-backup";

struct Entry {
    path: String,
    payload: String,
    bytes: u64,
    before: Option<String>,
}

/// The game's `Todoroki\Content` (`Binaries\Win64\<exe>` -> `Content`) and the dump's root (the
/// run directory's: its entries link the dump's).
fn folders() -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let project = exe.parent()?.parent()?.parent()?;
    // <root>\App\Todoroki\WindowsNoEditor\Todoroki
    let root = project.parent()?.parent()?.parent()?.parent()?;
    Some((project.join("Content"), root.to_path_buf()))
}

/// The translation's folder in the dump's root and its manifest's entries.
fn release(root: &Path) -> Option<(PathBuf, Vec<Entry>)> {
    for dir in fs::read_dir(root).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Some(manifest) = fs::read_to_string(dir.join("manifest.json")).ok().and_then(|t| Json::parse(t.trim_start_matches('\u{feff}'))) else { continue };
        if manifest.get("OriginalPakSHA256").is_none() {
            continue;
        }
        let entries = manifest
            .a("Files")
            .iter()
            .map(|f| Entry {
                path: f.s("Path").replace('/', "\\"),
                payload: f.s("Payload").replace('/', "\\"),
                bytes: f.u("Bytes"),
                before: f.get("Before").and_then(Json::as_str).map(str::to_ascii_lowercase),
            })
            .collect();
        return Some((dir, entries));
    }
    None
}

fn sha256(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).ok()?;
    Some(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.len())
}

fn apply(content: &Path, dir: &Path, entries: &[Entry]) {
    let backup = content.join(BACKUP);
    let mut copied = 0;
    for e in entries {
        let target = content.join(&e.path);
        let saved = backup.join(&e.path);
        let done = size(&target) == Some(e.bytes) && (e.before.is_none() || saved.exists());
        if done {
            continue;
        }
        if let Some(before) = &e.before {
            if !saved.exists() {
                if sha256(&target).as_deref() != Some(before.as_str()) {
                    log!("english: {}: not the translated build's, left as is", e.path);
                    continue;
                }
                let _ = fs::create_dir_all(saved.parent().unwrap());
                if let Err(err) = fs::rename(&target, &saved) {
                    log!("english: {}: backup: {err}", e.path);
                    continue;
                }
            }
        }
        if let Err(err) = fs::copy(dir.join(&e.payload), &target) {
            log!("english: {}: {err}", e.path);
            continue;
        }
        copied += 1;
    }
    log!("english: translation on ({copied} files copied, {} in all)", entries.len());
}

fn remove(content: &Path, entries: &[Entry]) {
    let backup = content.join(BACKUP);
    let mut restored = 0;
    for e in entries {
        let target = content.join(&e.path);
        let saved = backup.join(&e.path);
        if e.before.is_none() {
            restored += fs::remove_file(&target).is_ok() as usize;
        } else if saved.exists() {
            let _ = fs::remove_file(&target);
            if let Err(err) = fs::rename(&saved, &target) {
                log!("english: {}: restore: {err}", e.path);
                continue;
            }
            restored += 1;
        }
    }
    if restored > 0 {
        log!("english: translation off ({restored} originals restored)");
    }
}

pub(crate) fn init() {
    let on = std::env::var("WAL_SWDC_ENGLISH").is_ok_and(|v| v.trim() == "1");
    let Some((content, root)) = folders() else { return };
    let Some((dir, entries)) = release(&root) else {
        if on {
            log!("english: no translation folder (manifest.json) in {}", root.display());
        }
        return;
    };
    if on { apply(&content, &dir, &entries) } else { remove(&content, &entries) }
}
