//! Injustice Arcade's character card readers: a virtual card per player.
//!
//! On the cabinet each player shows a character card to a webcam: the frames (OpenCV
//! `VideoCapture`, one per player) are decoded by ZBar and the Code 128 barcode data names the
//! card. Here the cameras are emulated: the per-player device lookup succeeds, the capture
//! calls return blank frames, and while a player's card is "on the reader" its barcode is drawn
//! into the gray frame handed to the real `zbar_scan_image`, so the game's own decoding runs.
//!
//! The card is chosen on an overlay drawn over the game (`SwapBuffers`): up/down browse the 130
//! cards (left/right by 10), the `card` toggle puts the selected card on the reader and takes it
//! off. The selections are kept in `WindowsLoader\cards.txt`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::{iat, log};
use wal_protocol::button;

use crate::common::{Site, matches, write};

/// Card data: Code 128 (code set B) of 8 lowercase letters and digits.
const MODULE: usize = 3;

/// Camera slot of player `i`: `cv::VideoCapture` at 0x3150b68 + i * 0x80, the ZBar scanner at
/// +0x14, the gray frame at +0x18, its width and height at +0x1c / +0x20.
const CAMERAS: usize = 0x3150b68;
const CAMERA_SIZE: usize = 0x80;
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 480;

/// Player camera -> device index lookup (the DirectShow device names in the operator
/// settings): `mov eax, [esp+4]; ret`, camera i is device i.
const DEVICE_LOOKUP: Site = (0x1a136d0, &[0x53, 0x8b, 0x5c, 0x24, 0x08, 0x83, 0xfb, 0x02]);

/// The character cards: (name, barcode data).
pub(crate) const CARDS: [(&str, &str); 130] = [
    ("Catwoman (Regime)", "hcojx0fq"),
    ("Cyborg (Prime)", "ww5eo0d1"),
    ("Deathstroke (Insurgency)", "ro07wcqp"),
    ("Green Arrow (Insurgency)", "8qi91iwm"),
    ("Green Lantern (Prime)", "824gym83"),
    ("Green Lantern (New 52)", "xz9gdyzs"),
    ("Harley Quinn (Prime)", "1041tsu0"),
    ("Lex Luthor (Prime)", "csvmxi45"),
    ("Nightwing (Prime)", "l2lyk999"),
    ("Sinestro (Prime)", "84g5oeds"),
    ("Solomon Grundy (Regime)", "tg3t9z3m"),
    ("The Flash (Prime)", "27c3m0ql"),
    ("The Flash (New 52)", "d5w8uoqn"),
    ("Bane (Knightfall)", "3dls5ap2"),
    ("Bane (Regime)", "srlnu660"),
    ("Black Adam (Prime)", "of9tz8ne"),
    ("Catwoman (Prime)", "5t5cnztf"),
    ("Cyborg (Regime)", "bzqqr5vk"),
    ("Deathstroke (Prime)", "siia9m6d"),
    ("Doomsday (Regime)", "fxgxoj6p"),
    ("Green Arrow (Prime)", "lcmetqua"),
    ("Green Lantern (Regime)", "anzl9n1s"),
    ("Harley Quinn (Insurgency)", "m0nbfmcd"),
    ("Lex Luthor (Insurgency)", "c3fdkyqu"),
    ("Nightwing (Regime)", "vzx8bsfe"),
    ("Sinestro (Regime)", "suhyp2fk"),
    ("Solomon Grundy (Prime)", "heejmu2s"),
    ("The Flash (Regime)", "r9riknaj"),
    ("The Joker (Prime)", "heg3cnb6"),
    ("Wonder Woman (Prime)", "uklpyrrg"),
    ("Bane (Prime)", "mu24s1qq"),
    ("Batman (Prime)", "ha7qfehz"),
    ("Batman (Arkham Origins)", "3k16go3i"),
    ("Batman (Insurgency)", "yw1i7qsp"),
    ("Black Adam (Regime)", "8q4mfnl9"),
    ("Catwoman (Batman Returns)", "q71z6yjw"),
    ("Cyborg (Teen Titans)", "jpdiqg81"),
    ("Doomsday (Prime)", "19c4qz24"),
    ("Green Arrow (Arrow)", "4yfbacb6"),
    ("Harley Quinn (Animated)", "wdawoqn1"),
    ("Lex Luthor (Krypto)", "f0wd9veo"),
    ("Lobo (Prime)", "ym8p1yra"),
    ("Nightwing (New 52)", "012pi0g7"),
    ("Shazam (Prime)", "zpu9oedg"),
    ("Sinestro (Green Lantern)", "b8k0znxx"),
    ("Superman (Prime)", "qf4ulw4j"),
    ("Superman (Regime)", "0gyiezh6"),
    ("The Joker (Insurgency)", "tqw6h68x"),
    ("Wonder Woman (Regime)", "4eid5oqs"),
    ("Aquaman (Prime)", "q88ju3tq"),
    ("Aquaman (Regime)", "7w23xwdh"),
    ("Ares (Prime)", "7fcteiej"),
    ("Bane (Arkham Origins)", "d4mxvmsp"),
    ("Bane (Luchador)", "mf5h707f"),
    ("Batgirl (Prime)", "gvdsdb2n"),
    ("Batgirl (Cassandra Cain)", "tlllb532"),
    ("Batman (Arkham Knight)", "ufp6proz"),
    ("Batman (Beyond)", "m1ulss6a"),
    ("Batman Beyond (Animated)", "557orib2"),
    ("Batman (Blackest Night)", "wwa8fxtu"),
    ("Batman (Red Son)", "rwl3sibn"),
    ("Darkseid (Prime)", "flbh553i"),
    ("Deathstroke (Arkham Origins)", "vzu350il"),
    ("Deathstroke (Red Son)", "ac9a9xwt"),
    ("Doomsday (Containment)", "f3k1607o"),
    ("General Zod (Man of Steel)", "8pkvu7w0"),
    ("Green Lantern (John Stewart)", "4f0zxtar"),
    ("Green Lantern (Red Son)", "t6hh54un"),
    ("Hal Jordan (Red Lantern)", "wdtl54k8"),
    ("Harley Quinn (Arkham)", "0v2z1foz"),
    ("Harley Quinn (Arkham Knight)", "6xpstvkq"),
    ("Hawkgirl (Prime)", "8z2q93sh"),
    ("Hawkgirl (Regime)", "tmip4pct"),
    ("Killer Frost (Prime)", "b1273f6s"),
    ("Killer Frost (Regime)", "jztck6y4"),
    ("Lobo (Bounty Hunter)", "1mytm2ao"),
    ("Martian Manhunter (Prime)", "hcttm2wg"),
    ("Martian Manhunter (Blackest Night)", "6ea2ade7"),
    ("Raven (Prime)", "1mdkn86e"),
    ("Raven (Regime)", "uivfp7iu"),
    ("Reverse Flash", "kq59oc8q"),
    ("Scorpion (Mortal Kombat)", "60hdnfqw"),
    ("Scorpion (Mortal Kombat X)", "yx5ihl92"),
    ("Solomon Grundy (Boss)", "3gt4yriu"),
    ("Solomon Grundy (Red Son)", "rq9xxxfk"),
    ("Static (Prime)", "qj1393z0"),
    ("Superman (Godfall)", "ogt6qb9p"),
    ("Superman (Man of Steel)", "yglx046h"),
    ("Superman (Prison)", "damihayn"),
    ("Superman (Red Son)", "gkpawilq"),
    ("The Arkham Knight (Arkham Knight)", "zbng65c5"),
    ("The Flash (Elseworld)", "4zzbbc0o"),
    ("The Flash (Metahuman)", "zq5tqafx"),
    ("The Joker (Arkham Origins)", "7zpsj4yt"),
    ("The Joker (The Killing Joke)", "dji5v42o"),
    ("Wonder Woman (600)", "xsjmu1ij"),
    ("Wonder Woman (Justice League)", "hvcufg8j"),
    ("Wonder Woman (Red Son)", "967e0946"),
    ("Zatanna (Prime)", "py851k8m"),
    ("Zod (Prime)", "vybd46nj"),
    ("Team: Gotham Knights", "0lvs1jmi"),
    ("Team: Speed Force", "uuzptbpo"),
    ("Team: League of Green", "bqd6tddf"),
    ("Team: League of Assassins", "9wv5tza3"),
    ("Team: Femmes Fatales", "ph40oyf8"),
    ("Team: Justice League", "41b6ud1a"),
    ("Team: Teen Titans", "arxjqsw9"),
    ("Team: Suicide Squad", "vezms08a"),
    ("Team: The Ultimate", "6a9uqxqz"),
    ("Team: Blackest Night", "6r02i8fq"),
    ("Darkseid (Boss)", "37dq82pi"),
    ("Doomsday (Boss)", "04pvb1tv"),
    ("Ares (Boss)", "3zroecd0"),
    ("Luthor (Boss)", "gi6ebz5x"),
    ("Lobo (Boss)", "ueobmbhi"),
    ("Scorpion (Boss)", "nlw3fjz5"),
    ("Batman (Boss)", "00k2fm4e"),
    ("Superman (Boss)", "pl2769tg"),
    ("Harley (Boss)", "kbdf8thn"),
    ("Catwoman (Boss)", "si4g03lr"),
    ("Zatanna (Prime)", "1rqkahsm"),
    ("The Arkham Knight (Arkham Knight)", "ph6dzxje"),
    ("Shazam (Prime)", "exkzsrqy"),
    ("Nightwing (New 52)", "p9zkne4t"),
    ("Static (Prime)", "x60lgt60"),
    ("Raven (Prime)", "tnvroze1"),
    ("Superman (Man of Steel)", "1pyxstz5"),
    ("Batman (Arkham Origins)", "1dsyzs90"),
    ("Harley Quinn (Arkham)", "9vvfn4vg"),
    ("The Joker (Arkham Origins)", "qegfn17u"),
];

/// Code 128 bar/space widths of the values 0..=106 (106: stop).
const CODE128: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212",
    "221213", "221312", "231212", "112232", "122132", "122231", "113222", "123122", "123221",
    "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221",
    "312212", "322112", "322211", "212123", "212321", "232121", "111323", "131123", "131321",
    "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131",
    "311123", "311321", "331121", "312113", "312311", "332111", "314111", "221411", "431111",
    "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114",
    "122411", "142112", "142211", "241211", "221114", "413111", "241112", "134111", "111242",
    "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311",
    "113141", "114131", "311141", "411131", "211412", "211214", "211232", "2331112",
];

// ── players ─────────────────────────────────────────────────────────────────────────────────

struct Player {
    selected: usize,
    on_reader: bool,
    /// The panel stays shown until then (browsing), and while the card is on the reader.
    shown_until: Option<Instant>,
}

static PLAYERS: Mutex<[Player; 2]> = Mutex::new([
    Player { selected: 0, on_reader: false, shown_until: None },
    Player { selected: 0, on_reader: false, shown_until: None },
]);

const SHOW: Duration = Duration::from_secs(4);
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(80);

fn selections_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|d| d.join("WindowsLoader").join("cards.txt")).unwrap_or_default()
}

fn load_selections() {
    let text = std::fs::read_to_string(selections_path()).unwrap_or_default();
    let mut players = PLAYERS.lock().unwrap();
    for (line, p) in text.lines().zip(players.iter_mut()) {
        if let Some(i) = CARDS.iter().position(|(_, code)| *code == line.trim()) {
            p.selected = i;
        }
    }
}

fn save_selections(players: &[Player; 2]) {
    let text: String = players.iter().map(|p| format!("{}\n", CARDS[p.selected].1)).collect();
    let _ = std::fs::write(selections_path(), text);
}

/// The players' sticks: browsing, and the `card` toggle.
fn run_inputs() {
    let mut held = [[None::<Instant>; 4]; 2];
    loop {
        let now = Instant::now();
        let mut players = PLAYERS.lock().unwrap();
        let mut changed = false;
        for (i, p) in players.iter_mut().enumerate() {
            let stick = wal_payload_common::input(i);
            let on = stick.pressed(button::CARD);
            if on != p.on_reader {
                p.on_reader = on;
                p.shown_until = Some(now + SHOW);
                log!("injustice: P{} card {} ({})", i + 1, if on { "on the reader" } else { "removed" }, CARDS[p.selected].0);
            }
            for (k, (b, step)) in [(button::UP, -1isize), (button::DOWN, 1), (button::LEFT, -10), (button::RIGHT, 10)].into_iter().enumerate() {
                let fire = match (stick.pressed(b), held[i][k]) {
                    (false, _) => {
                        held[i][k] = None;
                        false
                    }
                    (true, None) => {
                        held[i][k] = Some(now + REPEAT_DELAY);
                        true
                    }
                    (true, Some(next)) if now >= next => {
                        held[i][k] = Some(now + REPEAT);
                        true
                    }
                    _ => false,
                };
                if fire {
                    p.selected = (p.selected as isize + step).rem_euclid(CARDS.len() as isize) as usize;
                    p.shown_until = Some(now + SHOW);
                    changed = true;
                }
            }
        }
        if changed {
            save_selections(&players);
        }
        drop(players);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The barcode of the card on player `i`'s reader.
fn card_on_reader(i: usize) -> Option<&'static str> {
    let players = PLAYERS.lock().unwrap();
    players.get(i).filter(|p| p.on_reader).map(|p| CARDS[p.selected].1)
}

// ── Code 128 ─────────────────────────────────────────────────────────────────────────────────

/// The bars of `data` in code set B, one bool per module (true: black).
fn code128(data: &str) -> Vec<bool> {
    let values: Vec<usize> = data.bytes().map(|b| (b - 32) as usize).collect();
    let check = (104 + values.iter().enumerate().map(|(i, v)| (i + 1) * v).sum::<usize>()) % 103;
    let mut modules = Vec::new();
    for v in std::iter::once(104).chain(values).chain([check, 106]) {
        for (k, w) in CODE128[v].bytes().enumerate() {
            modules.extend(std::iter::repeat_n(k % 2 == 0, (w - b'0') as usize));
        }
    }
    modules
}

/// Draws the card of `data` in the middle of a gray frame (a white card, the bars across).
fn draw_card(frame: *mut u8, width: usize, height: usize, data: &str) {
    let bars = code128(data);
    let pixels = unsafe { std::slice::from_raw_parts_mut(frame, width * height) };
    pixels.fill(0xff);
    let left = width.saturating_sub(bars.len() * MODULE) / 2;
    for y in height / 4..height * 3 / 4 {
        for (m, black) in bars.iter().enumerate() {
            if *black {
                for x in 0..MODULE {
                    if let Some(p) = pixels.get_mut(y * width + left + m * MODULE + x) {
                        *p = 0;
                    }
                }
            }
        }
    }
}

// ── cameras: OpenCV ──────────────────────────────────────────────────────────────────────────

/// The player of a camera slot (`this` of the game's `cv::VideoCapture` objects).
fn camera_player(this: *mut c_void) -> Option<usize> {
    let a = this as usize;
    (CAMERAS..CAMERAS + 2 * CAMERA_SIZE).contains(&a).then(|| (a - CAMERAS) / CAMERA_SIZE)
}

static CAP_OPEN: AtomicUsize = AtomicUsize::new(0);
static CAP_IS_OPENED: AtomicUsize = AtomicUsize::new(0);
static CAP_GET: AtomicUsize = AtomicUsize::new(0);
static CAP_SET: AtomicUsize = AtomicUsize::new(0);
static CAP_GRAB: AtomicUsize = AtomicUsize::new(0);
static CAP_RETRIEVE: AtomicUsize = AtomicUsize::new(0);
static CAP_RELEASE: AtomicUsize = AtomicUsize::new(0);
static MAT_CREATE: AtomicUsize = AtomicUsize::new(0);
static OPENED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

const CV_CAP_PROP_FRAME_WIDTH: i32 = 3;
const CV_CAP_PROP_FRAME_HEIGHT: i32 = 4;
const CV_8UC3: i32 = 16;

unsafe extern "thiscall" fn cap_open(this: *mut c_void, device: i32) -> bool {
    if let Some(i) = camera_player(this) {
        OPENED[i].store(true, Ordering::Relaxed);
        log!("injustice: P{} camera opened (device {device})", i + 1);
        return true;
    }
    let f: unsafe extern "thiscall" fn(*mut c_void, i32) -> bool = unsafe { std::mem::transmute(CAP_OPEN.load(Ordering::Relaxed)) };
    unsafe { f(this, device) }
}

unsafe extern "thiscall" fn cap_is_opened(this: *mut c_void) -> bool {
    if let Some(i) = camera_player(this) {
        return OPENED[i].load(Ordering::Relaxed);
    }
    let f: unsafe extern "thiscall" fn(*mut c_void) -> bool = unsafe { std::mem::transmute(CAP_IS_OPENED.load(Ordering::Relaxed)) };
    unsafe { f(this) }
}

unsafe extern "thiscall" fn cap_get(this: *mut c_void, property: i32) -> f64 {
    if camera_player(this).is_some() {
        return match property {
            CV_CAP_PROP_FRAME_WIDTH => FRAME_WIDTH as f64,
            CV_CAP_PROP_FRAME_HEIGHT => FRAME_HEIGHT as f64,
            _ => 0.0,
        };
    }
    let f: unsafe extern "thiscall" fn(*mut c_void, i32) -> f64 = unsafe { std::mem::transmute(CAP_GET.load(Ordering::Relaxed)) };
    unsafe { f(this, property) }
}

unsafe extern "thiscall" fn cap_set(this: *mut c_void, property: i32, value: f64) -> bool {
    if camera_player(this).is_some() {
        return true;
    }
    let f: unsafe extern "thiscall" fn(*mut c_void, i32, f64) -> bool = unsafe { std::mem::transmute(CAP_SET.load(Ordering::Relaxed)) };
    unsafe { f(this, property, value) }
}

/// A frame every 33 ms (the game's capture thread loops on it).
unsafe extern "thiscall" fn cap_grab(this: *mut c_void) -> bool {
    if let Some(i) = camera_player(this) {
        std::thread::sleep(Duration::from_millis(33));
        return OPENED[i].load(Ordering::Relaxed);
    }
    let f: unsafe extern "thiscall" fn(*mut c_void) -> bool = unsafe { std::mem::transmute(CAP_GRAB.load(Ordering::Relaxed)) };
    unsafe { f(this) }
}

/// `retrieve(OutputArray, flag)`: a gray 640x480 BGR frame in the `cv::Mat` the
/// `_OutputArray` { flags, obj, size } refers to (`Mat::create`; data at +0x10).
unsafe extern "thiscall" fn cap_retrieve(this: *mut c_void, image: *const usize, flag: i32) -> bool {
    if camera_player(this).is_some() {
        let mat = unsafe { image.add(1).read() } as *mut c_void;
        let create = MAT_CREATE.load(Ordering::Relaxed);
        if mat.is_null() || create == 0 {
            return false;
        }
        let create: unsafe extern "thiscall" fn(*mut c_void, i32, i32, i32) = unsafe { std::mem::transmute(create) };
        unsafe { create(mat, FRAME_HEIGHT as i32, FRAME_WIDTH as i32, CV_8UC3) };
        let data = unsafe { (mat as *const *mut u8).add(4).read() };
        if data.is_null() {
            return false;
        }
        unsafe { std::ptr::write_bytes(data, 0x80, (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize) };
        return true;
    }
    let f: unsafe extern "thiscall" fn(*mut c_void, *const usize, i32) -> bool = unsafe { std::mem::transmute(CAP_RETRIEVE.load(Ordering::Relaxed)) };
    unsafe { f(this, image, flag) }
}

unsafe extern "thiscall" fn cap_release(this: *mut c_void) {
    if let Some(i) = camera_player(this) {
        OPENED[i].store(false, Ordering::Relaxed);
        return;
    }
    let f: unsafe extern "thiscall" fn(*mut c_void) = unsafe { std::mem::transmute(CAP_RELEASE.load(Ordering::Relaxed)) };
    unsafe { f(this) }
}

// ── ZBar ─────────────────────────────────────────────────────────────────────────────────────

static ZBAR_SCAN: AtomicUsize = AtomicUsize::new(0);

/// `zbar_scan_image(scanner, image)` on a player's gray frame: the card on the reader drawn in.
unsafe extern "C" fn zbar_scan_image(scanner: *mut c_void, image: *mut c_void) -> i32 {
    for i in 0..2 {
        let slot = CAMERAS + i * CAMERA_SIZE;
        if unsafe { ((slot + 0x14) as *const usize).read() } != scanner as usize {
            continue;
        }
        if let Some(code) = card_on_reader(i) {
            let frame = unsafe { ((slot + 0x18) as *const *mut u8).read() };
            let width = unsafe { ((slot + 0x1c) as *const u32).read() } as usize;
            let height = unsafe { ((slot + 0x20) as *const u32).read() } as usize;
            if !frame.is_null() && width * height > 0 {
                draw_card(frame, width, height, code);
            }
        }
    }
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 = unsafe { std::mem::transmute(ZBAR_SCAN.load(Ordering::Relaxed)) };
    unsafe { f(scanner, image) }
}

// ── init ─────────────────────────────────────────────────────────────────────────────────────

pub(crate) fn init() {
    if !matches(DEVICE_LOOKUP) {
        log!("injustice: unexpected camera lookup code, card readers left alone");
        return;
    }
    write(DEVICE_LOOKUP.0, &[0x8b, 0x44, 0x24, 0x04, 0xc3]);
    let cv = "opencv_world300.dll";
    let hooks: [(&str, usize, &AtomicUsize); 7] = [
        ("?open@VideoCapture@cv@@UAE_NH@Z", cap_open as *const () as usize, &CAP_OPEN),
        ("?isOpened@VideoCapture@cv@@UBE_NXZ", cap_is_opened as *const () as usize, &CAP_IS_OPENED),
        ("?get@VideoCapture@cv@@UBENH@Z", cap_get as *const () as usize, &CAP_GET),
        ("?set@VideoCapture@cv@@UAE_NHN@Z", cap_set as *const () as usize, &CAP_SET),
        ("?grab@VideoCapture@cv@@UAE_NXZ", cap_grab as *const () as usize, &CAP_GRAB),
        ("?retrieve@VideoCapture@cv@@UAE_NABV_OutputArray@2@H@Z", cap_retrieve as *const () as usize, &CAP_RETRIEVE),
        ("?release@VideoCapture@cv@@UAEXXZ", cap_release as *const () as usize, &CAP_RELEASE),
    ];
    for (name, f, orig) in hooks {
        match unsafe { iat::hook(cv, name, f) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("injustice: {name} not imported"),
        }
    }
    MAT_CREATE.store(export(cv, "?create@Mat@cv@@QAEXHHH@Z"), Ordering::Relaxed);
    if let Some(o) = unsafe { iat::hook("libzbar-0.dll", "zbar_scan_image", zbar_scan_image as *const () as usize) } {
        ZBAR_SCAN.store(o, Ordering::Relaxed);
    }
    load_selections();
    std::thread::spawn(run_inputs);
    crate::injustice_overlay::init();
    log!("injustice: virtual card readers ({} cards)", CARDS.len());
}

/// For the overlay: (selected card, on the reader, panel shown) of each player.
pub(crate) fn panels() -> [(usize, bool, bool); 2] {
    let now = Instant::now();
    let players = PLAYERS.lock().unwrap();
    [0, 1].map(|i| {
        let p = &players[i];
        (p.selected, p.on_reader, p.on_reader || p.shown_until.is_some_and(|t| now < t))
    })
}

/// `dll!name` of a loaded module (0 when missing).
fn export(dll: &str, name: &str) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let dll: Vec<u16> = dll.encode_utf16().chain([0]).collect();
    let name: Vec<u8> = name.bytes().chain([0]).collect();
    unsafe {
        let m = GetModuleHandleW(dll.as_ptr());
        if m.is_null() {
            return 0;
        }
        GetProcAddress(m, name.as_ptr()).map_or(0, |f| f as usize)
    }
}
