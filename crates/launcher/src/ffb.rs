//! Force feedback on a wheel through its evdev node (as linuxloader's evdevFfb.c): a constant
//! force, a centering spring (the device's FF_SPRING, else its autocenter) and a vibration (a
//! sine). Each effect is uploaded once, then updated in place; it plays until replaced. The
//! wheel's own centering spring is turned off: the game centers it. Gamepads rumble through
//! SDL instead (input.rs).

use evdev::{Device, FFCondition, FFEffect, FFEffectCode, FFEffectData, FFEffectKind, FFEnvelope, FFReplay, FFTrigger, FFWaveform};
use wal_protocol::output;

use crate::config::FfbConfig;

pub struct Wheel {
    dev: Device,
    path: String,
    has_spring: bool,
    has_autocenter: bool,
    has_sine: bool,
    invert: bool,
    gain: i32,
    constant: Option<FFEffect>,
    spring: Option<FFEffect>,
    vibration: Option<FFEffect>,
    /// Last values set (constant, spring, vibration), protocol scale.
    last: [i32; 3],
}

impl Wheel {
    /// The wheel at `path` (an evdev node supporting a constant force), else None.
    pub fn open(path: &str, cfg: &FfbConfig) -> Option<Wheel> {
        let mut dev = Device::open(path).ok()?;
        let ff = dev.supported_ff()?;
        if !ff.contains(FFEffectCode::FF_CONSTANT) {
            return None;
        }
        let (has_spring, has_autocenter, has_sine, has_gain) = (
            ff.contains(FFEffectCode::FF_SPRING),
            ff.contains(FFEffectCode::FF_AUTOCENTER),
            ff.contains(FFEffectCode::FF_PERIODIC) && ff.contains(FFEffectCode::FF_SINE),
            ff.contains(FFEffectCode::FF_GAIN),
        );
        if has_gain {
            let _ = dev.set_ff_gain((0xFFFF * cfg.gain.min(100) / 100) as u16);
        }
        // the game centers the wheel itself
        if has_autocenter {
            let _ = dev.set_ff_autocenter(0);
        }
        eprintln!(
            "ffb: wheel {path} ({}){}{}",
            dev.name().unwrap_or("?"),
            if has_spring { ", spring" } else if has_autocenter { ", autocenter" } else { "" },
            if has_sine { ", sine" } else { "" }
        );
        Some(Wheel {
            dev,
            path: path.to_string(),
            has_spring,
            has_autocenter,
            has_sine,
            invert: cfg.invert,
            // without FF_GAIN, the levels are scaled
            gain: if has_gain { 100 } else { cfg.gain.min(100) as i32 },
            constant: None,
            spring: None,
            vibration: None,
            last: [i32::MIN; 3],
        })
    }

    /// Applies a force feedback output (protocol scale).
    pub fn set(&mut self, id: u16, value: i32) {
        let slot = match id {
            output::FFB_CONSTANT => 0,
            output::FFB_SPRING => 1,
            output::FFB_VIBRATION => 2,
            _ => return,
        };
        if self.last[slot] == value {
            return;
        }
        self.last[slot] = value;
        let level = |v: i32| (v.clamp(-output::FFB_MAX, output::FFB_MAX) as i64 * 0x7FFF / output::FFB_MAX as i64 * self.gain as i64 / 100) as i16;
        let kind = match slot {
            0 => {
                // direction 0x4000: positive levels pull to the left; the protocol's positive
                // value pushes to the right
                let v = if self.invert { value } else { -value };
                FFEffectKind::Constant { level: level(v), envelope: no_envelope() }
            }
            1 if self.has_spring => {
                let l = level(value.max(0));
                let c = FFCondition { right_saturation: 0xFFFF, left_saturation: 0xFFFF, right_coefficient: l, left_coefficient: l, deadband: 0, center: 0 };
                FFEffectKind::Spring { condition: [c, c] }
            }
            1 => {
                if self.has_autocenter {
                    let _ = self.dev.set_ff_autocenter((level(value.max(0)) as u16).saturating_mul(2));
                }
                return;
            }
            _ if self.has_sine => FFEffectKind::Periodic {
                waveform: FFWaveform::Sine,
                period: 60,
                magnitude: level(value.max(0)),
                offset: 0,
                phase: 0,
                envelope: no_envelope(),
            },
            _ => return,
        };
        let data = FFEffectData {
            direction: 0x4000,
            trigger: FFTrigger::default(),
            // length 0: until stopped
            replay: FFReplay { length: 0, delay: 0 },
            kind,
        };
        let effect = match slot {
            0 => &mut self.constant,
            1 => &mut self.spring,
            _ => &mut self.vibration,
        };
        let result = match effect {
            Some(e) => e.update(data),
            None => self.dev.upload_ff_effect(data).and_then(|mut e| {
                e.play(1)?;
                *effect = Some(e);
                Ok(())
            }),
        };
        if let Err(e) = result {
            eprintln!("ffb: {}: effect {id} failed: {e}", self.path);
        }
    }
}

fn no_envelope() -> FFEnvelope {
    FFEnvelope { attack_length: 0, attack_level: 0, fade_length: 0, fade_level: 0 }
}
