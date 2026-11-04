//! Options for one program of the game: `WAL_EXE_ENV="<name.exe>: VAR=value, VAR=value; ..."`.
//!
//! The payload runs in each program of a game (a frontend and the game it starts) with the same
//! environment. At the payload's start the variables listed for the running executable are set
//! (before the options are read): SiN's game (Quake II's engine, OpenGL) gets its scaling options,
//! its frontend (DirectDraw, scaled by Wine's display mode emulation) none.

use crate::log;

pub fn apply() {
    let Ok(spec) = std::env::var("WAL_EXE_ENV") else { return };
    let Some(exe) = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())) else {
        return;
    };
    for entry in spec.split(';') {
        let Some((name, vars)) = entry.split_once(':') else { continue };
        if name.trim().to_lowercase() != exe {
            continue;
        }
        for var in vars.split(',') {
            if let Some((k, v)) = var.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                if !k.is_empty() {
                    // SAFETY: at the payload's start, before its threads
                    unsafe { std::env::set_var(k, v) };
                    log!("exe env: {exe}: {k}={v}");
                }
            }
        }
    }
}
