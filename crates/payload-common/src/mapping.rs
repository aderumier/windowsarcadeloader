//! Virtual stick button -> native system input mapping.
//!
//! Each system declares its native inputs and a default mapping; a game profile
//! can override it through `WAL_MAP`, e.g. `b5=btn6,b6=none`.

use crate::log;
use wal_protocol::{StickState, button};

pub struct ButtonMap<N: Copy> {
    entries: Vec<(u32, N)>,
}

impl<N: Copy> ButtonMap<N> {
    /// `natives`: name -> native input of the system.
    /// `defaults`: virtual button name -> native name.
    pub fn new(natives: &[(&str, N)], defaults: &[(&str, &str)]) -> Self {
        let mut map = ButtonMap { entries: Vec::new() };
        for (v, n) in defaults {
            map.set(natives, v, n);
        }
        if let Ok(spec) = std::env::var(wal_protocol::env::MAP) {
            for pair in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                match pair.split_once('=') {
                    Some((v, n)) => map.set(natives, v.trim(), n.trim()),
                    None => log!("mapping: ignoring '{pair}', expected virtual=native"),
                }
            }
        }
        map
    }

    fn set(&mut self, natives: &[(&str, N)], virt: &str, native: &str) {
        let Some(mask) = button::from_name(virt) else {
            log!("mapping: unknown virtual button '{virt}'");
            return;
        };
        self.entries.retain(|(m, _)| *m != mask);
        if native.eq_ignore_ascii_case("none") {
            return;
        }
        match natives.iter().find(|(name, _)| name.eq_ignore_ascii_case(native)) {
            Some((_, n)) => self.entries.push((mask, *n)),
            None => log!("mapping: unknown native input '{native}'"),
        }
    }

    /// Calls `f` for every native input pressed on `stick`.
    pub fn for_each_pressed(&self, stick: &StickState, mut f: impl FnMut(N)) {
        for (mask, native) in &self.entries {
            if stick.pressed(*mask) {
                f(*native);
            }
        }
    }
}
