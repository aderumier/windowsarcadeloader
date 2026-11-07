//! Sinden frame geometry, shared by the launcher (aim) and native presentation hooks.
#[derive(Clone, Copy, Debug)]
pub struct Border {
    /// Colored frame and outer black outline, percent of the physical screen width.
    pub percent: f64,
    pub black_percent: f64,
    pub rgb: u32,
}

impl Border {
    pub fn parse(size: &str, color: &str) -> Result<Self, String> {
        let (percent, black_percent) = match size.trim() {
            "thin" => (1.0, 0.0),
            "medium" => (2.0, 0.0),
            "big" => (2.0, 1.0),
            v => (
                v.parse::<f64>().map_err(|_| {
                    "border size: thin, medium, big, or percent (0.1..10)".to_string()
                })?,
                0.0,
            ),
        };
        if !percent.is_finite() || !(0.1..=10.0).contains(&percent) {
            return Err("border size must be between 0.1 and 10 percent".into());
        }
        let rgb = match color.trim().to_ascii_lowercase().as_str() {
            "white" => 0xffffff,
            "red" => 0xff0000,
            "green" => 0x00ff00,
            "blue" => 0x0000ff,
            v => {
                let hex = v
                    .strip_prefix('#')
                    .or_else(|| v.strip_prefix("0x"))
                    .unwrap_or(v);
                if hex.len() != 6 {
                    return Err("border color: white, red, green, blue, or #RRGGBB".into());
                }
                u32::from_str_radix(hex, 16).map_err(|_| "invalid border RGB color".to_string())?
            }
        };
        Ok(Self {
            percent,
            black_percent,
            rgb,
        })
    }

    /// (colored thickness, outer black thickness), in pixels. Leave a usable interior even
    /// on unusually wide screens. Both renderers and input use this exact rounding.
    pub fn thickness(self, width: u32, height: u32) -> (u32, u32) {
        let limit = width.min(height).saturating_sub(2) / 2;
        let black = ((width as f64 * self.black_percent / 100.0).round() as u32).min(limit / 2);
        let color = ((width as f64 * self.percent / 100.0).round() as u32)
            .max(1)
            .min(limit.saturating_sub(black));
        (color, black)
    }

    /// The complete original screen fitted inside the frame, preserving its aspect.
    /// Returns left, top, width, height in pixels. No original pixels are cropped.
    pub fn picture(self, width: u32, height: u32) -> [u32; 4] {
        let (color, black) = self.thickness(width, height);
        let inset = color + black;
        let (iw, ih) = (
            width.saturating_sub(2 * inset).max(1),
            height.saturating_sub(2 * inset).max(1),
        );
        let (w, h) = if iw as u64 * height as u64 > ih as u64 * width as u64 {
            ((ih as u64 * width as u64 / height.max(1) as u64) as u32, ih)
        } else {
            (iw, (iw as u64 * height as u64 / width.max(1) as u64) as u32)
        };
        [
            (width.saturating_sub(w)) / 2,
            (height.saturating_sub(h)) / 2,
            w.max(1),
            h.max(1),
        ]
    }

    pub fn contains(self, position: [i32; 2], screen: [u32; 2]) -> bool {
        let rect = self.picture(screen[0], screen[1]);
        (0..2).all(|axis| {
            let p = (position[axis] as f64 + 32768.0) / 65535.0 * screen[axis] as f64;
            p >= rect[axis] as f64 && p <= (rect[axis] + rect[axis + 2]) as f64
        })
    }

    pub fn axis(self, value: i32, axis: usize, screen: [u32; 2]) -> i32 {
        let rect = self.picture(screen[0], screen[1]);
        let pixel = (value as f64 + 32768.0) / 65535.0 * screen[axis] as f64;
        (((pixel - rect[axis] as f64) / rect[axis + 2] as f64).clamp(0.0, 1.0) * 65535.0).round()
            as i32
            - 32768
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fits_without_overlap_and_aim_matches() {
        for screen in [[1920, 1080], [1280, 1024], [1080, 1920], [3840, 1080]] {
            for size in ["thin", "medium", "big", "1.5", "10"] {
                let border = Border::parse(size, "#12ABEF").unwrap();
                let [x, y, w, h] = border.picture(screen[0], screen[1]);
                let (c, b) = border.thickness(screen[0], screen[1]);
                assert!(x >= c + b && y >= c + b);
                assert!(x + w <= screen[0] - c - b && y + h <= screen[1] - c - b);
                assert!((w as f64 / h as f64 - screen[0] as f64 / screen[1] as f64).abs() < 0.02);
                for axis in 0..2 {
                    let raw = |pixel: u32| {
                        (pixel as f64 / screen[axis] as f64 * 65535.0).round() as i32 - 32768
                    };
                    assert!(border.axis(raw([x, y][axis]), axis, screen) <= -32760);
                    assert!(border.axis(raw([x + w, y + h][axis]), axis, screen) >= 32759);
                }
            }
        }
    }
    #[test]
    fn border_and_padding_are_offscreen() {
        let border = Border::parse("medium", "white").unwrap();
        assert!(border.contains([0, 0], [1920, 1080]));
        for p in [[-32768, 0], [32767, 0], [0, -32768], [0, 32767]] {
            assert!(!border.contains(p, [1920, 1080]));
        }
    }

    #[test]
    fn rejects_invalid_settings() {
        for size in ["0", "-1", "NaN", "inf", "11", "huge"] {
            assert!(Border::parse(size, "white").is_err());
        }
        assert!(Border::parse("2", "#zzzfff").is_err());
    }
}
