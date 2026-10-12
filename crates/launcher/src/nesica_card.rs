//! The player's NESiCA card, machine-wide (`userprofiles/nesica-card.yaml`, edited in the GUI):
//! every NESiCA / NESiCA 2 game reads this card (`WAL_NESICA_CARD_ID`) with this player name
//! (`WAL_NESICA_PLAYER_NAME`, for the games whose card server data has one: Fighting EX
//! Layer). Each game keeps its own progress on it (its saves, by card id). A game profile's
//! own `env` values win.

use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::USER_PROFILES;

const FILE: &str = "nesica-card.yaml";
/// The digits real NESiCA card ids start with.
const PREFIX: &str = "7020392";
/// The payload's card id when none is set.
pub const DEFAULT_ID: &str = "7020392010281502";
/// Player names longer than this are cut by the games' screens.
pub const NAME_MAX: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// 16 digits.
    pub card_id: String,
    /// Empty: the game's own (Fighting EX Layer registers a new card as NO-NAME).
    #[serde(default)]
    pub player_name: String,
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
    std::fs::write(&path, format!("# NESiCA card of every NESiCA game (launcher GUI)\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

/// A new card id: the NESiCA prefix and 9 random digits.
pub fn new_id() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{PREFIX}{:09}", h.finish() % 1_000_000_000)
}

/// A valid card id: 16 digits.
pub fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit())
}

/// The game environment of the card.
pub fn env(card: &Card) -> Vec<(&'static str, String)> {
    let mut env = Vec::new();
    if valid_id(&card.card_id) {
        env.push(("WAL_NESICA_CARD_ID", card.card_id.clone()));
    }
    let name = card.player_name.trim();
    if !name.is_empty() {
        env.push(("WAL_NESICA_PLAYER_NAME", name.chars().take(NAME_MAX).collect()));
    }
    env
}
