//! The players' data, in the server's data directory: `users/<user id>` (a user record) and
//! `cars/<car id>` (a car record), protocol buffers messages of this server.
//!
//! User record: 1 user id, 2 card chip id, 3 access code, 4 maxi gold, 5 confirmed tutorials
//! (bits), 6 600 HP cards, 7 car ids (the player's order), 8 shop grade, 9 total maxi gold.
//!
//! Car record: the fields of `LoadCarResponse` but its error (2 the car, 3 tuning points,
//! 4 odometer, 5 play count, 8 the car's settings, the story / versus / ghost battle
//! counters...): `load_car` answers it as stored, the game's updates are merged into it.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::pb::Message;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// One request changes the data at a time.
static LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn init(dir: &str) {
    let dir = PathBuf::from(dir);
    for sub in ["users", "cars"] {
        if let Err(e) = std::fs::create_dir_all(dir.join(sub)) {
            crate::log(&format!("wm5srv: {}: {e}", dir.join(sub).display()));
        }
    }
    let _ = DIR.set(dir);
}

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn path(kind: &str, id: u64) -> PathBuf {
    DIR.get().cloned().unwrap_or_default().join(kind).join(id.to_string())
}

fn ids(kind: &str) -> Vec<u64> {
    let dir = DIR.get().cloned().unwrap_or_default().join(kind);
    let mut ids: Vec<u64> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

pub(crate) fn load(kind: &str, id: u64) -> Option<Message> {
    Message::parse(&std::fs::read(path(kind, id)).ok()?)
}

pub(crate) fn save(kind: &str, id: u64, record: &Message) {
    if let Err(e) = std::fs::write(path(kind, id), record.encode()) {
        crate::log(&format!("wm5srv: {kind}/{id}: {e}"));
    }
}

/// A new id of `kind` (1 + the largest).
pub(crate) fn new_id(kind: &str) -> u64 {
    ids(kind).last().copied().unwrap_or(0) + 1
}

/// The user of an access code.
pub(crate) fn find_user(access_code: &str) -> Option<(u64, Message)> {
    ids("users").into_iter().find_map(|id| {
        let user = load("users", id)?;
        (user.string(3).as_deref() == Some(access_code)).then_some((id, user))
    })
}
