//! Won!Tertainment Music Channel's cards: a virtual card on the barcode scanner (COM3).
//!
//! A card's barcode is 12 decimal digits read as a 48-bit hex value, its low byte of the high
//! word its family. The game's card tables (in its data, one per family: entries with the card's
//! code twice at +8 / +0x10, its name's address at +0x18): Puppy (0x11, the dogs), Outfit (0x21),
//! Accessory (0x22), Miracle Power (0x31); the operators' version cards (0x23) are left out.
//!
//! The card is chosen on an overlay drawn over the game (`screenshot::set_overlay`): up / down
//! browse the family's cards (held: repeated), left / right change the family, `card` scans the
//! selected one (its code sent by the scanner). The overlay shows the family, the card's number
//! and the names around it (the game's own Japanese names, drawn with GDI); it shows while
//! browsing and after a scan. The selections are kept in `WindowsLoader\won-cards.txt`.
//! The common crosshair (`WAL_CROSSHAIR`: the touch's place) is drawn from the same overlay.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::d3d9_quads::{Image, draw_over, lost, quad, release, state_block, texture};
use wal_payload_common::{log, screenshot, serial};
use wal_protocol::button;
use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DIB_RGB_COLORS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FillRect, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::Foundation::RECT;

type P = *mut c_void;

/// The card families: (name, table address, entry size, entries).
const FAMILIES: [(&str, usize, usize, usize); 4] = [
    ("Puppy", 0x84F660, 0x108, 151),
    ("Outfit", 0x859218, 0xE0, 325),
    ("Accessory", 0x86AE78, 0xC8, 203),
    ("Miracle Power", 0x874FE0, 0x30, 32),
];
const SELECTION: &str = "WindowsLoader\\won-cards.txt";
const SHOWN: Duration = Duration::from_secs(4);
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(80);
/// The panel's size in pixels (drawn over the bottom of the 640x480 picture, scaled with it).
const PANEL: (i32, i32) = (600, 96);

struct Card {
    code: String,
    name: Vec<u16>,
}

struct State {
    families: Vec<(&'static str, Vec<Card>)>,
    family: usize,
    selected: Vec<usize>,
    held: u32,
    moved: Instant,
    shown_until: Option<Instant>,
    /// The panel's picture is out of date.
    dirty: bool,
    res: Option<Res>,
    /// The common crosshair (`WAL_CROSSHAIR`, the touch's place) drawn from this overlay.
    crosshair: bool,
}

struct Res {
    device: usize,
    panel: P,
    states: P,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub(crate) fn init() {
    let families: Vec<_> = FAMILIES.iter().map(|&(name, base, size, count)| (name, unsafe { table(base, size, count) })).collect();
    if families.iter().any(|(_, cards)| cards.is_empty()) {
        log!("wontertainment: card tables not found, no card scanner");
        return;
    }
    let saved: Vec<usize> = std::fs::read_to_string(SELECTION).unwrap_or_default().split_whitespace().filter_map(|n| n.parse().ok()).collect();
    let family = saved.first().copied().filter(|f| *f < families.len()).unwrap_or(0);
    let selected = families.iter().enumerate().map(|(i, (_, cards))| saved.get(i + 1).copied().filter(|n| *n < cards.len()).unwrap_or(0)).collect();
    log!("wontertainment: {} cards ({})", families.iter().map(|(_, c)| c.len()).sum::<usize>(), families.iter().map(|(n, c)| format!("{n} {}", c.len())).collect::<Vec<_>>().join(", "));
    let crosshair = wal_payload_common::crosshair::init_chained();
    *STATE.lock().unwrap() = Some(State { families, family, selected, held: 0, moved: Instant::now(), shown_until: None, dirty: true, res: None, crosshair });
    screenshot::set_overlay(draw, reset);
}

/// A family's cards from the game's table (entries without a code skipped).
unsafe fn table(base: usize, size: usize, count: usize) -> Vec<Card> {
    let mut cards = Vec::new();
    for i in 0..count {
        let entry = base + i * size;
        let (low, high) = unsafe { (*((entry + 8) as *const u32), *((entry + 12) as *const u32)) };
        if low == 0 && high == 0 {
            continue;
        }
        let name = unsafe { *((entry + 0x18) as *const usize) };
        cards.push(Card { code: format!("{high:04X}{low:08X}"), name: unsafe { sjis(name) } });
    }
    cards
}

/// The Shift-JIS string at `at` as UTF-16.
unsafe fn sjis(at: usize) -> Vec<u16> {
    if at == 0 {
        return Vec::new();
    }
    let len = (0..80).find(|&i| unsafe { *((at + i) as *const u8) } == 0).unwrap_or(80);
    let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, len) };
    let mut wide = vec![0u16; len + 1];
    let n = unsafe { windows_sys::Win32::Globalization::MultiByteToWideChar(932, 0, bytes.as_ptr(), len as i32, wide.as_mut_ptr(), wide.len() as i32) };
    wide.truncate(n.max(0) as usize);
    wide
}

fn save(state: &State) {
    let line = std::iter::once(state.family).chain(state.selected.iter().copied()).map(|n| n.to_string()).collect::<Vec<_>>().join(" ");
    let _ = std::fs::write(SELECTION, line);
}

/// Player 1's picker inputs.
fn update(state: &mut State) {
    const BROWSE: [(u32, isize); 2] = [(button::UP, -1), (button::DOWN, 1)];
    let stick = wal_payload_common::input(0);
    let now = Instant::now();
    let held = [button::UP, button::DOWN, button::LEFT, button::RIGHT, button::CARD].into_iter().filter(|&b| stick.pressed(b)).fold(0, |a, b| a | b);
    let pressed = held & !state.held;
    state.held = held;
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
    let families = state.families.len() as isize;
    let turn = if pressed & button::LEFT != 0 { -1 } else if pressed & button::RIGHT != 0 { 1 } else { 0 };
    if turn != 0 {
        state.family = (state.family as isize + turn).rem_euclid(families) as usize;
    }
    if step != 0 {
        let len = state.families[state.family].1.len() as isize;
        let s = &mut state.selected[state.family];
        *s = (*s as isize + step).rem_euclid(len) as usize;
    }
    if step != 0 || turn != 0 {
        state.shown_until = Some(now + SHOWN);
        state.dirty = true;
        save(state);
    }
    if pressed & button::CARD != 0 {
        let card = &state.families[state.family].1[state.selected[state.family]];
        log!("wontertainment: card {} scanned ({})", card.code, String::from_utf16_lossy(&card.name));
        serial::push("COM3", format!("{}\r\n", card.code).as_bytes());
        state.shown_until = Some(now + SHOWN);
    }
}

/// The panel's picture: the family and card number, the selected card's name between its
/// neighbours'.
fn panel(state: &State) -> Image {
    let (w, h) = PANEL;
    let (name, cards) = &state.families[state.family];
    let n = state.selected[state.family];
    let at = |offset: isize| &cards[(n as isize + offset).rem_euclid(cards.len() as isize) as usize].name;
    let wide = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..std::mem::zeroed() };
        let mut bits: P = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old_bitmap = SelectObject(dc, bitmap);
        let brush = CreateSolidBrush(0x30_20_20);
        FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, brush);
        DeleteObject(brush);
        SetBkMode(dc, TRANSPARENT as i32);
        let font = |height: i32| CreateFontW(height, 0, 0, 0, 700, 0, 0, 0, 128, 0, 0, 4, 0, windows_sys::w!("MS Gothic"));
        let (big, small) = (font(30), font(20));
        let line = |text: &[u16], font, color: u32, rect: RECT| {
            let old = SelectObject(dc, font);
            SetTextColor(dc, color);
            let mut r = rect;
            DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut r, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(dc, old);
        };
        let header = wide(&format!("< {name} >   {} / {}   (card: scan)", n + 1, cards.len()));
        line(&header, small, 0x60_D0_FF, RECT { left: 12, top: 2, right: w - 12, bottom: 24 });
        line(at(-1), small, 0xA0_A0_A0, RECT { left: 30, top: 24, right: w - 12, bottom: 44 });
        line(at(0), big, 0xFF_FF_FF, RECT { left: 12, top: 44, right: w - 12, bottom: 76 });
        line(at(1), small, 0xA0_A0_A0, RECT { left: 30, top: 76, right: w - 12, bottom: 96 });
        DeleteObject(big);
        DeleteObject(small);
        let mut bgra = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        for px in bgra.chunks_mut(4) {
            px[3] = 0xE0;
        }
        SelectObject(dc, old_bitmap);
        DeleteObject(bitmap);
        DeleteDC(dc);
        Image { w: w as u32, h: h as u32, bgra }
    }
}

unsafe fn reset(dev: P) {
    if STATE.lock().unwrap().as_ref().is_some_and(|s| s.crosshair) {
        unsafe { wal_payload_common::crosshair::reset_d3d9(dev) };
    }
    if let Some(state) = STATE.lock().unwrap().as_mut()
        && let Some(res) = state.res.take()
    {
        unsafe {
            release(res.panel);
            release(res.states);
        }
        state.dirty = true;
    }
}

unsafe fn draw(dev: P, window: P) -> Option<P> {
    if STATE.lock().unwrap().as_ref().is_some_and(|s| s.crosshair) {
        unsafe { wal_payload_common::crosshair::draw_d3d9(dev, window) };
    }
    let mut guard = STATE.lock().unwrap();
    let state = guard.as_mut()?;
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
            state.res = Some(Res { device: dev as usize, panel: std::ptr::null_mut(), states });
        }
        if state.dirty {
            let image = panel(state);
            let res = state.res.as_mut().unwrap();
            release(res.panel);
            res.panel = texture(dev, &image);
            state.dirty = false;
        }
        let res = state.res.as_ref().unwrap();
        if res.panel.is_null() {
            return None;
        }
        draw_over(dev, res.states, |w, h| {
            let (w, h) = (w as f32, h as f32);
            let pw = w * 0.94;
            let ph = pw * PANEL.1 as f32 / PANEL.0 as f32;
            quad(dev, res.panel, ((w - pw) / 2.0, h - ph - h * 0.03, pw, ph));
        });
    }
    None
}
