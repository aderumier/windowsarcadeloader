//! Minimal file logger: the launcher sets `WAL_LOG` to a Windows path.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);

pub(crate) fn init() {
    if let Ok(path) = std::env::var(wal_protocol::env::LOG) {
        *FILE.lock().unwrap() = File::create(path).ok();
    }
}

pub fn write(args: std::fmt::Arguments) {
    if let Some(f) = FILE.lock().unwrap().as_mut() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let _ = writeln!(f, "[{}.{:03}] {}", t.as_secs(), t.subsec_millis(), args);
        let _ = f.flush();
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(format_args!($($arg)*)) };
}
