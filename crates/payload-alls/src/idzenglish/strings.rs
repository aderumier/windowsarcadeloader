//! The texts' English translations, by their exact Japanese text (as the file has it: its
//! line breaks `\n`, its full-width characters and spaces kept).
//!
//! Each translator's table is its own file, `strings_a.rs` to `strings_d.rs` (version 1.30),
//! `strings_e.rs` to `strings_j.rs` (Ver.2's texts left, its cars' tuning texts), exporting
//! `pub(super) const STRINGS: &[(&str, &str)] = &[("日本語", "English"), ...];` in any order. A
//! translation's `\n`s are its line breaks (the lines' table is rebuilt from them). A Japanese
//! text in several tables takes the first one's (a to d).

use std::collections::HashMap;
use std::sync::OnceLock;

#[path = "strings_a.rs"]
mod a;
#[path = "strings_b.rs"]
mod b;
#[path = "strings_c.rs"]
mod c;
#[path = "strings_d.rs"]
mod d;
#[path = "strings_e.rs"]
mod e;
#[path = "strings_f.rs"]
mod f;
#[path = "strings_g.rs"]
mod g;
#[path = "strings_h.rs"]
mod h;
#[path = "strings_i.rs"]
mod i;
#[path = "strings_j.rs"]
mod j;

const TABLES: &[&[(&str, &str)]] = &[a::STRINGS, b::STRINGS, c::STRINGS, d::STRINGS, e::STRINGS, f::STRINGS, g::STRINGS, h::STRINGS, i::STRINGS, j::STRINGS];

/// `japanese`'s English text, when a table has it.
pub(super) fn translate(japanese: &str) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for (jp, en) in TABLES.iter().copied().flatten() {
            map.entry(*jp).or_insert(*en);
        }
        map
    })
    .get(japanese)
    .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        assert_eq!(translate("車は追加購入で全車種所有することができるぞ！"), Some("You can buy extra cars and own every model!"));
        assert_eq!(translate("車"), None);
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (jp, en) in TABLES.iter().copied().flatten() {
            assert!(super::super::txbn::has_japanese(jp), "key without Japanese: {jp:?}");
            assert!(!en.is_empty() && !en.contains('\0'), "bad translation of {jp:?}");
            if let Some(prev) = seen.insert(jp, en) {
                assert_eq!(prev, *en, "{jp:?} translated twice differently");
            }
        }
    }
}
