//! Native frame presentation, following linuxloader's clean-frame approach. The game's
//! back buffer is saved, scaled inside the frame for presentation, then restored.
use std::sync::OnceLock;
use wal_protocol::border::Border;

pub fn config() -> Option<Border> {
    static CONFIG: OnceLock<Option<Border>> = OnceLock::new();
    *CONFIG.get_or_init(|| {
        if !std::env::var("WAL_SINDEN_BORDER").is_ok_and(|v| v == "1") {
            return None;
        }
        Border::parse(
            &std::env::var("WAL_SINDEN_BORDER_SIZE").unwrap_or_else(|_| "medium".into()),
            &std::env::var("WAL_SINDEN_BORDER_COLOR").unwrap_or_else(|_| "white".into()),
        )
        .ok()
    })
}

/// Geometry in a rendering surface. Thickness and picture fractions are calculated from
/// physical screen pixels, so Wine's fullscreen scaling cannot alter the frame's shape.
pub fn geometry(w: u32, h: u32) -> Option<([i32; 4], i32, i32, u32)> {
    geometry_impl(w, h, true)
}
pub fn outer_geometry(w: u32, h: u32) -> Option<([i32; 4], i32, i32, u32)> {
    geometry_impl(w, h, false)
}
fn geometry_impl(w: u32, h: u32, desktop: bool) -> Option<([i32; 4], i32, i32, u32)> {
    let border = config()?;
    let (sw, sh) = std::env::var("WAL_SINDEN_SCREEN")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
        })
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((w, h));
    let [mut x, mut y, mut pw, mut ph] = border.picture(sw, sh);
    let source_aspect = crate::desktop::size().or_else(|| {
        let window = std::env::var("WAL_WINDOW_SIZE").unwrap_or_default();
        let aspect = match window.trim() {
            "aspect" => crate::screenshot::aspect_setting(),
            "picture" => crate::screenshot::d3d9_aspect(),
            _ => None,
        };
        aspect.map(|(w, h)| (w as i32, h as i32))
    });
    if let Some((dw, dh)) = source_aspect.filter(|&(w, h)| desktop && w > 0 && h > 0) {
        let (dw, dh) = (dw as u64, dh as u64);
        let (fw, fh) = if pw as u64 * dh > ph as u64 * dw {
            ((ph as u64 * dw / dh) as u32, ph)
        } else {
            (pw, (pw as u64 * dh / dw) as u32)
        };
        x += (pw - fw) / 2;
        y += (ph - fh) / 2;
        pw = fw;
        ph = fh;
    }
    let (color, black) = border.thickness(sw, sh);
    let sx = |x: u32| (x as u64 * w as u64 / sw.max(1) as u64) as i32;
    let sy = |y: u32| (y as u64 * h as u64 / sh.max(1) as u64) as i32;
    Some((
        [sx(x), sy(y), sx(x + pw), sy(y + ph)],
        sx(color + black),
        sy(color + black),
        border.rgb,
    ))
}

pub fn outline(w: u32, h: u32) -> (i32, i32) {
    let Some(border) = config() else {
        return (0, 0);
    };
    let screen = std::env::var("WAL_SINDEN_SCREEN")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
        })
        .unwrap_or((w, h));
    let (_, black) = border.thickness(screen.0, screen.1);
    (
        (black as u64 * w as u64 / screen.0.max(1) as u64) as i32,
        (black as u64 * h as u64 / screen.1.max(1) as u64) as i32,
    )
}

/// The colored ring's four strips, leaving its black outline and interior untouched.
pub fn strips(w: i32, h: i32, bx: i32, by: i32, ox: i32, oy: i32) -> [[i32; 4]; 4] {
    [
        [ox, oy, w - ox, by],
        [ox, h - by, w - ox, h - oy],
        [ox, by, bx, h - by],
        [w - bx, by, w - ox, h - by],
    ]
}

/// The separate-window bezel, composed once into a desktop-sized opaque backdrop. GPU
/// presentation places this behind the game, then scales the entire composition into the
/// frame. This preserves the same opening fit as bezel.rs, including portrait setups.
pub fn backdrop() -> Option<&'static crate::images_d3d11::Image> {
    static IMAGE: OnceLock<Option<crate::images_d3d11::Image>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            config()?;
            let path = std::env::var("WAL_BEZEL").ok()?;
            if matches!(path.trim(), "" | "0" | "black") {
                return None;
            }
            let image = match crate::images_d3d11::read_png(std::path::Path::new(&path)) {
                Ok(image) => image,
                Err(error) => {
                    crate::log!("sinden: bezel {error}");
                    return None;
                }
            };
            let (sw, sh) = crate::desktop::screen_size();
            if sw <= 0 || sh <= 0 || image.w == 0 || image.h == 0 {
                return None;
            }
            let (sw, sh) = (sw as u32, sh as u32);
            let aspect = crate::desktop::size()
                .map(|(w, h)| (w as u32, h as u32))
                .or(crate::screenshot::d3d9_aspect())
                .unwrap_or((sw, sh));
            let (gw, gh) = if sw as u64 * aspect.1 as u64 > sh as u64 * aspect.0 as u64 {
                (
                    (sh as u64 * aspect.0 as u64 / aspect.1.max(1) as u64) as u32,
                    sh,
                )
            } else {
                (
                    sw,
                    (sw as u64 * aspect.1 as u64 / aspect.0.max(1) as u64) as u32,
                )
            };
            let (gl, gr) = ((sw - gw) / 2, (sw + gw) / 2);
            let dark = |x: u32| {
                let at = ((image.h / 2 * image.w + x) * 4) as usize;
                let p = &image.bgra[at..at + 4];
                p[3] < 24 || (p[0] < 24 && p[1] < 24 && p[2] < 24)
            };
            let (mut hl, mut hr) = (image.w / 2, image.w / 2);
            while hl > 0 && dark(hl - 1) {
                hl -= 1;
            }
            while hr < image.w && dark(hr) {
                hr += 1;
            }
            let mode = std::env::var("WAL_BEZEL_FIT").unwrap_or_default();
            let fitted =
                matches!(mode.trim(), "1" | "stretch") && hl > 0 && hr < image.w && hr > hl;
            let stretch = mode.trim() == "stretch";
            let scale = (sw as f64 / image.w as f64).min(sh as f64 / image.h as f64);
            let (iw, ih) = (image.w as f64 * scale, image.h as f64 * scale);
            let mut bgra = vec![0; sw as usize * sh as usize * 4];
            for y in 0..sh {
                for x in 0..sw {
                    let (sx, sy) = if fitted {
                        let scale = sh as f64 / image.h as f64;
                        let sx = if x < gl {
                            if stretch {
                                x as f64 / gl.max(1) as f64 * hl as f64
                            } else {
                                hl as f64 + (x as f64 - gl as f64) / scale
                            }
                        } else if x >= gr {
                            if stretch {
                                hr as f64
                                    + (x - gr) as f64 / (sw - gr).max(1) as f64
                                        * (image.w - hr) as f64
                            } else {
                                hr as f64 + (x - gr) as f64 / scale
                            }
                        } else {
                            -1.0
                        };
                        (sx, y as f64 / scale)
                    } else {
                        (
                            (x as f64 - (sw as f64 - iw) / 2.0) / scale,
                            (y as f64 - (sh as f64 - ih) / 2.0) / scale,
                        )
                    };
                    let out = ((y * sw + x) * 4) as usize;
                    bgra[out + 3] = 255;
                    if sx >= 0.0 && sy >= 0.0 && sx < image.w as f64 && sy < image.h as f64 {
                        let src = ((sy as u32 * image.w + sx as u32) * 4) as usize;
                        let alpha = image.bgra[src + 3] as u32;
                        for channel in 0..3 {
                            bgra[out + channel] =
                                (image.bgra[src + channel] as u32 * alpha / 255) as u8;
                        }
                    }
                }
            }
            let _ = gh;
            crate::log!("sinden: bezel {path} composed into the {sw}x{sh} desktop");
            Some(crate::images_d3d11::Image { w: sw, h: sh, bgra })
        })
        .as_ref()
}
