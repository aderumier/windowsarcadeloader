//! Font face names in the game's code page: `WAL_FONT_CODEPAGE=<cp>` (932 = Shift-JIS).
//!
//! Japanese games call `gdi32!CreateFontA` with Shift-JIS face names ("ＭＳ ゴシック", the
//! family names of fonts they add with AddFontResourceEx). With a western ANSI code page Wine
//! decodes them wrongly, matches no font and falls back to one with other metrics (Crimzon
//! Clover's texts were far too big). The face name is decoded with the given code page and
//! the font created with `CreateFontW`.
//!
//! `WAL_FONT_REPLACE=<from>=<to>,...` then renames faces: Wine only knows the Japanese family
//! name of a font ("FOT-キアロ Std B") under a Japanese locale, the English one
//! ("FOT-Chiaro Std B") always.
//!
//! `WAL_FONT_SCALE=<factor>` multiplies the height of the fonts the game creates: the font
//! wine falls back to may be much larger than the original (Crimzon Clover's texts).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{iat, log};
use windows_sys::Win32::Globalization::MultiByteToWideChar;
use windows_sys::Win32::Graphics::Gdi::{CreateFontW, HFONT};

static CODEPAGE: AtomicU32 = AtomicU32::new(0);
/// Height factor, f32 bits (0: unscaled).
static SCALE: AtomicU32 = AtomicU32::new(0);
static REPLACE: OnceLock<Vec<(String, String)>> = OnceLock::new();

pub fn init() {
    let cp = std::env::var("WAL_FONT_CODEPAGE").ok().and_then(|v| v.trim().parse::<u32>().ok());
    let scale = std::env::var("WAL_FONT_SCALE").ok().and_then(|v| v.trim().parse::<f32>().ok());
    if cp.is_none() && scale.is_none() {
        return;
    }
    // CP_ACP (0): the face name is passed through unchanged
    let cp = cp.unwrap_or(0);
    CODEPAGE.store(cp, Ordering::Relaxed);
    SCALE.store(scale.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
    let replace = std::env::var("WAL_FONT_REPLACE").unwrap_or_default();
    let _ = REPLACE.set(
        replace
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .map(|(from, to)| (from.trim().to_string(), to.trim().to_string()))
            .collect(),
    );
    if unsafe { iat::hook("gdi32.dll", "CreateFontA", create_font_a as *const () as usize) }.is_some() {
        log!("font: CreateFontA hooked (code page {cp}, scale {scale:?})");
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_font_a(
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u32,
    underline: u32,
    strike_out: u32,
    charset: u32,
    out_precision: u32,
    clip_precision: u32,
    quality: u32,
    pitch_and_family: u32,
    face: *const u8,
) -> HFONT {
    let mut wide = [0u16; 64];
    if !face.is_null() {
        let cp = CODEPAGE.load(Ordering::Relaxed);
        // -1: NUL terminated; at most 31 characters + NUL (LF_FACESIZE)
        let n = unsafe { MultiByteToWideChar(cp, 0, face, -1, wide.as_mut_ptr(), 32) };
        if n > 0 {
            let name = String::from_utf16_lossy(&wide[..n as usize - 1]);
            let to = REPLACE.get().and_then(|r| r.iter().find(|(from, _)| *from == name)).map(|(_, to)| to.clone());
            log!("font: CreateFont \"{name}\"{} height {height} weight {weight} charset {charset}", to.as_ref().map_or(String::new(), |t| format!(" -> \"{t}\"")));
            if let Some(to) = to {
                wide = [0; 64];
                for (i, c) in to.encode_utf16().take(31).enumerate() {
                    wide[i] = c;
                }
            }
        }
    }
    let scale = f32::from_bits(SCALE.load(Ordering::Relaxed));
    let height = if scale > 0.0 { (height as f32 * scale).round() as i32 } else { height };
    unsafe {
        CreateFontW(
            height,
            width,
            escapement,
            orientation,
            weight,
            italic,
            underline,
            strike_out,
            charset,
            out_precision,
            clip_precision,
            quality,
            pitch_and_family,
            if face.is_null() { std::ptr::null() } else { wide.as_ptr() },
        )
    }
}
