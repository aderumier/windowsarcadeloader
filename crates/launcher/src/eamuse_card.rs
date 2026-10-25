//! The player's e-Amusement card, machine-wide (`userprofiles/eamuse-card.yaml`, edited in the
//! GUI): every Konami game reads this card (`WAL_KONAMI_CARD_ID`) and, with a PIN set
//! (`WAL_KONAMI_CARD_PIN`), the loader's e-Amusement server registers it with that PIN (the
//! game then only asks the PIN; Metal Gear Arcade's reader types it). Each game keeps its own
//! progress on it (its dump's `dev/nvram/easrv/`). A game profile's own `env` values win.
//! Player 2's card (`card2_id`, `pin2`; `WAL_KONAMI_CARD2_ID`, `WAL_KONAMI_CARD2_PIN`), when
//! set, is in player 2's reader (Hello! Pop'n Music: a reader per player).

use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::USER_PROFILES;

const FILE: &str = "eamuse-card.yaml";
/// The payload's card id when none is set.
pub const DEFAULT_ID: &str = "E004010027104B1D";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// 16 hex digits (e-Amusement cards: `E004...`).
    pub card_id: String,
    /// 4 digits; empty: the card is registered on the game's screens.
    #[serde(default)]
    pub pin: String,
    /// Player 2's card: 16 hex digits, empty: none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub card2_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pin2: String,
}

fn path(root: &Path) -> PathBuf {
    root.join(USER_PROFILES).join(FILE)
}

/// The saved card, or `None`.
pub fn load(root: &Path) -> Result<Option<Card>> {
    let path = path(root);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save(root: &Path, card: &Card) -> Result<()> {
    let path = path(root);
    std::fs::create_dir_all(root.join(USER_PROFILES))?;
    let text = serde_yaml_ng::to_string(card)?;
    std::fs::write(&path, format!("# e-Amusement card of every Konami game (launcher GUI)\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

/// A new card id: `E004` and 12 random hex digits.
pub fn new_id() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("E004{:012X}", h.finish() & 0xffff_ffff_ffff)
}

/// A valid card id: 16 hex digits.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A valid PIN: empty or 4 digits.
pub fn valid_pin(pin: &str) -> bool {
    pin.is_empty() || (pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit()))
}

/// The game environment of the card.
pub fn env(card: &Card) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if valid_id(&card.card_id) {
        env.push(("WAL_KONAMI_CARD_ID", card.card_id.to_ascii_uppercase()));
    }
    if valid_pin(&card.pin) && !card.pin.is_empty() {
        env.push(("WAL_KONAMI_CARD_PIN", card.pin.clone()));
    }
    if valid_id(&card.card2_id) {
        env.push(("WAL_KONAMI_CARD2_ID", card.card2_id.to_ascii_uppercase()));
        if valid_pin(&card.pin2) && !card.pin2.is_empty() {
            env.push(("WAL_KONAMI_CARD2_PIN", card.pin2.clone()));
        }
    }
    env
}
