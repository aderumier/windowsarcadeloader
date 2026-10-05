//! Minimal file logger: the launcher sets `WAL_LOG` to a Windows path.
//!
//! The first process creates the log; processes it starts (a launcher starting the game)
//! inherit `WAL_LOG_APPEND` and append to it, their lines tagged with their executable name.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);
static TAG: OnceLock<String> = OnceLock::new();
const APPEND: &str = "WAL_LOG_APPEND";

pub(crate) fn init() {
    if let Ok(path) = std::env::var(wal_protocol::env::LOG) {
        let child = std::env::var(APPEND).is_ok();
        if child {
            let exe = std::env::current_exe().ok();
            let name = exe.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            let _ = TAG.set(format!("[{}] ", name.unwrap_or_default()));
        } else {
            let _ = File::create(&path);
            // SAFETY: payload init, before the game starts other threads
            unsafe { std::env::set_var(APPEND, "1") };
        }
        // every process appends: their lines never overwrite each other
        *FILE.lock().unwrap() = OpenOptions::new().append(true).create(true).open(path).ok();
    }
}

pub fn write(args: std::fmt::Arguments) {
    if let Some(f) = FILE.lock().unwrap().as_mut() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let tag = TAG.get().map_or("", |t| t.as_str());
        // one write per line: the processes sharing the log never mix their lines
        let line = format!("[{}.{:03}] {tag}{}\n", t.as_secs(), t.subsec_millis(), args);
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(format_args!($($arg)*)) };
}
