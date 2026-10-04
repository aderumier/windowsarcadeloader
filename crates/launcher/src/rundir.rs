//! Game run directory: symlinks to every game file, minus the hidden ones, plus the
//! payload DLLs. The game directory itself is never modified.
//!
//! Files the game creates next to its executable land in the run directory and are kept
//! between launches; files inside the game's sub directories go to the game directory.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

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
