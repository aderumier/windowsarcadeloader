//! Game run directory: symlinks to every game file, minus the hidden ones, plus the
//! payload DLLs. The game directory itself is never modified.
//!
//! Files the game creates next to its executable land in the run directory and are kept
//! between launches; files inside the game's sub directories go to the game directory.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

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
