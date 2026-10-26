//! Zombie Crisis (SpiderZSoft, Unreal Engine 4.25, 64-bit): its I/O DLL `GamePort.dll`
//! (`WAL_ARCADEPC_BOARD=zombiecrisis`), the payload installed under that name.
//!
//! The game (`ADllActorGameport`) loads `GamePort.dll`, calls its `GetInstance` and uses the
//! returned C++ object through its virtual functions only (113 slots, `this` first; the
//! original, MFC and OpenCV, reads a serial board and the guns' camera tracker): coins buffered
//! per player (got, subtracted when credited, cleared), the start keys, the guns' first button
//! (`Frie1`: the reload, a shot off the screen) and second (`Frie2`: the trigger; player 4 has
//! only `Frie1`, its trigger), the aim in the screen's pixels (1920x1080; the game maps it
//! through its own calibration, the service menu's), the operator's setting, left and right
//! keys, `init` 1; the rest (lamps, ticket printers, motors, the camera) 0.
//!
//! Presses are latched until the game reads them: a short tap between two of its polls is not
//! lost. The slots' map comes from the original DLL's vtable.
//!
//! Natives: `trigger reload start coin offscreen` (each player), `setting left right` (the
//! operator's).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};

use crate::demul;

const PLAYERS: usize = 4;
const SLOTS: usize = 113;
const AIM_W: f32 = 1920.0;
const AIM_H: f32 = 1080.0;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Start,
    Coin,
    Offscreen,
    Setting,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("offscreen", Native::Offscreen),
    ("setting", Native::Setting),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b3", "reload"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "setting"),
    ("down", "setting"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A button: held, or pressed since the game's last read.
struct Button {
    held: AtomicBool,
    latch: AtomicBool,
}

impl Button {
    const fn new() -> Button {
        Button { held: AtomicBool::new(false), latch: AtomicBool::new(false) }
    }

    fn set(&self, on: bool) {
        self.held.store(on, Ordering::Relaxed);
        if on {
            self.latch.store(true, Ordering::Relaxed);
        }
    }

    /// Held or pressed since the last read (the latch cleared).
    fn read(&self) -> i64 {
        (self.latch.swap(false, Ordering::Relaxed) | self.held.load(Ordering::Relaxed)) as i64
    }
}

static TRIGGER: [Button; PLAYERS] = [const { Button::new() }; PLAYERS];
static RELOAD: [Button; PLAYERS] = [const { Button::new() }; PLAYERS];
static START: [Button; PLAYERS] = [const { Button::new() }; PLAYERS];
static COINS: [AtomicI32; PLAYERS] = [const { AtomicI32::new(0) }; PLAYERS];
static OPERATOR: [AtomicBool; 3] = [const { AtomicBool::new(false) }; 3];

/// The players' sticks into the buttons, every 4 ms.
fn input_thread() {
    let mut coin_was = [false; PLAYERS];
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let mut operator = [false; 3];
        for player in 0..PLAYERS {
            let (mut trigger, mut reload, mut start, mut coin, mut offscreen) = (false, false, false, false, false);
            map().for_each_pressed(&input(player), |n| match n {
                Native::Trigger => trigger = true,
                Native::Reload => reload = true,
                Native::Start => start = true,
                Native::Coin => coin = true,
                Native::Offscreen => offscreen = true,
                Native::Setting if player == 0 => operator[0] = true,
                Native::Left if player == 0 => operator[1] = true,
                Native::Right if player == 0 => operator[2] = true,
                _ => {}
            });
            // a shot off the screen: the reload
            let off = offscreen || demul::gun_of_player_in_picture(player).is_none();
            TRIGGER[player].set(trigger && !off);
            RELOAD[player].set(reload || (trigger && off));
            START[player].set(start);
            if coin && !coin_was[player] {
                COINS[player].fetch_add(1, Ordering::Relaxed);
            }
            coin_was[player] = coin;
        }
        for (o, on) in OPERATOR.iter().zip(operator) {
            o.store(on, Ordering::Relaxed);
        }
    }
}

fn aim(player: usize, y: bool) -> i64 {
    let (px, py) = demul::gun_of_player_in_picture(player).unwrap_or((0.0, 0.0));
    if y { (py * AIM_H) as i64 } else { (px * AIM_W) as i64 }
}

type This = *mut c_void;

extern "C" fn zero(_: This) -> i64 {
    0
}

extern "C" fn init(_: This) -> i64 {
    log!("zombiecrisis: init");
    1
}

macro_rules! player_slots {
    ($($p:literal),*) => {
        /// The functions of player `p` (0-based): coins, sub, clear, start, reload, trigger, x, y.
        fn player_slots(p: usize) -> [usize; 8] {
            match p {
                $(
                    $p => {
                        extern "C" fn coins(_: This) -> i64 { COINS[$p].load(Ordering::Relaxed) as i64 }
                        extern "C" fn sub(_: This, count: i32) -> i64 {
                            let _ = COINS[$p].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c - count).max(0)));
                            0
                        }
                        extern "C" fn clear(_: This) -> i64 { COINS[$p].store(0, Ordering::Relaxed); 0 }
                        extern "C" fn start(_: This) -> i64 { START[$p].read() }
                        extern "C" fn reload(_: This) -> i64 { RELOAD[$p].read() }
                        extern "C" fn trigger(_: This) -> i64 { TRIGGER[$p].read() }
                        extern "C" fn x(_: This) -> i64 { aim($p, false) }
                        extern "C" fn y(_: This) -> i64 { aim($p, true) }
                        [coins as *const () as usize, sub as *const () as usize, clear as *const () as usize, start as *const () as usize, reload as *const () as usize, trigger as *const () as usize, x as *const () as usize, y as *const () as usize]
                    }
                )*
                _ => unreachable!(),
            }
        }
    };
}

player_slots!(0, 1, 2, 3);

extern "C" fn setting(_: This) -> i64 {
    OPERATOR[0].load(Ordering::Relaxed) as i64
}

extern "C" fn left(_: This) -> i64 {
    OPERATOR[1].load(Ordering::Relaxed) as i64
}

extern "C" fn right(_: This) -> i64 {
    OPERATOR[2].load(Ordering::Relaxed) as i64
}

/// The object `GetInstance` returns: its vtable pointer.
#[repr(C)]
struct Instance {
    vtable: *const usize,
}

unsafe impl Sync for Instance {}
unsafe impl Send for Instance {}

fn vtable() -> &'static [usize; SLOTS] {
    static VTABLE: OnceLock<[usize; SLOTS]> = OnceLock::new();
    VTABLE.get_or_init(|| {
        let mut v = [zero as *const () as usize; SLOTS];
        let p: Vec<[usize; 8]> = (0..PLAYERS).map(player_slots).collect();
        // coins: got, sub (1, 2 / 8, 9 / 11, 12 / 14, 15), clear (17, 19, 21, 23)
        for (player, (got, sub)) in [(1, 2), (8, 9), (11, 12), (14, 15)].into_iter().enumerate() {
            v[got] = p[player][0];
            v[sub] = p[player][1];
            v[17 + 2 * player] = p[player][2];
        }
        for player in 0..PLAYERS {
            // start keys 67..70
            v[67 + player] = p[player][3];
        }
        // Frie1 (the reload) 71..73, Frie2 (the trigger) 74..76; player 4: Frie1 77, its trigger
        for player in 0..3 {
            v[71 + player] = p[player][4];
            v[74 + player] = p[player][5];
        }
        v[77] = p[3][5];
        // aim: P1 x 88 y 91, P2 x 89 y 92, P3 x 94 y 95, P4 x 96 y 97
        for (player, (x, y)) in [(88, 91), (89, 92), (94, 95), (96, 97)].into_iter().enumerate() {
            v[x] = p[player][6];
            v[y] = p[player][7];
        }
        v[98] = setting as *const () as usize;
        v[100] = left as *const () as usize;
        v[101] = right as *const () as usize;
        v[105] = init as *const () as usize;
        v
    })
}

static INSTANCE: OnceLock<Instance> = OnceLock::new();

/// `GamePort.dll`'s only export: the board's object.
#[unsafe(no_mangle)]
pub extern "C" fn GetInstance() -> *const c_void {
    let instance = INSTANCE.get_or_init(|| {
        std::thread::spawn(input_thread);
        log!("zombiecrisis: GamePort instance");
        Instance { vtable: vtable().as_ptr() }
    });
    instance as *const Instance as *const c_void
}
