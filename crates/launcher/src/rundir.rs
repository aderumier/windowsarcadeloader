//! Game run directory: symlinks to every game file, minus the hidden ones, plus the
//! payload DLLs. The game directory itself is never modified.
//!
//! Files the game creates next to its executable land in the run directory and are kept
//! between launches; files inside the game's sub directories go to the game directory.
//!
//! Sub directories holding the executable or a payload are real directories built the same
//! way (links plus payloads), the other ones are links.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Legacy names of the `D:` data folder in existing dumps, moved to the current one on start.
const LEGACY_DATA_DIRS: [&str; 2] = [concat!("Open", "Parrot"), concat!("Tekno", "Parrot")];

/// Moves the legacy data folders of `game_dir` into `game_dir/<name>`: a legacy folder becomes
/// `<name>` when that does not exist, the others are merged in without overwriting anything
/// (a file already present stays in its legacy folder, reported). Empty legacy folders are
/// removed.
pub fn migrate_data_dir(game_dir: &Path, name: &str) -> Result<()> {
    let target = game_dir.join(name);
    for legacy in LEGACY_DATA_DIRS {
        let old = game_dir.join(legacy);
        if !old.is_dir() || old.is_symlink() {
            continue;
        }
        let has_files = fs::read_dir(&old)?.next().is_some();
        if !target.exists() && has_files {
            fs::rename(&old, &target).with_context(|| format!("renaming {} to {name}", old.display()))?;
            eprintln!("launcher: data folder {legacy} renamed to {name}");
            continue;
        }
        fs::create_dir_all(&target)?;
        merge(&old, &target)?;
        if fs::read_dir(&old)?.next().is_none() {
            fs::remove_dir(&old)?;
        } else {
            eprintln!("launcher: {} kept: files also present in {name}", old.display());
        }
    }
    Ok(())
}

/// Moves every file of `from` into `to` unless `to` has it; empty folders are removed.
fn merge(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from)?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() && !entry.file_type()?.is_symlink() {
            if !dst.exists() {
                fs::rename(&src, &dst)?;
                continue;
            }
            if dst.is_dir() {
                merge(&src, &dst)?;
                if fs::read_dir(&src)?.next().is_none() {
                    fs::remove_dir(&src)?;
                }
            }
        } else if !dst.exists() {
            fs::rename(&src, &dst)?;
        }
    }
    Ok(())
}

pub fn build(run_dir: &Path, game_dir: &Path, hide: &[String], payloads: &[(&Path, &str)]) -> Result<()> {
    fs::create_dir_all(run_dir).with_context(|| format!("creating {}", run_dir.display()))?;
    // previous links, the game files may have changed
    for entry in fs::read_dir(run_dir)?.flatten() {
        if entry.file_type()?.is_symlink() {
            fs::remove_file(entry.path())?;
        }
    }
    let excluded = |name: &str| {
        hide.iter().any(|h| h.eq_ignore_ascii_case(name)) || payloads.iter().any(|(_, n)| n.eq_ignore_ascii_case(name))
    };
    for entry in fs::read_dir(game_dir).with_context(|| format!("reading {}", game_dir.display()))?.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if excluded(&name_str) {
            continue;
        }
        let dst = run_dir.join(&name);
        if fs::symlink_metadata(&dst).is_ok() {
            eprintln!("rundir: keeping {} created by the game", dst.display());
            continue;
        }
        std::os::unix::fs::symlink(entry.path(), &dst).with_context(|| format!("linking {}", dst.display()))?;
    }
    for (src, name) in payloads {
        let dst = run_dir.join(name);
        let _ = fs::remove_file(&dst);
        fs::copy(src, &dst).with_context(|| format!("installing {}", dst.display()))?;
    }
    Ok(())
}

/// Builds the run directory of the game root `game_dir`: `real` (relative paths: the
/// executable's folder, payload folders) are real directories down to them, and each payload
/// `(source, path relative to the game root)` is installed in its folder.
pub fn build_tree(run_dir: &Path, game_dir: &Path, hide: &[String], real: &[PathBuf], payloads: &[(&Path, PathBuf)]) -> Result<()> {
    // every folder on the way to a real one or to a payload
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let parents = payloads.iter().filter_map(|(_, p)| p.parent().map(Path::to_path_buf));
    for d in real.iter().cloned().chain(parents) {
        let mut cur = PathBuf::new();
        for part in d.iter() {
            cur.push(part);
            dirs.insert(cur.clone());
        }
    }
    let installed: BTreeSet<PathBuf> = payloads.iter().map(|(_, p)| p.clone()).collect();
    remove_stale(run_dir, &installed, &dirs)?;
    level(run_dir, game_dir, Path::new(""), hide, &dirs, payloads)?;
    let list: String = installed.iter().map(|p| format!("{}\n", p.display())).collect();
    fs::write(run_dir.join(INSTALLED), list).with_context(|| format!("writing {}", run_dir.join(INSTALLED).display()))
}

/// Files installed by the previous build (payloads, profile files), one path per line.
const INSTALLED: &str = ".wal-installed";

/// Removes what the previous build installed and this one does not (a profile file dropped
/// from the profile would otherwise stay, kept as a file of the game), then the real folders
/// no longer needed that hold nothing of the game's (links only): they become links again.
fn remove_stale(run_dir: &Path, installed: &BTreeSet<PathBuf>, dirs: &BTreeSet<PathBuf>) -> Result<()> {
    let Ok(list) = fs::read_to_string(run_dir.join(INSTALLED)) else { return Ok(()) };
    for rel in list.lines().filter(|l| !l.is_empty()).map(PathBuf::from) {
        if installed.contains(&rel) {
            continue;
        }
        if fs::symlink_metadata(run_dir.join(&rel)).is_ok_and(|m| m.is_file()) {
            fs::remove_file(run_dir.join(&rel))?;
            eprintln!("rundir: removed {}, no longer installed", rel.display());
        }
        // its folders, deepest first
        for dir in rel.ancestors().skip(1).filter(|d| !d.as_os_str().is_empty()) {
            let path = run_dir.join(dir);
            if dirs.contains(dir) || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) || !links_only(&path)? {
                break;
            }
            fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// The folder holds links and folders of links only.
fn links_only(dir: &Path) -> Result<bool> {
    for entry in fs::read_dir(dir)?.flatten() {
        let kind = entry.file_type()?;
        if !kind.is_symlink() && !(kind.is_dir() && links_only(&entry.path())?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn level(run_dir: &Path, game_dir: &Path, rel: &Path, hide: &[String], dirs: &BTreeSet<PathBuf>, payloads: &[(&Path, PathBuf)]) -> Result<()> {
    let children: Vec<&PathBuf> = dirs.iter().filter(|d| d.parent() == Some(rel)).collect();
    let mut level_hide = hide.to_vec();
    level_hide.extend(children.iter().filter_map(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()));
    let here: Vec<(&Path, &str)> = payloads
        .iter()
        .filter(|(_, p)| p.parent().unwrap_or(Path::new("")) == rel)
        .filter_map(|(src, p)| Some((*src, p.file_name()?.to_str()?)))
        .collect();
    let dst = run_dir.join(rel);
    // a real directory replaces a link left by a previous layout
    if fs::symlink_metadata(&dst).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::remove_file(&dst)?;
    }
    build(&dst, &game_dir.join(rel), &level_hide, &here)?;
    for child in children {
        level(run_dir, game_dir, child, hide, dirs, payloads)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = LEGACY_DATA_DIRS[0];
    const B: &str = LEGACY_DATA_DIRS[1];

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wal-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn renames_and_merges_legacy_data() {
        let g = tmp("merge");
        fs::create_dir_all(g.join(A).join("sub")).unwrap();
        fs::write(g.join(A).join("save.bin"), "op").unwrap();
        fs::write(g.join(A).join("sub/a"), "a").unwrap();
        fs::create_dir_all(g.join(B).join("sub")).unwrap();
        fs::write(g.join(B).join("save.bin"), "tp").unwrap();
        fs::write(g.join(B).join("sub/b"), "b").unwrap();
        fs::write(g.join(B).join("news.png"), "n").unwrap();
        migrate_data_dir(&g, "WindowsLoader").unwrap();
        let w = g.join("WindowsLoader");
        assert_eq!(fs::read_to_string(w.join("save.bin")).unwrap(), "op");
        assert_eq!(fs::read_to_string(w.join("sub/a")).unwrap(), "a");
        assert_eq!(fs::read_to_string(w.join("sub/b")).unwrap(), "b");
        assert_eq!(fs::read_to_string(w.join("news.png")).unwrap(), "n");
        assert!(!g.join(A).exists());
        // the conflicting file stays in its legacy folder
        assert_eq!(fs::read_to_string(g.join(B).join("save.bin")).unwrap(), "tp");
        fs::remove_dir_all(&g).unwrap();
    }

    #[test]
    fn dropped_profile_file_removed() {
        let t = tmp("stale");
        let (game, run, src) = (t.join("game"), t.join("run"), t.join("Settings.sw"));
        fs::create_dir_all(game.join("Data/Loader")).unwrap();
        fs::write(game.join("Data/Loader/Settings.sw"), "game").unwrap();
        fs::write(game.join("game.exe"), "").unwrap();
        fs::write(&src, "profile").unwrap();
        let file = PathBuf::from("Data/Loader/Settings.sw");
        build_tree(&run, &game, &[], &[PathBuf::new()], &[(src.as_path(), file.clone())]).unwrap();
        assert_eq!(fs::read_to_string(run.join(&file)).unwrap(), "profile");
        assert!(!run.join("Data").is_symlink());
        build_tree(&run, &game, &[], &[PathBuf::new()], &[]).unwrap();
        assert_eq!(fs::read_to_string(run.join(&file)).unwrap(), "game");
        assert!(run.join("Data").is_symlink());
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn empty_legacy_folder_removed() {
        let g = tmp("empty");
        fs::create_dir_all(g.join(A)).unwrap();
        fs::create_dir_all(g.join(B)).unwrap();
        fs::write(g.join(B).join("x"), "x").unwrap();
        migrate_data_dir(&g, "WindowsLoader").unwrap();
        assert_eq!(fs::read_to_string(g.join("WindowsLoader/x")).unwrap(), "x");
        assert!(!g.join(A).exists() && !g.join(B).exists());
        fs::remove_dir_all(&g).unwrap();
    }
}

/// Copies the PE executable `src` to `dst` (replacing the link) without the DYNAMIC_BASE flag:
/// Wine then maps it at its preferred base instead of relocating it.
pub fn copy_fixed_base(src: &Path, dst: &Path) -> Result<()> {
    let mut data = fs::read(src).with_context(|| format!("reading {}", src.display()))?;
    let pe = data.get(0x3C..0x40).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).context("not a PE file")?;
    // DllCharacteristics: optional header (PE + 24) + 70, same offset in PE32 and PE32+
    let at = pe + 24 + 70;
    if data.get(pe..pe + 4) != Some(b"PE\0\0") || data.len() < at + 2 {
        bail!("{}: not a PE file", src.display());
    }
    let flags = u16::from_le_bytes([data[at], data[at + 1]]) & !0x0040;
    data[at..at + 2].copy_from_slice(&flags.to_le_bytes());
    let _ = fs::remove_file(dst);
    fs::write(dst, data).with_context(|| format!("writing {}", dst.display()))?;
    eprintln!("launcher: {} loads at its preferred base", dst.display());
    Ok(())
}
