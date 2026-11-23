//! Puzzle & Dragons Battle Tournament's title server (`/puzzle/game/...`, `/puzzle/helper/...`):
//! the game posts `data=<json>` through NESYS (`nesys.rs` 0x27, its request table at +0x544AF8);
//! the reply body is a result code line (1: OK; 9002 and 9012..9023 the server's errors, the
//! checker +0x5DD70), then the JSON its parser of that request reads.
//!
//! The players themselves are NESYS cards (`nesys.rs`: registration, the JSON save read as
//! type 0, written by the card insert and updates). Every title server request is answered OK
//! with an empty object: the shop registration (`registTenpo`), its error reports, the
//! version and update checks, the system parameters, card locks, play histories, the NG word
//! check of a new name (accepted), helpers, rankings and missions (none: the game's own
//! helpers), the shop's items.

use wal_payload_common::log;

/// The reply body of a request (`None`: not a title server path: the game's GET of
/// `/dist/SimpleVSTeamMasterInfo.csv`, an update of its own data's table, answered 404).
pub(crate) fn reply(path: &str, body: &[u8]) -> Option<Vec<u8>> {
    if !path.starts_with("/puzzle/") {
        return None;
    }
    let body = String::from_utf8_lossy(body);
    let data = body.strip_prefix("data=").unwrap_or(&body).trim_end();
    log!("pdserver: {path} {data}");
    // the end of a credit: the card's lock released
    if path == "/puzzle/game/unlockCard" {
        crate::rfid::take_off();
    }
    Some(b"1\n{}".to_vec())
}
