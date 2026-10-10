//! Wheel calibration wizard: the steering axis' ends, the pedals' rest and pressed positions,
//! then the wheel buttons that act as each gamepad button (`wheels.rs`). The axes are found by
//! what moves: per axis, the lowest and highest values seen during a step.

use crate::wheels::{BUTTONS, Calibration, Pedal, Steering};

/// Smallest move from rest that counts as the axis being turned or pressed.
const MOVED: i32 = 8000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Wheel centered, pedals released: the rest positions.
    Start,
    Left,
    Right,
    Gas,
    Brake,
    Clutch,
    /// `BUTTONS[i]`.
    Button(usize),
    Done,
}

pub struct Wizard {
    /// SDL device id and joystick name.
    pub id: u32,
    pub name: String,
    pub step: Step,
    pub cal: Calibration,
    /// Last problem (shown under the instruction).
    pub note: String,
    current: Vec<i32>,
    rest: Vec<i32>,
    low: Vec<i32>,
    high: Vec<i32>,
}

/// A hat direction's raw source name (`hat0up`), the first direction of `bits`.
pub fn hat_source(index: u8, bits: u8) -> String {
    let dir = match 1 << bits.trailing_zeros() {
        1 => "up",
        2 => "right",
        4 => "down",
        _ => "left",
    };
    format!("hat{index}{dir}")
}

impl Wizard {
    pub fn new(id: u32, name: String, axes: &[i32]) -> Wizard {
        Wizard {
            id,
            name,
            step: Step::Start,
            cal: Calibration::default(),
            note: String::new(),
            current: axes.to_vec(),
            rest: axes.to_vec(),
            low: axes.to_vec(),
            high: axes.to_vec(),
        }
    }

    pub fn instruction(&self) -> String {
        match self.step {
            Step::Start => "Center the wheel and release every pedal, then Next.".into(),
            Step::Left => "Turn the wheel fully left (as far as the games should steer), then Next or any wheel button.".into(),
            Step::Right => "Turn the wheel fully right, then Next or any wheel button.".into(),
            Step::Gas => "Press the gas pedal fully, then Next or any wheel button (Skip: no gas pedal).".into(),
            Step::Brake => "Press the brake pedal fully, then Next or any wheel button (Skip: no brake pedal).".into(),
            Step::Clutch => "Press the clutch pedal fully, then Next or any wheel button (Skip: no clutch).".into(),
            Step::Button(i) => format!("Press the wheel button for the gamepad's {} (Skip: none).", BUTTONS[i].1),
            Step::Done => "Done: Save to use this wheel as a gamepad in every game.".into(),
        }
    }

    fn axis_step(&self) -> bool {
        matches!(self.step, Step::Start | Step::Left | Step::Right | Step::Gas | Step::Brake | Step::Clutch)
    }

    pub fn axis_moved(&mut self, index: u8, value: i32) {
        let i = index as usize;
        if i >= self.current.len() {
            for v in [&mut self.current, &mut self.rest, &mut self.low, &mut self.high] {
                v.resize(i + 1, 0);
            }
        }
        self.current[i] = value;
        self.low[i] = self.low[i].min(value);
        self.high[i] = self.high[i].max(value);
    }

    /// A wheel button or hat direction pressed: the next step (axis steps) or that gamepad
    /// button (button steps).
    pub fn pressed(&mut self, source: String) {
        match self.step {
            Step::Button(i) => {
                if let Some((name, _)) = self.cal.buttons.iter().find(|(_, s)| **s == source) {
                    self.note = format!("that button is already {name}: another one, or Skip");
                    return;
                }
                self.cal.buttons.insert(BUTTONS[i].0.to_string(), source);
                self.advance();
            }
            _ if self.axis_step() => self.next(),
            _ => {}
        }
    }

    /// Next (an axis step: what moved).
    pub fn next(&mut self) {
        let moved = |s: &Wizard, i: usize, end: i32| (end - s.rest[i]).abs();
        match self.step {
            Step::Start => {
                self.rest = self.current.clone();
            }
            Step::Left => {
                // the axis that moved most, and the end it went to
                let best = (0..self.rest.len())
                    .flat_map(|i| [(i, self.low[i]), (i, self.high[i])])
                    .max_by_key(|&(i, end)| moved(self, i, end));
                match best.filter(|&(i, end)| moved(self, i, end) > MOVED) {
                    Some((i, end)) => {
                        let center = self.rest[i];
                        self.cal.steering = Some(Steering { axis: i as u8, left: end, center, right: center });
                    }
                    None => return self.note = "the wheel did not turn: turn it fully left".into(),
                }
            }
            Step::Right => {
                let Some(mut s) = self.cal.steering else { return };
                let i = s.axis as usize;
                // the other side of the center
                let end = if s.left < s.center { self.high[i] } else { self.low[i] };
                if moved(self, i, end) <= MOVED {
                    return self.note = "the wheel did not turn right".into();
                }
                s.right = end;
                self.cal.steering = Some(s);
            }
            Step::Gas | Step::Brake | Step::Clutch => {
                let steering = self.cal.steering.map(|s| s.axis as usize);
                let used: Vec<(u8, bool)> = [self.cal.gas, self.cal.brake, self.cal.clutch]
                    .into_iter()
                    .flatten()
                    .map(|p| (p.axis, p.full > p.rest))
                    .collect();
                let best = (0..self.rest.len())
                    .filter(|i| Some(*i) != steering)
                    .flat_map(|i| [(i, self.low[i]), (i, self.high[i])])
                    .filter(|&(i, end)| !used.contains(&(i as u8, end > self.rest[i])))
                    .max_by_key(|&(i, end)| moved(self, i, end));
                let Some((i, end)) = best.filter(|&(i, end)| moved(self, i, end) > MOVED) else {
                    return self.note = "no pedal moved: press it fully (or Skip)".into();
                };
                let pedal = Some(Pedal { axis: i as u8, rest: self.rest[i], full: end });
                match self.step {
                    Step::Gas => self.cal.gas = pedal,
                    Step::Brake => self.cal.brake = pedal,
                    _ => self.cal.clutch = pedal,
                }
            }
            _ => return,
        }
        self.advance();
    }

    /// The step left without a value.
    pub fn skip(&mut self) {
        match self.step {
            Step::Start | Step::Done => return,
            // no steering: no right side either
            Step::Left => self.step = Step::Right,
            _ => {}
        }
        self.advance();
    }

    fn advance(&mut self) {
        self.note.clear();
        self.step = match self.step {
            Step::Start => Step::Left,
            Step::Left => Step::Right,
            Step::Right => Step::Gas,
            Step::Gas => Step::Brake,
            Step::Brake => Step::Clutch,
            Step::Clutch => Step::Button(0),
            Step::Button(i) if i + 1 < BUTTONS.len() => Step::Button(i + 1),
            _ => Step::Done,
        };
        // the next step's movements from where the axes are now
        self.low = self.current.clone();
        self.high = self.current.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_calibration() {
        // axis 0 wheel, 1 gas, 2 brake (released at +32767, pressed toward -32768)
        let mut w = Wizard::new(1, "Wheel".into(), &[100, 32767, 32767]);
        w.next();
        assert_eq!(w.step, Step::Left);
        w.next();
        assert_eq!(w.step, Step::Left, "nothing moved");
        for v in [-5000, -20000, -30000, -15000] {
            w.axis_moved(0, v);
        }
        w.pressed("button3".into());
        assert_eq!(w.step, Step::Right);
        w.axis_moved(0, 31000);
        w.next();
        assert_eq!(w.cal.steering, Some(Steering { axis: 0, left: -30000, center: 100, right: 31000 }));
        w.axis_moved(1, -32768);
        w.axis_moved(1, 32767);
        w.next();
        assert_eq!(w.cal.gas, Some(Pedal { axis: 1, rest: 32767, full: -32768 }));
        w.axis_moved(2, -32000);
        w.next();
        assert_eq!(w.cal.brake, Some(Pedal { axis: 2, rest: 32767, full: -32000 }));
        w.skip();
        assert_eq!(w.step, Step::Button(0));
        w.pressed("button4".into());
        w.pressed("button4".into());
        assert_eq!(w.step, Step::Button(1), "a button used twice");
        w.pressed(hat_source(0, 0x09));
        assert_eq!(w.cal.buttons.get("r1").map(String::as_str), Some("button4"));
        assert_eq!(w.cal.buttons.get("l1").map(String::as_str), Some("hat0up"));
        while w.step != Step::Done {
            w.skip();
        }
        assert_eq!(w.cal.buttons.len(), 2);
    }

    #[test]
    fn shared_pedal_axis() {
        // gas and brake on the two halves of axis 1
        let mut w = Wizard::new(1, "Wheel".into(), &[0, 0]);
        w.next();
        w.skip();
        assert_eq!(w.step, Step::Gas);
        w.axis_moved(1, -32768);
        w.axis_moved(1, 0);
        w.next();
        w.axis_moved(1, 32767);
        w.axis_moved(1, 0);
        w.next();
        assert_eq!(w.cal.gas, Some(Pedal { axis: 1, rest: 0, full: -32768 }));
        assert_eq!(w.cal.brake, Some(Pedal { axis: 1, rest: 0, full: 32767 }));
        assert_eq!(w.cal.steering, None);
    }
}
