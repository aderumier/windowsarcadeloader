//! K-On! After School Rhythm Selection's card scanner (`WAL_TYPEX_KON_CARDS=1`): a virtual card.
//!
//! The cabinet sells collectible cards (its dispenser: `WAL_TYPEX_JVS_LAYOUT=k-on`) and the
//! player scans up to 3 of them before a game. Each card carries a 13-digit JAN barcode; the
//! scanner is a keyboard: the game's window procedure feeds each `WM_KEYDOWN` to its reader
//! (RVA 0xCE70), which takes 13 digit keys typed less than a second apart followed by Enter,
//! Ctrl and J, then looks the code up in its card table (RVA 0xBCC4C: 304 entries of 24 bytes,
//! the code's address first; entries 0x62, 0x67 and 0x6A-0x99 are not accepted).
//!
//! The card is chosen on a picker drawn over the game (`screenshot::set_overlay`) with the
//! game's own picture of the card (`data\common\card_<n>.tga`): player 1's b1 / b2 browse the
//! cards (b3 / b4 by 10), `card` scans the selected one (its keys posted to the game window,
//! through the payload's key filter: the real keyboard is hidden from the game). The picker
//! shows while browsing and after a scan. The selected card is kept in
//! `WindowsLoader\kon-card.txt`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::d3d9_quads::{Image, draw_over, lost, quad, release, state_block, texture};
use wal_payload_common::{input_filter, log, screenshot};
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

type P = *mut c_void;

/// The card table: entries of 24 bytes, the barcode's address first, a null one last.
const TABLE: usize = 0xBCC4C;
const ENTRY: usize = 24;
const SELECTION: &str = "WindowsLoader\\kon-card.txt";
/// The picker shows this long after the last browse or scan.
const SHOWN: Duration = Duration::from_secs(4);
/// A held browse button repeats after this, then at this pace.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(80);

struct State {
    /// (card number, barcode) of the cards the game accepts.
    cards: Vec<(usize, String)>,
    selected: usize,
    /// Buttons held at the last frame and when the held browse button last moved.
    held: u32,
    moved: Instant,
    shown_until: Option<Instant>,
    window: usize,
    res: Option<Res>,
}

/// Device resources: the card textures made so far, a dark backdrop, the state block.
struct Res {
    device: usize,
    textures: HashMap<usize, (P, f32)>,
    backdrop: P,
    states: P,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub(crate) fn init() {
    if !std::env::var("WAL_TYPEX_KON_CARDS").is_ok_and(|v| v == "1") {
        return;
    }
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let cards = unsafe { table(exe) };
    if cards.is_empty() {
        log!("kon: no card table at {TABLE:#x}, no card scanner");
        return;
    }
    let saved = std::fs::read_to_string(SELECTION).ok().and_then(|s| s.trim().parse::<usize>().ok());
    let selected = saved.and_then(|n| cards.iter().position(|(c, _)| *c == n)).unwrap_or(0);
    log!("kon: card scanner, {} cards, card {} selected", cards.len(), cards[selected].0);
    // the real keyboard hidden (the game reads it for the scanner), the payload's keys let through
    input_filter::init(exe, |_| false);
    *STATE.lock().unwrap() = Some(State {
        cards,
        selected,
        held: 0,
        moved: Instant::now(),
        shown_until: None,
        window: 0,
        res: None,
    });
    screenshot::set_overlay(draw, reset);
}

/// The cards of the game's table it accepts (the rule of its lookup).
unsafe fn table(exe: usize) -> Vec<(usize, String)> {
    let mut cards = Vec::new();
    for n in 0..1000 {
        let code = unsafe { ((exe + TABLE + n * ENTRY) as *const *const u8).read() };
        if code.is_null() {
            break;
        }
        let digits = unsafe { std::slice::from_raw_parts(code, 13) };
        if !digits.iter().all(u8::is_ascii_digit) {
            return Vec::new();
        }
        if n == 0x62 || n == 0x67 || (0x6A..=0x99).contains(&n) {
            continue;
        }
        cards.push((n, String::from_utf8_lossy(digits).into_owned()));
    }
    cards
}

/// Posts the barcode's keys as the scanner types them: the digits, Enter, Ctrl, J.
fn scan(window: P, code: &str) {
    let keys = code.bytes().map(u16::from).chain([0x0D, 0x11, b'J' as u16]);
    for vk in keys {
        input_filter::post_key(window, vk, true);
        input_filter::post_key(window, vk, false);
    }
}

/// Player 1's picker buttons: browsing, scanning.
fn update(state: &mut State) {
    const BROWSE: [(u32, isize); 4] = [(button::B1, -1), (button::B2, 1), (button::B3, -10), (button::B4, 10)];
    let stick = wal_payload_common::input(0);
    let now = Instant::now();
    let mut held = 0;
    for (b, _) in BROWSE {
        if stick.pressed(b) {
            held |= b;
        }
    }
    if stick.pressed(button::CARD) {
        held |= button::CARD;
    }
    let pressed = held & !state.held;
    let mut step = 0;
    for (b, n) in BROWSE {
        if pressed & b != 0 {
            step += n;
            state.moved = now + REPEAT_DELAY - REPEAT;
        } else if held & b != 0 && now.duration_since(state.moved) >= REPEAT {
            step += n;
            state.moved = now;
        }
    }
    if step != 0 {
        let len = state.cards.len() as isize;
        state.selected = (state.selected as isize + step).rem_euclid(len) as usize;
        state.shown_until = Some(now + SHOWN);
        let _ = std::fs::write(SELECTION, state.cards[state.selected].0.to_string());
    }
    if pressed & button::CARD != 0 && state.window != 0 {
        let (n, code) = &state.cards[state.selected];
        log!("kon: card {n} scanned ({code})");
        scan(state.window as P, code);
        state.shown_until = Some(now + SHOWN);
    }
    state.held = held;
}

/// The game's picture of card `n` (24-bit TGA, bottom-up), opaque BGRA.
fn picture(n: usize) -> Option<Image> {
    let data = std::fs::read(format!("data\\common\\card_{n:02}.tga")).ok()?;
    let header = data.get(..18)?;
    let (id, kind, bpp) = (header[0] as usize, header[2], header[16]);
    let (w, h) = (u16::from_le_bytes([header[12], header[13]]) as usize, u16::from_le_bytes([header[14], header[15]]) as usize);
    if kind != 2 || bpp != 24 {
        return None;
    }
    let pixels = data.get(18 + id..18 + id + w * h * 3)?;
    let top_down = header[17] & 0x20 != 0;
    let mut bgra = vec![0xFF; w * h * 4];
    for y in 0..h {
        let src = if top_down { y } else { h - 1 - y };
        for x in 0..w {
            bgra[(y * w + x) * 4..][..3].copy_from_slice(&pixels[(src * w + x) * 3..][..3]);
        }
    }
    Some(Image { w: w as u32, h: h as u32, bgra })
}

unsafe fn reset(_dev: P) {
    if let Some(state) = STATE.lock().unwrap().as_mut()
        && let Some(res) = state.res.take()
    {
        for (t, _) in res.textures.into_values() {
            unsafe { release(t) };
        }
        unsafe {
            release(res.backdrop);
            release(res.states);
        }
    }
}

unsafe fn draw(dev: P, window: P) -> Option<P> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.as_mut()?;
    state.window = window as usize;
    update(state);
    if state.shown_until.is_none_or(|t| Instant::now() >= t) {
        return None;
    }
    unsafe {
        if lost(dev) || state.res.as_ref().is_some_and(|r| r.device != dev as usize) {
            drop(guard);
            reset(dev);
            return None;
        }
        if state.res.is_none() {
            let states = state_block(dev);
            if states.is_null() {
                return None;
            }
            let backdrop = texture(dev, &Image { w: 1, h: 1, bgra: vec![0, 0, 0, 0xB0] });
            state.res = Some(Res { device: dev as usize, textures: HashMap::new(), backdrop, states });
        }
        let len = state.cards.len();
        // the selected card, its neighbours smaller
        let shown = [(len - 1, 0.28f32, -1.0f32), (1, 0.28, 1.0), (0, 0.45, 0.0)].map(|(offset, size, side)| (state.cards[(state.selected + offset) % len].0, size, side));
        let res = state.res.as_mut().unwrap();
        for (n, _, _) in shown {
            res.textures.entry(n).or_insert_with(|| match picture(n) {
                Some(image) => (texture(dev, &image), image.w as f32 / image.h as f32),
                None => (std::ptr::null_mut(), 0.733),
            });
        }
        let res = state.res.as_ref().unwrap();
        draw_over(dev, res.states, |w, h| {
            let (w, h) = (w as f32, h as f32);
            let big = 0.45 * h;
            let panel_w = big * 0.733 * 2.4;
            quad(dev, res.backdrop, ((w - panel_w) / 2.0, h * 0.5 - big * 0.6, panel_w, big * 1.2));
            for (n, size, side) in shown {
                let &(tex, aspect) = &res.textures[&n];
                if tex.is_null() {
                    continue;
                }
                let (ch, cw) = (size * h, size * h * aspect);
                let x = w / 2.0 + side * (big * 0.733 / 2.0 + cw / 2.0 + 0.02 * h) - cw / 2.0;
                quad(dev, tex, (x, h * 0.5 - ch / 2.0, cw, ch));
            }
        });
    }
    None
}
