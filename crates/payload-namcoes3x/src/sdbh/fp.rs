//! The play mat: the cabinet's flat panel card scanner ("FP", a camera under the glass that
//! reads each card's Melon code, position and angle), answered from a virtual mat.
//!
//! The game reads it through neodcd's `NeoDcdFP*` functions: its imports of them are the
//! payload's (neodcd's own scanner module never starts, it would look for the reader on a
//! serial port and show error 21-1). Each card is a `NeoDcdFPCardData` (100 bytes): status
//! (1 front), type (16 Melon code), the code's text, its number (the card's Melon ID), X, Y,
//! the angle (4096 per turn), a counter.
//!
//! The game's mat (its battle code, `BFPUpdate`): X from 90 to 550 (left to right), Y from 8
//! to 471, small Y the far side (the attack area, near the screen), large Y the support area;
//! a card is known by its number.

use std::sync::Mutex;

use wal_payload_common::{iat, log};

/// `NeoDcdFPCardData`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CardData {
    status: i32,
    kind: i32,
    id_length: i32,
    id: [u8; 60],
    id_number: u32,
    x: i32,
    y: i32,
    angle: i32,
    counter: i32,
    status_detail: i32,
    type_detail: i32,
}

const _: () = assert!(std::mem::size_of::<CardData>() == 100);

const STATUS_FRONT: i32 = 1;
const TYPE_MELON: i32 = 16;
const INIT_COMPLETED: i32 = 2;
const STATUS_OK: i32 = 1;

/// The game's mat, in scanner units.
pub const MAT_LEFT: i32 = 90;
pub const MAT_RIGHT: i32 = 550;
pub const MAT_TOP: i32 = 8;
pub const MAT_BOTTOM: i32 = 471;
/// A card's size on the mat (a real card, 59 x 86 mm, on the cabinet's glass).
pub const CARD_W: i32 = 82;
pub const CARD_H: i32 = 120;
/// The deck holds 5 cards.
pub const DECK_MAX: usize = 5;

/// A card on the mat.
#[derive(Clone, Copy, Debug)]
pub struct MatCard {
    /// The card's Melon ID.
    pub id: u16,
    /// Its centre, in scanner units.
    pub x: i32,
    pub y: i32,
    /// Its angle, 4096 per turn (0 upright).
    pub angle: i32,
}

pub struct Mat {
    pub cards: Vec<MatCard>,
    /// The deck lies on the mat (F4 lifts it off).
    pub placed: bool,
    /// What the game read last (the scanner's answers stay valid until the next count).
    data: Vec<CardData>,
    counter: i32,
}

pub static MAT: Mutex<Mat> = Mutex::new(Mat { cards: Vec::new(), placed: true, data: Vec::new(), counter: 0 });

/// The support area's row and the attack area's (card centres).
pub const SUPPORT_Y: i32 = MAT_BOTTOM - CARD_H / 2 - 20;
pub const ATTACK_Y: i32 = MAT_TOP + CARD_H / 2 + 20;

/// The deck laid on the mat in the support area, one column each.
pub fn lay(ids: &[u16]) {
    let mut mat = MAT.lock().unwrap();
    let n = ids.len().min(DECK_MAX);
    mat.cards = (0..n)
        .map(|i| MatCard { id: ids[i], x: column_x(i, n), y: SUPPORT_Y, angle: 0 })
        .collect();
}

/// The centre of column `i` of `n` (the mat's width shared).
pub fn column_x(i: usize, n: usize) -> i32 {
    let width = MAT_RIGHT - MAT_LEFT;
    let n = n.max(1) as i32;
    MAT_LEFT + width * (2 * i as i32 + 1) / (2 * n)
}

pub fn init(exe: usize) {
    watch_reading(exe);
    let hooks: [(&str, usize); 11] = [
        ("NeoDcdFPInit", fp_init as *const () as usize),
        ("NeoDcdFPGetInitStatus", fp_get_init_status as *const () as usize),
        ("NeoDcdFPGetStatus", fp_get_status as *const () as usize),
        ("NeoDcdFPConfigPort", fp_config as *const () as usize),
        ("NeoDcdFPConfigReaderType", fp_config as *const () as usize),
        ("NeoDcdFPSetScanCardType", fp_config as *const () as usize),
        ("NeoDcdFPSetShutterSpeed", fp_config as *const () as usize),
        ("NeoDcdFPSetMaxScanCards", fp_config as *const () as usize),
        ("NeoDcdFPGetMaxScanCards", fp_get_max as *const () as usize),
        ("NeoDcdFPGetCardCount", fp_get_card_count as *const () as usize),
        ("NeoDcdFPGetCard", fp_get_card as *const () as usize),
    ];
    for (name, f) in hooks {
        if unsafe { iat::hook_module(exe, "neodcd.dll", name, f) }.is_none() {
            log!("sdbh: the game does not import {name}");
        }
    }
}

unsafe extern "system" fn fp_init() -> i32 {
    log!("sdbh: card scanner: the play mat");
    1
}

unsafe extern "system" fn fp_get_init_status() -> i32 {
    INIT_COMPLETED
}

unsafe extern "system" fn fp_get_status() -> i32 {
    STATUS_OK
}

/// Port, reader type, card type, shutter speed, card count: the virtual mat needs none.
unsafe extern "system" fn fp_config(_value: usize) {}

unsafe extern "system" fn fp_get_max() -> i32 {
    10
}

/// The cards on the mat now (their answers kept for the game's `GetCard` calls).
/// Counted by the game's own card reading code each frame it runs (the detours of
/// [`watch_reading`]): the menus' reader (`MenuCardReader::Update`, card registration) and the
/// battle's mat (`BFPUpdate`). The game polls the scanner on every screen, even its attract.
static READING_TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static READING_SEEN: Mutex<(u32, Option<std::time::Instant>)> = Mutex::new((0, None));

/// The game reads the mat now (card registration, a battle).
pub fn reading() -> bool {
    let ticks = READING_TICKS.load(std::sync::atomic::Ordering::Relaxed);
    let mut seen = READING_SEEN.lock().unwrap();
    if ticks != seen.0 {
        *seen = (ticks, Some(std::time::Instant::now()));
    }
    seen.1.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
}

/// The game's card reading functions of this build (preferred base 0x400000), each starting
/// with `push ebp; mov ebp, esp; sub esp, imm32`.
const READERS: [(usize, [u8; 9], &str); 2] = [
    (0x7cb650, [0x55, 0x8b, 0xec, 0x81, 0xec, 0x14, 0x06, 0x00, 0x00], "MenuCardReader::Update"),
    (0x56f360, [0x55, 0x8b, 0xec, 0x81, 0xec, 0x28, 0x06, 0x00, 0x00], "BFPUpdate"),
];

/// Each reader counts [`READING_TICKS`] (`inc dword [ticks]`, then the function).
pub fn watch_reading(exe: usize) {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
    for (va, prologue, name) in READERS {
        let at = exe + va - 0x400000;
        if unsafe { std::slice::from_raw_parts(at as *const u8, prologue.len()) } != prologue {
            log!("sdbh: {name} differs, the play mat panel does not follow it");
            continue;
        }
        unsafe {
            // the stub: inc dword [ticks]; the prologue; jmp back after it
            let stub = VirtualAlloc(std::ptr::null(), 32, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
            if stub.is_null() {
                continue;
            }
            let ticks = &READING_TICKS as *const _ as usize as u32;
            let mut code = vec![0xFF, 0x05];
            code.extend_from_slice(&ticks.to_le_bytes());
            code.extend_from_slice(&prologue);
            let back = stub as usize + code.len();
            code.push(0xE9);
            code.extend_from_slice(&((at + prologue.len()).wrapping_sub(back + 5) as i32).to_le_bytes());
            std::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());
            let mut jump = [0x90u8; 9];
            jump[0] = 0xE9;
            jump[1..5].copy_from_slice(&((stub as usize).wrapping_sub(at + 5) as i32).to_le_bytes());
            let mut old = 0;
            VirtualProtect(at as *const _, 9, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, 9);
            VirtualProtect(at as *const _, 9, old, &mut old);
        }
    }
}

unsafe extern "system" fn fp_get_card_count() -> i32 {
    let mut mat = MAT.lock().unwrap();
    mat.counter = mat.counter.wrapping_add(1);
    let counter = mat.counter;
    let cards = if mat.placed { mat.cards.clone() } else { Vec::new() };
    mat.data = cards
        .iter()
        .map(|c| {
            let text = format!("{:04X}", c.id);
            let mut id = [0u8; 60];
            id[..text.len()].copy_from_slice(text.as_bytes());
            CardData {
                status: STATUS_FRONT,
                kind: TYPE_MELON,
                id_length: text.len() as i32,
                id,
                id_number: c.id as u32,
                x: c.x,
                y: c.y,
                angle: c.angle.rem_euclid(4096),
                counter,
                status_detail: 0,
                type_detail: 0,
            }
        })
        .collect();
    mat.data.len() as i32
}

unsafe extern "system" fn fp_get_card(index: i32) -> *const CardData {
    let mat = MAT.lock().unwrap();
    match mat.data.get(index as usize) {
        // the vector is only replaced at the next count, as the reader's buffer
        Some(c) => c as *const CardData,
        None => std::ptr::null(),
    }
}
