//! The game server of BlazBlue: Cross Tag Battle (Arc System Works' own, not SEGA's): WinINet
//! requests to `<base>/<name>` (base `https://api.bbtag.blazblue.jp/api/v1`, patched by the
//! profile to `http://127.0.0.1/api/v1`, the payload's title server, [`crate::apms`]).
//!
//! After a card is read: `GET /prod/150/config.json` (`base_url`: the base of the next requests),
//! `/IsUserCreated` (`result`, a JSON bool), `/session` (`{"code":"<card>:<access code>"}`,
//! answers `sid`, sent back as `X-SID`), `/GetUserId` (`user_id`), `/GetUserData`: the player's
//! data, read raw (one 0x314c-byte block; the game checks its magic `0xfc` and checksums, else a
//! new player). `/SetMyData` (octet-stream) sends that block back after a change: stored as
//! `apms/bb-<card>.bin`. `/LogEvent` and `/EndPlay` (statistics): an empty answer.
//!
//! The block carries the card's lock: playing (`+0x32c` > 0) less than 900 s after the time at
//! `+0x324` is "this Aime is in use on another cabinet" (`0x5f3fd7`). A play ended by closing the
//! game stays locked for those 15 minutes: one cabinet here, the block is answered unlocked.

use wal_payload_common::log;

/// The player's data block.
const DATA_SIZE: usize = 0x314c;

/// The card's code as a file name part (digits only).
fn card(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

fn data_path(card: &str) -> String {
    format!("apms/bb-{card}.bin")
}

/// The block with its lock off (0, as the end of a play saves it), checksums updated.
fn unlocked(mut data: Vec<u8>) -> Vec<u8> {
    const LOCK: usize = 0x32c;
    data[LOCK..LOCK + 4].copy_from_slice(&0u32.to_le_bytes());
    let sum8 = data[4..].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    let sum16 = data[4..].chunks_exact(2).fold(0u16, |sum, w| sum.wrapping_add(u16::from_le_bytes([w[0], w[1]])));
    data[1] = sum8;
    data[2..4].copy_from_slice(&sum16.to_le_bytes());
    data
}

/// A string member of a flat JSON object.
fn member(json: &str, key: &str) -> String {
    let pattern = format!("\"{key}\"");
    json.find(&pattern)
        .map(|i| &json[i + pattern.len()..])
        .and_then(|rest| rest.trim_start().strip_prefix(':'))
        .and_then(|rest| rest.trim_start().strip_prefix('"'))
        .and_then(|rest| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}

/// A request's response: content type and body.
pub(crate) fn reply(method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> (&'static str, Vec<u8>) {
    let api = path.strip_prefix("/api/v1").unwrap_or(path);
    let json = String::from_utf8_lossy(body);
    let sid = headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("x-sid")).map(|(_, value)| card(value)).unwrap_or_default();
    match api {
        "/GetUserData" => {
            let id = card(&member(&json, "user_id"));
            // a new card: zeros (the block of the last player is still in the game's buffer)
            let data = std::fs::read(data_path(&id)).ok().filter(|d| d.len() == DATA_SIZE);
            log!("bb: GetUserData {id}: {}", if data.is_some() { "known player" } else { "new player" });
            return ("application/octet-stream", data.map_or_else(|| vec![0; DATA_SIZE], unlocked));
        }
        "/SetMyData" => {
            if !sid.is_empty() && body.len() == DATA_SIZE {
                let _ = std::fs::create_dir_all("apms");
                match std::fs::write(data_path(&sid), body) {
                    Ok(()) => log!("bb: SetMyData {sid}: saved"),
                    Err(e) => log!("bb: SetMyData {sid}: {e}"),
                }
            } else {
                log!("bb: SetMyData {sid}: {} bytes, not saved", body.len());
            }
            return ("application/json", b"{}".to_vec());
        }
        _ => {}
    }
    log!("bb: {method} {api} {json}");
    let answer = match api {
        "/prod/150/config.json" => r#"{"base_url":"http://127.0.0.1/api/v1"}"#.to_string(),
        "/IsUserCreated" => r#"{"result":true}"#.to_string(),
        // "<card>:<access code>": the card is the session id and the user id
        "/session" => format!(r#"{{"sid":"{}"}}"#, card(member(&json, "code").split(':').next().unwrap_or(""))),
        "/GetUserId" => format!(r#"{{"user_id":"{}"}}"#, card(&member(&json, "platform_user_id"))),
        _ => "{}".to_string(),
    };
    ("application/json", answer.into_bytes())
}
