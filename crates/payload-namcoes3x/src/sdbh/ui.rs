//! A small software canvas for the card binder and the play mat panel, drawn in the look of
//! the Odin emulator's menus: granite with the game showing through, corners cut at 45
//! degrees, one pixel strokes, the Cozette pixel font (MIT, `assets/cozette.ttf`) at whole
//! multiples of its size, the red ochre accent on the frame of focus and teal on what is chosen.
//!
//! The canvas is BGRA with straight alpha (drawn over the game with alpha blending).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

/// Colours, `0xAARRGGBB`: Odin's Rúnasteinn palette.
pub mod look {
    pub const FRAME: u32 = 0xE62A2A28;
    pub const INK: u32 = 0xFFEEEAE2;
    pub const DIM: u32 = 0xFFB0ABA0;
    pub const FAINT: u32 = 0xFF837F75;
    pub const LINE: u32 = 0x29EEEAE2;
    pub const OUTLINE: u32 = 0x38EEEAE2;
    pub const ACCENT: u32 = 0xFFE0664A;
    pub const ACTION: u32 = 0xFF6AA9B0;
    pub const ON_ACTION: u32 = 0xFF2A2A28;
    pub const TITLE: u32 = 0xFFBDB5D0;
    pub const TILE: u32 = 0xFF1D1D1B;
    pub const SHADE: u32 = 0x80000000;
    /// The attack area's red glow and the support area's blue (the play mat).
    pub const ATTACK: u32 = 0xFFE0664A;
    pub const SUPPORT: u32 = 0xFF6A8FD0;

    /// `c` with its alpha multiplied by `share`.
    pub fn alpha(c: u32, share: f32) -> u32 {
        let a = ((c >> 24) as f32 * share).round().clamp(0.0, 255.0) as u32;
        (c & 0x00FF_FFFF) | (a << 24)
    }
}

const COZETTE: &[u8] = include_bytes!("../../assets/cozette.ttf");
/// Cozette's design size, in pixels.
pub const FONT_SIZE: f32 = 13.0;

pub fn font() -> &'static FontRef<'static> {
    static FONT: std::sync::OnceLock<FontRef<'static>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(COZETTE).expect("the Cozette font"))
}

/// A picture, straight alpha BGRA.
#[derive(Clone)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    /// The font size now (a whole multiple of Cozette's).
    pub font: f32,
    /// Drawing limited to this rectangle (x0, y0, x1, y1).
    clip: (i32, i32, i32, i32),
}

impl Canvas {
    pub fn new(w: i32, h: i32, scale: f32) -> Self {
        Canvas { w, h, px: vec![0; (w.max(0) * h.max(0)) as usize], font: FONT_SIZE * scale, clip: (0, 0, w, h) }
    }

    /// `c` over the pixel with this coverage (0..1).
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: u32, coverage: f32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        if x < cx0 || y < cy0 || x >= cx1 || y >= cy1 || coverage <= 0.0 {
            return;
        }
        let sa = (c >> 24) as f32 / 255.0 * coverage.min(1.0);
        if sa <= 0.0 {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.px[i];
        let da = (d >> 24) as f32 / 255.0;
        let oa = sa + da * (1.0 - sa);
        let ch = |shift: u32| {
            let s = ((c >> shift) & 0xFF) as f32;
            let dd = ((d >> shift) & 0xFF) as f32;
            (((s * sa + dd * da * (1.0 - sa)) / oa).round() as u32).min(255)
        };
        self.px[i] = ((oa * 255.0).round() as u32) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0);
    }

    pub fn fill_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: u32) {
        if c >> 24 == 0 {
            return;
        }
        for y in (y0.floor() as i32).max(0)..(y1.ceil() as i32).min(self.h) {
            let cy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
            for x in (x0.floor() as i32).max(0)..(x1.ceil() as i32).min(self.w) {
                let cx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                self.blend(x, y, c, cx * cy);
            }
        }
    }

    /// Coverage of the pixel (x, y) by the box (a, b) with its corners cut by `cut`.
    fn octagon_coverage(px: f32, py: f32, a: (f32, f32), b: (f32, f32), cut: f32) -> f32 {
        let cx = ((px + 1.0).min(b.0) - px.max(a.0)).clamp(0.0, 1.0);
        let cy = ((py + 1.0).min(b.1) - py.max(a.1)).clamp(0.0, 1.0);
        let mut cov = cx * cy;
        if cov <= 0.0 || cut <= 0.0 {
            return cov;
        }
        // the cut corners: half planes x' + y' >= cut from each corner, anti-aliased
        let (mx, my) = (px + 0.5, py + 0.5);
        for (dx, dy) in [(mx - a.0, my - a.1), (b.0 - mx, my - a.1), (b.0 - mx, b.1 - my), (mx - a.0, b.1 - my)] {
            let d = (dx + dy - cut) / std::f32::consts::SQRT_2;
            cov = cov.min((d + 0.5).clamp(0.0, 1.0));
        }
        cov
    }

    /// A box with its corners cut: filled (`fill`) and stroked inside (`border`, `strokes` px).
    pub fn frame(&mut self, a: (f32, f32), b: (f32, f32), cut: f32, fill: u32, border: u32, strokes: f32) {
        let (x0, y0, x1, y1) = (a.0.floor() as i32, a.1.floor() as i32, b.0.ceil() as i32, b.1.ceil() as i32);
        let t = strokes.max(1.0).round();
        let inner_cut = (cut - t * 0.4).max(0.0);
        for y in y0.max(0)..y1.min(self.h) {
            for x in x0.max(0)..x1.min(self.w) {
                let outer = Self::octagon_coverage(x as f32, y as f32, a, b, cut);
                if outer <= 0.0 {
                    continue;
                }
                if fill >> 24 != 0 {
                    self.blend(x, y, fill, outer);
                }
                if border >> 24 != 0 {
                    let inner = Self::octagon_coverage(x as f32, y as f32, (a.0 + t, a.1 + t), (b.0 - t, b.1 - t), inner_cut);
                    self.blend(x, y, border, (outer - inner).max(0.0));
                }
            }
        }
    }

    pub fn measure(&self, text: &str) -> f32 {
        self.measure_at(text, self.font)
    }

    pub fn measure_at(&self, text: &str, size: f32) -> f32 {
        let f = font().as_scaled(PxScale::from(size));
        text.chars().map(|ch| f.h_advance(f.glyph_id(ch))).sum()
    }

    pub fn write(&mut self, x: f32, y: f32, c: u32, text: &str) -> f32 {
        self.write_at(x, y, c, text, self.font)
    }

    /// `text` with its top at `y`; the width written.
    pub fn write_at(&mut self, x: f32, y: f32, c: u32, text: &str, size: f32) -> f32 {
        let f = font().as_scaled(PxScale::from(size));
        let (mut pen, base) = (x.floor(), (y + f.ascent()).floor());
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            let glyph = id.with_scale_and_position(size, ab_glyph::point(pen, base));
            if let Some(outline) = font().outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, v| {
                    self.blend(bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32, c, v);
                });
            }
            pen += f.h_advance(id);
        }
        pen - x
    }

    /// A title in capitals, its letters spread.
    pub fn spaced(&mut self, x: f32, y: f32, c: u32, text: &str, gap: f32) -> f32 {
        let mut pen = x;
        for ch in text.chars() {
            let s = ch.to_string();
            pen += self.write(pen, y, c, &s) + gap;
        }
        pen - x
    }

    /// The text cut to that width, with dots.
    pub fn fitted(&self, text: &str, width: f32) -> String {
        if self.measure(text) <= width {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        for n in (0..chars.len()).rev() {
            let s: String = chars[..n].iter().collect::<String>() + "...";
            if self.measure(&s) <= width {
                return s;
            }
        }
        String::new()
    }

    /// The lines of `text` wrapped at `width` (words).
    pub fn wrap(&self, text: &str, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if self.measure(&candidate) <= width || line.is_empty() {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// A picture drawn into the box (its size already the box's, else scaled nearest).
    pub fn picture(&mut self, p: &Picture, x: f32, y: f32, w: f32, h: f32, opacity: f32) {
        let (x0, y0) = (x.round() as i32, y.round() as i32);
        let (w, h) = (w.round().max(1.0) as i32, h.round().max(1.0) as i32);
        for j in 0..h {
            let sy = ((j as u32 * p.h) / h as u32).min(p.h - 1) as usize;
            for i in 0..w {
                let sx = ((i as u32 * p.w) / w as u32).min(p.w - 1) as usize;
                let k = (sy * p.w as usize + sx) * 4;
                let c = (p.bgra[k + 3] as u32) << 24 | (p.bgra[k + 2] as u32) << 16 | (p.bgra[k + 1] as u32) << 8 | p.bgra[k] as u32;
                self.blend(x0 + i, y0 + j, c, opacity);
            }
        }
    }

    /// The canvas as BGRA bytes.
    pub fn bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out
    }
}

/// A JPEG (the card pack's pictures) decoded and reduced to `w` x `h` (area average).
pub fn load_jpeg(bytes: &[u8], w: u32, h: u32) -> Option<Picture> {
    let mut d = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(bytes));
    let rgb = d.decode().ok()?;
    let (sw, sh) = d.dimensions()?;
    let comps = rgb.len() / (sw * sh).max(1);
    if comps < 3 {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        let (sy0, sy1) = (y * sh / h as usize, ((y + 1) * sh / h as usize).max(y * sh / h as usize + 1).min(sh));
        for x in 0..w as usize {
            let (sx0, sx1) = (x * sw / w as usize, ((x + 1) * sw / w as usize).max(x * sw / w as usize + 1).min(sw));
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let k = (sy * sw + sx) * comps;
                    acc[0] += rgb[k] as u32;
                    acc[1] += rgb[k + 1] as u32;
                    acc[2] += rgb[k + 2] as u32;
                    n += 1;
                }
            }
            let o = (y * w as usize + x) * 4;
            let n = n.max(1);
            out[o] = (acc[2] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[0] / n) as u8;
            out[o + 3] = 255;
        }
    }
    Some(Picture { w, h, bgra: out })
}
