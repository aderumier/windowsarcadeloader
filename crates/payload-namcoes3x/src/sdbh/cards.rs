//! The printed cards and their pictures, from the card pack installed in the dump
//! (`app\Cards`): `names.tsv` (each card's Melon ID in hex, its name, `JP` when it was only
//! printed in Japan), `circle\XXXX.jpg` (the game's own virtual card frame) and `rect\XXXX.jpg`
//! (a rectangular frame), 488 x 712 pixels.
//!
//! The pictures are decoded on a thread of their own, reduced to the size asked for, and kept
//! (the binder's page, the mat's cards, the card looked at).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use wal_payload_common::log;

use super::ui::{Picture, load_jpeg};

/// A printed card.
#[derive(Clone, Debug)]
pub struct Card {
    pub id: u16,
    pub name: String,
    /// Only printed in Japan.
    pub japan: bool,
}

/// The cards the scanner cannot read in this build (their scan codes are missing from its
/// card database).
const UNSCANNABLE: [u16; 3] = [0x00DC, 0x54BA, 0x54DB];

/// A card picture's frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Style {
    Circle,
    Rect,
}

impl Style {
    pub fn folder(self) -> &'static str {
        match self {
            Style::Circle => "circle",
            Style::Rect => "rect",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Style::Circle => "Circle",
            Style::Rect => "Rectangle",
        }
    }
}

/// A picture's state.
#[derive(Clone)]
pub enum Art {
    Loading,
    Ready(Arc<Picture>),
    Missing,
}

type Key = (u16, Style, u32, u32);

struct Loader {
    cache: HashMap<Key, Art>,
    queue: VecDeque<Key>,
}

static LOADER: Mutex<Option<Loader>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
/// Called when a picture is ready (the overlay redraws).
static ON_READY: OnceLock<fn()> = OnceLock::new();

fn cards_dir() -> std::path::PathBuf {
    super::ddtel::game_root().join("app").join("Cards")
}

/// The printed cards, in the pack's order.
pub fn all() -> &'static [Card] {
    static CARDS: OnceLock<Vec<Card>> = OnceLock::new();
    CARDS.get_or_init(|| {
        let path = cards_dir().join("names.tsv");
        let Ok(text) = std::fs::read_to_string(&path) else {
            log!("sdbh: no card pack ({} not found): the binder has no cards", path.display());
            return Vec::new();
        };
        let mut cards: Vec<Card> = text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| {
                let mut f = l.split('\t');
                let id = u16::from_str_radix(f.next()?.trim(), 16).ok()?;
                let name = f.next().unwrap_or("").trim().to_string();
                let japan = f.next().is_some_and(|m| m.trim() == "JP");
                Some(Card { id, name, japan })
            })
            .filter(|c| !UNSCANNABLE.contains(&c.id))
            .collect();
        cards.dedup_by_key(|c| c.id);
        log!("sdbh: {} printed cards in the card pack ({} printed only in Japan)", cards.len(), cards.iter().filter(|c| c.japan).count());
        cards
    })
}

pub fn find(id: u16) -> Option<&'static Card> {
    all().iter().find(|c| c.id == id)
}

/// The character a card shows (its name before the form and the outfit).
pub fn character(name: &str) -> &str {
    let end = name.find(['[', '<', ':']).unwrap_or(name.len());
    name[..end].trim()
}

/// The card's picture at that size: ready, or asked for (`Loading`), or not in the pack.
pub fn picture(id: u16, style: Style, w: u32, h: u32) -> Art {
    let key = (id, style, w.max(1), h.max(1));
    let mut guard = LOADER.lock().unwrap();
    let loader = guard.get_or_insert_with(|| {
        std::thread::spawn(load_loop);
        Loader { cache: HashMap::new(), queue: VecDeque::new() }
    });
    if let Some(art) = loader.cache.get(&key) {
        return art.clone();
    }
    loader.cache.insert(key, Art::Loading);
    // the latest asked first: the page shown now
    loader.queue.push_front(key);
    // a bounded cache: the oldest pictures of other sizes dropped
    if loader.cache.len() > 600 {
        let stale: Vec<Key> = loader.cache.keys().filter(|k| k.2 != key.2).take(200).copied().collect();
        for k in stale {
            loader.cache.remove(&k);
        }
    }
    WAKE.notify_one();
    Art::Loading
}

pub fn on_ready(f: fn()) {
    let _ = ON_READY.set(f);
}

fn load_loop() {
    let dir = cards_dir();
    loop {
        let key = {
            let mut guard = LOADER.lock().unwrap();
            loop {
                if let Some(k) = guard.as_mut().and_then(|l| l.queue.pop_front()) {
                    break k;
                }
                guard = WAKE.wait(guard).unwrap();
            }
        };
        let (id, style, w, h) = key;
        let path = dir.join(style.folder()).join(format!("{id:04X}.jpg"));
        let art = match std::fs::read(&path).ok().and_then(|b| load_jpeg(&b, w, h)) {
            Some(p) => Art::Ready(Arc::new(p)),
            None => Art::Missing,
        };
        if let Some(l) = LOADER.lock().unwrap().as_mut() {
            l.cache.insert(key, art);
        }
        if let Some(f) = ON_READY.get() {
            f();
        }
    }
}
