//! Scripted virtual stick input, for automated game tests (`--input-script FILE`).
//!
//! One step per line: `<seconds since launch> p<player> <inputs...>`; the inputs (virtual
//! stick button names, or pedal axes `accel`/`brake`, fully pressed) are held until the
//! player's next step, `-` releases everything. `#` starts a comment. Script inputs are
//! OR-ed with the real devices (a scripted pedal overrides the real one while held).
//!
//! ```text
//! 20.0 p1 coin
//! 20.3 p1 -
//! 22.0 p1 accel
//! 24.0 p1 -
//! ```

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use wal_protocol::{Axis, MAX_PLAYERS, StickState, button};

/// Inputs held by one script step: buttons, and pedal axes pressed fully.
#[derive(Clone, Copy, Default)]
struct Held {
    buttons: u32,
    pedals: [bool; Axis::COUNT],
}

pub struct Script {
    /// (time, player, held inputs), sorted by time.
    steps: Vec<(f64, usize, Held)>,
    start: Instant,
}

impl Script {
    pub fn load(path: &Path) -> Result<Script> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut steps = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let parse = || -> Result<(f64, usize, Held)> {
                let mut words = line.split_whitespace();
                let time: f64 = words.next().context("time")?.parse().context("time")?;
                let player: usize = words
                    .next()
                    .and_then(|p| p.strip_prefix('p'))
                    .and_then(|p| p.parse().ok())
                    .filter(|p| (1..=MAX_PLAYERS).contains(p))
                    .context("player p1..p4")?;
                let mut held = Held::default();
                for w in words.filter(|w| *w != "-") {
                    match Axis::from_name(w).filter(|a| a.is_pedal()) {
                        Some(a) => held.pedals[a as usize] = true,
                        None => held.buttons |= button::from_name(w).with_context(|| format!("unknown input '{w}'"))?,
                    }
                }
                Ok((time, player - 1, held))
            };
            match parse() {
                Ok(step) => steps.push(step),
                Err(e) => bail!("{}:{}: {e:#}", path.display(), n + 1),
            }
        }
        steps.sort_by(|a, b| a.0.total_cmp(&b.0));
        eprintln!("script: {} steps from {}", steps.len(), path.display());
        Ok(Script { steps, start: Instant::now() })
    }

    /// Apply the inputs currently held by the script for `player` to its stick.
    pub fn apply(&self, player: usize, stick: &mut StickState) {
        let now = self.start.elapsed().as_secs_f64();
        let Some((_, _, held)) = self.steps.iter().filter(|(t, p, _)| *p == player && *t <= now).next_back() else {
            return;
        };
        stick.buttons |= held.buttons;
        for (axis, pressed) in stick.axes.iter_mut().zip(held.pedals) {
            if pressed {
                *axis = i16::MAX;
            }
        }
    }
}
