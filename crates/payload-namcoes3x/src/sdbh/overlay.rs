//! The card binder and the play mat panel, drawn over the game (`screenshot::set_overlay`,
//! each a canvas of [`ui`] in a texture, redrawn when something changed).
//!
//! The binder (b5, or the keyboard's key mapped to it) opens over the game, which takes no
//! button while it is open: every printed card of the card pack, sorted by tabs (all, the
//! characters with the most cards, the others, the cards printed only in Japan), the card
//! under the frame large at the right with the deck. b1 puts the card in the deck or takes it
//! out (5 cards), b2 changes the pictures' frame (circle, rectangle), b7 / b8 the tab, b4
//! empties the deck, b3 or b5 closes it. The deck is kept in `WindowsLoader\sdbh\deck.txt`.
//!
//! The play mat panel shows while the game reads the mat (card registration, battles): the
//! deck as it lies, the attack area (near the screen) at the top, the support area at the
//! bottom. b7 / b8 pick a card, the right stick moves it (up: attack, down: support; left and
//! right shake it), b6 held with the right stick turns it, b6 alone cycles the panel's size
//! (large, small, hidden).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::d3d9_quads::{Image, draw_over, lost, quad, release, state_block, texture};
use wal_payload_common::{log, screenshot};
use wal_protocol::{Axis, StickState, button};

use super::cards::{self, Art, Style};
use super::fp::{self, DECK_MAX, MAT};
use super::ui::{Canvas, look};

type P = *mut std::ffi::c_void;

const CARD_ASPECT: f32 = 712.0 / 488.0;
/// A held direction repeats after this, then at this pace.
const REPEAT_DELAY: Duration = Duration::from_millis(350);
const REPEAT: Duration = Duration::from_millis(85);
/// The characters given a tab of their own.
const CHARACTER_TABS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelSize {
    Large,
    Small,
    Hidden,
}

struct Tab {
    name: String,
    cards: Vec<usize>,
}

struct State {
    binder: bool,
    tabs: Vec<Tab>,
    tab: usize,
    focus: Vec<usize>,
    /// First row shown, per tab.
    scroll: Vec<usize>,
    cols: usize,
    rows: usize,
    style: Style,
    deck: Vec<u16>,
    note: Option<(String, Instant)>,
    panel: PanelSize,
    picked: usize,
    /// Buttons held at the last step, when the held direction last moved.
    held: u32,
    moved: Instant,
    rotating: bool,
    /// The panel shown at the last redraw (the game reading the mat).
    panel_shown: bool,
    res: Option<Res>,
}

/// Device resources: the state block, the binder's and the panel's textures.
struct Res {
    device: usize,
    states: P,
    binder: Option<(P, u32, u32)>,
    panel: Option<(P, u32, u32, f32, f32)>,
}

unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Something changed: the canvases are drawn again.
static DIRTY: AtomicBool = AtomicBool::new(true);

fn dirty() {
    DIRTY.store(true, Ordering::Relaxed);
}

fn deck_path() -> std::path::PathBuf {
    super::save_dir().join("deck.txt")
}

fn load_deck() -> Vec<u16> {
    let ids: Vec<u16> = std::fs::read_to_string(deck_path())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| u16::from_str_radix(l.split_whitespace().next()?, 16).ok())
        .filter(|id| cards::find(*id).is_some())
        .take(DECK_MAX)
        .collect();
    if !ids.is_empty() {
        return ids;
    }
    // a first deck: the pack's first cards
    cards::all().iter().filter(|c| !c.japan).take(DECK_MAX).map(|c| c.id).collect()
}

fn save_deck(deck: &[u16]) {
    let mut text = String::from("# The deck on the play mat (up to 5 cards): Melon ID (hex) and name. Made by the card binder.\n");
    for id in deck {
        text.push_str(&format!("{id:04X} {}\n", cards::find(*id).map_or("", |c| c.name.as_str())));
    }
    let path = deck_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        log!("sdbh: deck not saved: {}: {e}", path.display());
    }
}

fn make_tabs() -> Vec<Tab> {
    let all = cards::all();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for c in all {
        let ch = cards::character(&c.name);
        match counts.iter_mut().find(|(n, _)| *n == ch) {
            Some(e) => e.1 += 1,
            None => counts.push((ch, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let main: Vec<&str> = counts.iter().take(CHARACTER_TABS).map(|c| c.0).collect();
    let mut tabs = vec![Tab { name: "All".into(), cards: (0..all.len()).collect() }];
    for ch in &main {
        tabs.push(Tab { name: ch.to_string(), cards: (0..all.len()).filter(|&i| cards::character(&all[i].name) == *ch).collect() });
    }
    tabs.push(Tab { name: "Others".into(), cards: (0..all.len()).filter(|&i| !main.contains(&cards::character(&all[i].name))).collect() });
    tabs.push(Tab { name: "Japan".into(), cards: (0..all.len()).filter(|&i| all[i].japan).collect() });
    tabs
}

pub fn init() {
    let deck = load_deck();
    fp::lay(&deck);
    let tabs = make_tabs();
    let n = tabs.len();
    log!("sdbh: deck of {} card(s) on the play mat", deck.len());
    *STATE.lock().unwrap() = Some(State {
        binder: false,
        tabs,
        tab: 0,
        focus: vec![0; n],
        scroll: vec![0; n],
        cols: 8,
        rows: 3,
        style: Style::Circle,
        deck,
        note: None,
        panel: PanelSize::Large,
        picked: 0,
        held: 0,
        moved: Instant::now(),
        rotating: false,
        panel_shown: false,
        res: None,
    });
    cards::on_ready(dirty);
    screenshot::set_overlay(draw, reset);
}

fn say(state: &mut State, text: &str) {
    state.note = Some((text.to_string(), Instant::now() + Duration::from_secs(3)));
}

// ── input ───────────────────────────────────────────────────────────────────────────────────

/// One step of the player's controls (every 8 ms).
pub fn input(stick: &StickState) {
    let mut guard = STATE.lock().unwrap();
    let Some(state) = guard.as_mut() else { return };
    let now = Instant::now();
    let pressed = stick.buttons & !state.held;
    let released = state.held & !stick.buttons;
    let held_before = state.held;
    state.held = stick.buttons;

    if pressed & button::B5 != 0 {
        state.binder = !state.binder;
        super::io::BLOCKED.store(state.binder, Ordering::Relaxed);
        if !state.binder {
            close_binder(state);
        }
        dirty();
        return;
    }
    if state.binder {
        binder_input(state, stick, pressed, now);
        return;
    }
    let _ = held_before;
    mat_input(state, stick, pressed, released);
}

fn close_binder(state: &mut State) {
    save_deck(&state.deck);
    fp::lay(&state.deck);
    state.picked = state.picked.min(state.deck.len().saturating_sub(1));
    log!("sdbh: deck of {} card(s) on the play mat", state.deck.len());
}

fn binder_input(state: &mut State, stick: &StickState, pressed: u32, now: Instant) {
    const DIRECTIONS: [(u32, isize, isize); 4] = [(button::UP, 0, -1), (button::DOWN, 0, 1), (button::LEFT, -1, 0), (button::RIGHT, 1, 0)];
    for (b, dx, dy) in DIRECTIONS {
        let repeat = stick.pressed(b) && now.duration_since(state.moved) >= REPEAT_DELAY;
        if pressed & b != 0 || repeat {
            move_focus(state, dx, dy);
            state.moved = if pressed & b != 0 { now } else { now - REPEAT_DELAY + REPEAT };
            dirty();
        }
    }
    if pressed & button::B1 != 0 {
        if let Some(&card) = state.tabs[state.tab].cards.get(state.focus[state.tab]) {
            toggle_card(state, cards::all()[card].id);
        }
    }
    if pressed & button::B2 != 0 {
        state.style = if state.style == Style::Circle { Style::Rect } else { Style::Circle };
        dirty();
    }
    if pressed & button::B3 != 0 {
        state.binder = false;
        super::io::BLOCKED.store(false, Ordering::Relaxed);
        close_binder(state);
        dirty();
    }
    if pressed & button::B4 != 0 {
        state.deck.clear();
        say(state, "Deck emptied");
        dirty();
    }
    let n = state.tabs.len();
    if pressed & button::B7 != 0 {
        state.tab = (state.tab + n - 1) % n;
        dirty();
    }
    if pressed & button::B8 != 0 {
        state.tab = (state.tab + 1) % n;
        dirty();
    }
}

fn move_focus(state: &mut State, dx: isize, dy: isize) {
    let n = state.tabs[state.tab].cards.len();
    if n == 0 {
        return;
    }
    let cols = state.cols.max(1) as isize;
    let f = state.focus[state.tab] as isize;
    let to = f + dx + dy * cols;
    let to = if dy > 0 && to >= n as isize && f / cols < (n as isize - 1) / cols { n as isize - 1 } else { to };
    if (0..n as isize).contains(&to) {
        state.focus[state.tab] = to as usize;
    }
}

fn toggle_card(state: &mut State, id: u16) {
    if let Some(i) = state.deck.iter().position(|&d| d == id) {
        state.deck.remove(i);
    } else if state.deck.len() < DECK_MAX {
        state.deck.push(id);
    } else {
        say(state, "A deck holds 5 cards: take one out first");
    }
    dirty();
}

fn mat_input(state: &mut State, stick: &StickState, pressed: u32, released: u32) {
    let n = state.deck.len();
    if n == 0 {
        return;
    }
    if pressed & button::B7 != 0 {
        state.picked = (state.picked + n - 1) % n;
        dirty();
    }
    if pressed & button::B8 != 0 {
        state.picked = (state.picked + 1) % n;
        dirty();
    }
    state.picked = state.picked.min(n - 1);
    let rx = stick.axis(Axis::RightX) as f32 / 32767.0;
    let ry = stick.axis(Axis::RightY) as f32 / 32767.0;
    let deflected = rx.hypot(ry) > 0.35;
    let mut mat = MAT.lock().unwrap();
    let count = mat.cards.len();
    let Some(card) = mat.cards.get_mut(state.picked) else { return };
    let column = fp::column_x(state.picked, count);
    if stick.pressed(button::B6) {
        // turning: the card points where the stick points
        if rx.hypot(ry) > 0.6 {
            let turns = rx.atan2(-ry) / std::f32::consts::TAU;
            card.angle = ((turns * 4096.0).round() as i32).rem_euclid(4096);
            state.rotating = true;
            dirty();
        }
        return;
    }
    if released & button::B6 != 0 {
        if state.rotating {
            card.angle = 0;
        } else {
            state.panel = match state.panel {
                PanelSize::Large => PanelSize::Small,
                PanelSize::Small => PanelSize::Hidden,
                PanelSize::Hidden => PanelSize::Large,
            };
        }
        state.rotating = false;
        dirty();
    }
    // sliding: up toward the attack area, down toward the support area; left / right shake
    let x = column + (rx * 70.0) as i32;
    let y = if ry.abs() > 0.35 { (card.y + (ry * 9.0) as i32).clamp(fp::ATTACK_Y, fp::SUPPORT_Y) } else { card.y };
    if (x, y) != (card.x, card.y) {
        card.x = x;
        card.y = y;
        dirty();
    }
    let _ = deflected;
}

// ── drawing ─────────────────────────────────────────────────────────────────────────────────

unsafe fn reset(_dev: P) {
    if let Some(state) = STATE.lock().unwrap().as_mut()
        && let Some(res) = state.res.take()
    {
        unsafe {
            if let Some((t, ..)) = res.binder {
                release(t);
            }
            if let Some((t, ..)) = res.panel {
                release(t);
            }
            release(res.states);
        }
    }
    dirty();
}

unsafe fn draw(dev: P, _window: P) -> Option<P> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.as_mut()?;
    let show_panel = fp::reading() && state.panel != PanelSize::Hidden && !state.deck.is_empty();
    if show_panel != state.panel_shown {
        state.panel_shown = show_panel;
        dirty();
    }
    if state.note.as_ref().is_some_and(|(_, until)| Instant::now() >= *until) {
        state.note = None;
        dirty();
    }
    if !state.binder && !show_panel {
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
            state.res = Some(Res { device: dev as usize, states, binder: None, panel: None });
        }
        let states = state.res.as_ref().unwrap().states;
        draw_over(dev, states, |w, h| {
            let redraw = DIRTY.swap(false, Ordering::Relaxed);
            let scale = ((h as f32 / 600.0).round()).max(1.0);
            let res = state.res.as_mut().unwrap();
            if show_panel {
                let stale = res.panel.as_ref().is_none_or(|p| p.1 != w || redraw);
                if stale {
                    if let Some((t, ..)) = res.panel.take() {
                        release(t);
                    }
                    let (canvas, x, y) = panel_canvas(state, w, h, scale);
                    let tex = texture(dev, &Image { w: canvas.w as u32, h: canvas.h as u32, bgra: canvas.bgra() });
                    if !tex.is_null() {
                        state.res.as_mut().unwrap().panel = Some((tex, w, canvas.h as u32, x, y));
                    }
                }
                if let Some((tex, _, ph, x, y)) = state.res.as_ref().unwrap().panel {
                    let pw = panel_width(w, h, scale, state.panel);
                    quad(dev, tex, (x, y, pw, ph as f32));
                }
            }
            if state.binder {
                let res = state.res.as_mut().unwrap();
                let stale = res.binder.as_ref().is_none_or(|b| b.1 != w || b.2 != h || redraw);
                if stale {
                    if let Some((t, ..)) = res.binder.take() {
                        release(t);
                    }
                    let canvas = binder_canvas(state, w as i32, h as i32, scale);
                    let tex = texture(dev, &Image { w, h, bgra: canvas.bgra() });
                    if !tex.is_null() {
                        state.res.as_mut().unwrap().binder = Some((tex, w, h));
                    }
                }
                if let Some((tex, ..)) = state.res.as_ref().unwrap().binder {
                    quad(dev, tex, (0.0, 0.0, w as f32, h as f32));
                }
            }
        });
    }
    None
}

fn px(scale: f32, points: f32) -> f32 {
    (points * scale).round()
}

/// A key cap and what it does; the width taken.
fn key_hint(c: &mut Canvas, x: f32, y: f32, key: &str, what: &str, s: f32) -> f32 {
    let kw = c.measure(key);
    let pad = px(s, 4.0);
    let h = c.font + px(s, 4.0);
    c.frame((x, y), (x + kw + 2.0 * pad, y + h), px(s, 2.0), 0, look::OUTLINE, 1.0);
    c.write(x + pad, y + px(s, 2.0), look::DIM, key);
    let tx = x + kw + 2.0 * pad + px(s, 5.0);
    c.write(tx, y + px(s, 2.0), look::FAINT, what);
    tx + c.measure(what) + px(s, 14.0) - x
}

/// A card's picture, or its drawn tile while it loads (or without the card pack).
fn card_picture(c: &mut Canvas, id: u16, style: Style, a: (f32, f32), b: (f32, f32), s: f32) {
    let (w, h) = ((b.0 - a.0).round() as u32, (b.1 - a.1).round() as u32);
    match cards::picture(id, style, w, h) {
        Art::Ready(p) => c.picture(&p, a.0, a.1, w as f32, h as f32, 1.0),
        art => {
            c.frame(a, b, px(s, 4.0), look::TILE, look::LINE, 1.0);
            let code = format!("{id:04X}");
            let cw = c.measure(&code);
            c.write((a.0 + b.0 - cw) / 2.0, a.1 + (b.1 - a.1) * 0.25, look::alpha(look::TITLE, 0.5), &code);
            if let Some(card) = cards::find(id) {
                let wrap = b.0 - a.0 - px(s, 8.0);
                let lines = c.wrap(&card.name, wrap);
                let mut y = a.1 + (b.1 - a.1) * 0.5;
                for line in lines.iter().take(((b.1 - y) / c.font).max(0.0) as usize) {
                    let line = c.fitted(line, wrap);
                    let lw = c.measure(&line);
                    c.write((a.0 + b.0 - lw) / 2.0, y, look::DIM, &line);
                    y += c.font;
                }
            }
            if matches!(art, Art::Loading) {
                let dw = c.measure("...");
                c.write((a.0 + b.0 - dw) / 2.0, b.1 - c.font - px(s, 4.0), look::FAINT, "...");
            }
        }
    }
}

fn binder_canvas(state: &mut State, w: i32, h: i32, s: f32) -> Canvas {
    let mut c = Canvas::new(w, h, s);
    let font = c.font;
    c.fill_rect(0.0, 0.0, w as f32, h as f32, look::SHADE);
    let m = px(s, 16.0).max((w.min(h) as f32 * 0.04).floor());
    let (mut ax, ay, mut bx, by) = (m, m, w as f32 - m, h as f32 - m);
    if bx - ax > px(s, 1500.0) {
        let cut = ((bx - ax) - px(s, 1500.0)) / 2.0;
        ax += cut;
        bx -= cut;
    }
    c.frame((ax, ay), (bx, by), px(s, 6.0), look::FRAME, look::OUTLINE, 1.0);
    let pad = px(s, 14.0);
    let (x0, x1) = (ax + pad, bx - pad);
    let mut y = ay + pad;

    // the head: the title, the game, whether the game reads the mat now
    let tw = c.spaced(x0, y + px(s, 2.0), look::TITLE, "CARD BINDER", px(s, 1.5));
    c.write(x0 + tw + px(s, 10.0), y + px(s, 2.0), look::FAINT, "Super Dragon Ball Heroes");
    let reading = fp::reading();
    let status = if reading { "The game reads the play mat now" } else { "Cards wait for the play mat" };
    let sw = c.measure(status);
    let seg_h = font + px(s, 8.0);
    let dot = px(s, 6.0);
    c.fill_rect(x1 - sw - dot - px(s, 6.0), y + (seg_h - dot) / 2.0, x1 - sw - px(s, 6.0), y + (seg_h + dot) / 2.0, if reading { look::ACTION } else { look::ACCENT });
    c.write(x1 - sw, y + (seg_h - font) / 2.0, if reading { look::DIM } else { look::FAINT }, status);
    y += seg_h + px(s, 10.0);

    // the tabs, LB and RB at their ends
    let tab_h = font + px(s, 10.0);
    let lb = "LB";
    let lbw = c.measure(lb) + px(s, 8.0);
    c.frame((x0, y + px(s, 3.0)), (x0 + lbw, y + tab_h - px(s, 3.0)), px(s, 2.0), 0, look::OUTLINE, 1.0);
    c.write(x0 + px(s, 4.0), y + (tab_h - font) / 2.0, look::DIM, lb);
    let mut tx = x0 + lbw + px(s, 8.0);
    for (k, tab) in state.tabs.iter().enumerate() {
        let count = tab.cards.len().to_string();
        let tw = c.measure(&tab.name) + c.measure(&count) + px(s, 28.0);
        let on = k == state.tab;
        c.frame((tx, y), (tx + tw, y + tab_h), px(s, 4.0), if on { look::alpha(look::ACTION, 0.16) } else { 0 }, if on { look::ACTION } else { look::LINE }, 1.0);
        c.write(tx + px(s, 10.0), y + (tab_h - font) / 2.0, if on { look::INK } else { look::DIM }, &tab.name);
        let cw = c.measure(&count);
        c.write(tx + tw - px(s, 10.0) - cw, y + (tab_h - font) / 2.0, if on { look::ACTION } else { look::FAINT }, &count);
        tx += tw + px(s, 4.0);
    }
    c.frame((tx + px(s, 4.0), y + px(s, 3.0)), (tx + px(s, 4.0) + lbw, y + tab_h - px(s, 3.0)), px(s, 2.0), 0, look::OUTLINE, 1.0);
    c.write(tx + px(s, 8.0), y + (tab_h - font) / 2.0, look::DIM, "RB");
    y += tab_h + px(s, 10.0);

    // the foot: the keys, and a word when there is one
    let foot_h = font + px(s, 4.0);
    let foot_y = by - pad - foot_h;
    c.fill_rect(ax + 1.0, foot_y - px(s, 8.0), bx - 1.0, foot_y - px(s, 7.0), look::LINE);
    let mut kx = x0;
    for (key, what) in [("D-pad", "move"), ("A", "pick"), ("LB RB", "tab"), ("X", "frame"), ("Y", "empty deck"), ("B", "close")] {
        kx += key_hint(&mut c, kx, foot_y, key, what, s);
    }
    if let Some((note, _)) = &state.note {
        let nw = c.measure(note);
        c.write(x1 - nw, foot_y + px(s, 2.0), look::ACCENT, note);
    }

    // the body: the cards, at the right the one under the frame and the deck
    let side_w = ((x1 - x0) * 0.27).clamp(px(s, 200.0), px(s, 380.0)).floor();
    let body_bottom = foot_y - px(s, 16.0);
    draw_grid(&mut c, state, (x0 + px(s, 4.0), y + px(s, 4.0)), (x1 - side_w - px(s, 16.0), body_bottom), s);
    c.fill_rect(x1 - side_w - px(s, 8.0), y, x1 - side_w - px(s, 7.0), body_bottom, look::LINE);
    draw_side(&mut c, state, (x1 - side_w, y + px(s, 4.0)), (x1, body_bottom), s);
    c
}

fn draw_grid(c: &mut Canvas, state: &mut State, a: (f32, f32), b: (f32, f32), s: f32) {
    let font = c.font;
    let tab = state.tab;
    let n = state.tabs[tab].cards.len();
    let gap = px(s, 8.0);
    let min_tile = px(s, 84.0);
    let w = b.0 - a.0 - px(s, 10.0);
    let cols = (((w + gap) / (min_tile + gap)) as usize).max(2);
    let tw = ((w - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let th = (tw * CARD_ASPECT).floor();
    let row_h = th + font + px(s, 6.0) + gap;
    let rows = (((b.1 - a.1 + gap) / row_h) as usize).max(1);
    state.cols = cols;
    state.rows = rows;
    if n == 0 {
        c.write(a.0, a.1, look::FAINT, "No card here (install the card pack in the game's folder)");
        return;
    }
    let f = state.focus[tab].min(n - 1);
    state.focus[tab] = f;
    // the frame of focus kept in view
    let row = f / cols;
    let scroll = &mut state.scroll[tab];
    if row < *scroll {
        *scroll = row;
    }
    if row >= *scroll + rows {
        *scroll = row + 1 - rows;
    }
    let first = *scroll;
    for i in first * cols..((first + rows) * cols).min(n) {
        let (r, col) = (i / cols - first, i % cols);
        let card = &cards::all()[state.tabs[tab].cards[i]];
        let ta = (a.0 + col as f32 * (tw + gap), a.1 + r as f32 * row_h);
        let tb = (ta.0 + tw, ta.1 + th);
        card_picture(c, card.id, state.style, ta, tb, s);
        if let Some(k) = state.deck.iter().position(|&d| d == card.id) {
            c.frame(ta, tb, px(s, 4.0), look::alpha(look::ACTION, 0.18), look::ACTION, 2.0 * s);
            let bs = font + px(s, 6.0);
            let ba = (tb.0 - bs - px(s, 4.0), ta.1 + px(s, 4.0));
            c.frame(ba, (ba.0 + bs, ba.1 + bs), px(s, 3.0), look::ACTION, 0, 1.0);
            let num = (k + 1).to_string();
            let nw = c.measure(&num);
            c.write(ba.0 + (bs - nw) / 2.0, ba.1 + (bs - font) / 2.0, look::ON_ACTION, &num);
        }
        if card.japan {
            c.frame((ta.0 + px(s, 4.0), ta.1 + px(s, 4.0)), (ta.0 + px(s, 4.0) + c.measure("JP") + px(s, 6.0), ta.1 + px(s, 4.0) + font + px(s, 2.0)), px(s, 2.0), look::alpha(look::TILE, 0.85), look::OUTLINE, 1.0);
            c.write(ta.0 + px(s, 7.0), ta.1 + px(s, 5.0), look::DIM, "JP");
        }
        if i == f {
            c.frame((ta.0 - px(s, 3.0), ta.1 - px(s, 3.0)), (tb.0 + px(s, 3.0), tb.1 + px(s, 3.0)), px(s, 5.0), 0, look::ACCENT, 2.0 * s);
        }
        let in_deck = state.deck.contains(&card.id);
        let name = c.fitted(&card.name, tw);
        let nw = c.measure(&name);
        c.write(ta.0 + (tw - nw) / 2.0, tb.1 + px(s, 3.0), if i == f { look::INK } else if in_deck { look::ACTION } else { look::DIM }, &name);
    }
    // the scroll bar
    let total_rows = n.div_ceil(cols);
    if total_rows > rows {
        let x = b.0 - px(s, 3.0);
        let bar_h = b.1 - a.1;
        c.fill_rect(x - px(s, 1.0), a.1, x + px(s, 1.0), b.1, look::LINE);
        let thumb_h = (bar_h * rows as f32 / total_rows as f32).max(px(s, 24.0));
        let ty = a.1 + (bar_h - thumb_h) * (first as f32 / (total_rows - rows) as f32);
        c.frame((x - px(s, 3.0), ty), (x + px(s, 3.0), ty + thumb_h), px(s, 2.0), look::FAINT, 0, 1.0);
    }
}

fn draw_side(c: &mut Canvas, state: &State, a: (f32, f32), b: (f32, f32), s: f32) {
    let font = c.font;
    let w = b.0 - a.0;
    let mut y = a.1;
    let tab = &state.tabs[state.tab];
    if let Some(&ci) = tab.cards.get(state.focus[state.tab]) {
        let card = &cards::all()[ci];
        let ph = ((b.1 - a.1) * 0.42).min(w * CARD_ASPECT).floor();
        let pw = (ph / CARD_ASPECT).floor();
        let pa = (a.0 + ((w - pw) / 2.0).floor(), y);
        card_picture(c, card.id, state.style, pa, (pa.0 + pw, pa.1 + ph), s);
        y += ph + px(s, 8.0);
        for line in c.wrap(&card.name, w).into_iter().take(2) {
            c.write(a.0, y, look::INK, &line);
            y += font + px(s, 1.0);
        }
        y += px(s, 2.0);
        let detail = format!("{}  #{:04X}  {} frame", cards::character(&card.name), card.id, state.style.name());
        let detail = c.fitted(&detail, w);
        c.write(a.0, y, look::FAINT, &detail);
        y += font + px(s, 3.0);
        if card.japan {
            c.write(a.0, y, look::ACCENT, "Printed only in Japan");
            y += font + px(s, 3.0);
        }
        y += px(s, 8.0);
    }

    // the deck
    c.fill_rect(a.0, y, b.0, y + px(s, 1.0), look::LINE);
    y += px(s, 7.0);
    c.spaced(a.0, y, look::TITLE, "DECK", px(s, 1.5));
    let count = format!("{}/{DECK_MAX}", state.deck.len());
    let cw = c.measure(&count);
    c.write(b.0 - cw, y, look::FAINT, &count);
    y += font + px(s, 6.0);
    let row_h = font + px(s, 5.0);
    for (k, id) in state.deck.iter().enumerate() {
        if y + row_h > b.1 {
            break;
        }
        c.write(a.0 + px(s, 3.0), y + px(s, 2.0), look::ACTION, &(k + 1).to_string());
        let name = cards::find(*id).map_or_else(|| format!("{id:04X}"), |c| c.name.clone());
        let name = c.fitted(&name, w - px(s, 16.0));
        c.write(a.0 + px(s, 16.0), y + px(s, 2.0), look::INK, &name);
        y += row_h;
    }
    if state.deck.is_empty() {
        c.write(a.0, y + px(s, 2.0), look::FAINT, "A picks a card");
    }
}

fn panel_width(w: u32, h: u32, s: f32, size: PanelSize) -> f32 {
    let base = (w.min(h) as f32 * if size == PanelSize::Large { 0.42 } else { 0.26 }).round();
    base.max(px(s, 160.0))
}

/// The play mat panel: its canvas and where it goes (bottom left).
fn panel_canvas(state: &State, w: u32, h: u32, s: f32) -> (Canvas, f32, f32) {
    let pw = panel_width(w, h, s, state.panel);
    let mut c = Canvas::new(pw as i32, 1, s);
    let font = c.font;
    let pad = px(s, 8.0);
    let head = font + px(s, 6.0);
    let foot = if state.panel == PanelSize::Large { font + px(s, 8.0) } else { 0.0 };
    let mat_w = pw - 2.0 * pad;
    let mat_h = (mat_w * (fp::MAT_BOTTOM - fp::MAT_TOP) as f32 / (fp::MAT_RIGHT - fp::MAT_LEFT) as f32).round();
    let ph = (pad + head + mat_h + foot + pad).round();
    c = Canvas::new(pw as i32, ph as i32, s);
    c.frame((0.0, 0.0), (pw, ph), px(s, 5.0), look::FRAME, look::OUTLINE, 1.0);
    c.spaced(pad, pad, look::TITLE, "PLAY MAT", px(s, 1.5));
    let reading = "reading";
    let rw = c.measure(reading);
    c.write(pw - pad - rw, pad, look::ACTION, reading);
    let (mx, my) = (pad, pad + head);
    // the areas: attack (top, red) and support (bottom, blue)
    c.frame((mx, my), (mx + mat_w, my + mat_h), px(s, 3.0), look::alpha(look::TILE, 0.6), look::LINE, 1.0);
    let half = (mat_h / 2.0).round();
    c.fill_rect(mx + 1.0, my + 1.0, mx + mat_w - 1.0, my + half, look::alpha(look::ATTACK, 0.12));
    c.fill_rect(mx + 1.0, my + half, mx + mat_w - 1.0, my + mat_h - 1.0, look::alpha(look::SUPPORT, 0.12));
    c.fill_rect(mx + 1.0, my + half, mx + mat_w - 1.0, my + half + 1.0, look::LINE);
    if state.panel == PanelSize::Large {
        c.write(mx + px(s, 4.0), my + px(s, 3.0), look::alpha(look::ATTACK, 0.8), "ATTACK");
        let sw = c.measure("SUPPORT");
        c.write(mx + px(s, 4.0), my + mat_h - font - px(s, 3.0), look::alpha(look::SUPPORT, 0.9), "SUPPORT");
        let _ = sw;
    }
    // the cards where they lie
    let to_x = |x: i32| mx + (x - fp::MAT_LEFT) as f32 * mat_w / (fp::MAT_RIGHT - fp::MAT_LEFT) as f32;
    let to_y = |y: i32| my + (y - fp::MAT_TOP) as f32 * mat_h / (fp::MAT_BOTTOM - fp::MAT_TOP) as f32;
    let cw = fp::CARD_W as f32 * mat_w / (fp::MAT_RIGHT - fp::MAT_LEFT) as f32;
    let ch = fp::CARD_H as f32 * mat_h / (fp::MAT_BOTTOM - fp::MAT_TOP) as f32;
    let mat = MAT.lock().unwrap();
    for (k, card) in mat.cards.iter().enumerate() {
        let (cx, cy) = (to_x(card.x), to_y(card.y));
        let a = (cx - cw / 2.0, cy - ch / 2.0);
        let b = (cx + cw / 2.0, cy + ch / 2.0);
        card_picture(&mut c, card.id, state.style, a, b, s);
        if card.angle != 0 {
            // the turn shown as a hand on the card
            let t = card.angle as f32 / 4096.0 * std::f32::consts::TAU;
            for i in 0..(ch * 0.4) as i32 {
                let r = i as f32;
                c.fill_rect(cx + t.sin() * r - 1.0, cy - t.cos() * r - 1.0, cx + t.sin() * r + 1.0, cy - t.cos() * r + 1.0, look::ACCENT);
            }
        }
        if k == state.picked {
            c.frame((a.0 - px(s, 2.0), a.1 - px(s, 2.0)), (b.0 + px(s, 2.0), b.1 + px(s, 2.0)), px(s, 3.0), 0, look::ACCENT, 2.0 * s);
        }
    }
    drop(mat);
    if state.panel == PanelSize::Large {
        let fy = ph - pad - font - px(s, 4.0);
        let mut kx = pad;
        for (key, what) in [("LB RB", "pick"), ("R stick", "move"), ("Start", "size")] {
            kx += key_hint(&mut c, kx, fy, key, what, s);
        }
    }
    let x = px(s, 12.0);
    let y = h as f32 - ph - px(s, 12.0);
    (c, x, y)
}
