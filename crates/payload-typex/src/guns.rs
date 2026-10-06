//! Type X / X2 lightgun games (`WAL_TYPEX_GUNS=<game>`): per-game gun inputs.
//!
//! These games read their guns from a gun board on a serial port (`WAL_TYPEX_GUN_PORT`, comma
//! separated, default `COM1`). Nothing answers on that port: the guns are written straight into the game's memory instead, every 16 ms:
//! `COM1` is a silent port, and a thread writes each player's trigger, offscreen flag and
//! position (0..=16384) at the game's addresses (RVAs of the game executable, from
//! build of each game).
//!
//! Gun of a player = its virtual stick: position `lx`/`ly`, trigger `b1`; offscreen when the
//! position is at an edge of the screen (lightguns report the edge off screen) or `b2` is held
//! (reload with a mouse or a gun's second button).
//!
//! `WAL_TYPEX_GUN_PLAYERS=2,1`: the virtual player driving each gun (default 1,2...), e.g. to
//! calibrate gun 2 with a single mouse.

use std::time::Duration;

use wal_payload_common::{log, serial};
use wal_protocol::{Axis, StickState, button};

/// Addresses of one player's gun.
struct Gun {
    /// Trigger and offscreen bytes written (copies, like the positions).
    trigger: &'static [u32],
    offscreen: &'static [u32],
    /// Position words written (raw and processed copies).
    x: &'static [u32],
    y: &'static [u32],
    /// Byte set to 1 only on the first frame of a trigger press.
    trigger_edge: Option<u32>,
    /// Auto-fire byte, follows the trigger.
    auto_fire: Option<u32>,
}

struct Game {
    name: &'static str,
    /// Bytes written every frame (e.g. "gun board connected").
    constants: &'static [(u32, u8)],
    guns: &'static [Gun],
}

const GAIA_ATTACK_4: Game = Game {
    name: "gaia-attack-4",
    // gun board connected (state 2), 4 players
    constants: &[(0x32F068, 0x02), (0x32F069, 0x00), (0x32F06A, 0x00), (0x32F06B, 0x00), (0xB3B820, 0x04)],
    // As the Haunted Museum games (same engine): the 40-byte gun board record (10 bytes a gun:
    // offscreen +2/+3, x +4, y +6, trigger +8), received at +0x32F030, copied to +0x32EFF8,
    // which the game reads while the port is open (COM1 silent) and the state is 1-4.
    guns: &[
        Gun { trigger: &[0x32F038, 0x32F000], offscreen: &[0x32F032, 0x32F033, 0x32EFFA, 0x32EFFB], x: &[0x32F034, 0x32EFFC], y: &[0x32F036, 0x32EFFE], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x32F042, 0x32F00A], offscreen: &[0x32F03C, 0x32F03D, 0x32F004, 0x32F005], x: &[0x32F03E, 0x32F006], y: &[0x32F040, 0x32F008], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x32F04C, 0x32F014], offscreen: &[0x32F046, 0x32F047, 0x32F00E, 0x32F00F], x: &[0x32F048, 0x32F010], y: &[0x32F04A, 0x32F012], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x32F056, 0x32F01E], offscreen: &[0x32F050, 0x32F051, 0x32F018, 0x32F019], x: &[0x32F052, 0x32F01A], y: &[0x32F054, 0x32F01C], trigger_edge: None, auto_fire: None },
    ],
};

const MUSIC_GUNGUN_2: Game = Game {
    name: "music-gungun-2",
    // gun board connected, JVS type
    constants: &[(0x2B8128, 0x02), (0x2B3708, 0x03)],
    guns: &[
        Gun { trigger: &[0x2B8108], offscreen: &[0x2B8102], x: &[0x2B8104], y: &[0x2B8106], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x2B8112], offscreen: &[0x2B810C], x: &[0x2B810E], y: &[0x2B8110], trigger_edge: None, auto_fire: None },
    ],
};

const HAUNTED_MUSEUM: Game = Game {
    name: "haunted-museum",
    // gun board connected (state 2), gun library initialized
    constants: &[(0x32797C, 0x02), (0x32796C, 0xEE), (0x32796D, 0xEE), (0x32796E, 0xEE), (0x32796F, 0xEE)],
    // The gun board's 20-byte record (10 bytes a player: offscreen +2, x +4, y +6, trigger
    // +8): received at +0x327958, copied to +0x327944 while connected, copied every frame to
    // the game's gun state (+0x98B414..), which derives the edge and auto-fire bytes. Written
    // at the source: the game's state itself is overwritten by that copy every frame.
    guns: &[
        Gun { trigger: &[0x32794C, 0x327960], offscreen: &[0x327946, 0x32795A], x: &[0x327948, 0x32795C], y: &[0x32794A, 0x32795E], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x327956, 0x32796A], offscreen: &[0x327950, 0x327964], x: &[0x327952, 0x327966], y: &[0x327954, 0x327968], trigger_edge: None, auto_fire: None },
    ],
};

const HAUNTED_MUSEUM_2: Game = Game {
    name: "haunted-museum-2",
    // gun board connected (state 2)
    constants: &[(0x3BB448, 0x02), (0x3BB449, 0x00), (0x3BB44A, 0x00), (0x3BB44B, 0x00)],
    // As Haunted Museum: the board record (10 bytes a player: offscreen +2/+3, x +4, y +6,
    // trigger +8), received at +0x3BB410, copied to +0x3BB3D8, which the game reads while the
    // port is open (COM1 silent) and the state is 1-4.
    guns: &[
        Gun { trigger: &[0x3BB418, 0x3BB3E0], offscreen: &[0x3BB412, 0x3BB413, 0x3BB3DA, 0x3BB3DB], x: &[0x3BB414, 0x3BB3DC], y: &[0x3BB416, 0x3BB3DE], trigger_edge: None, auto_fire: None },
        Gun { trigger: &[0x3BB422, 0x3BB3EA], offscreen: &[0x3BB41C, 0x3BB41D, 0x3BB3E4, 0x3BB3E5], x: &[0x3BB41E, 0x3BB3E6], y: &[0x3BB420, 0x3BB3E8], trigger_edge: None, auto_fire: None },
    ],
};

const GAMES: &[Game] = &[GAIA_ATTACK_4, MUSIC_GUNGUN_2, HAUNTED_MUSEUM, HAUNTED_MUSEUM_2];

/// Edge band of the -32768..=32767 position in which the gun counts as off screen
/// (<= 1 or >= 254 on 0..=255).
const EDGE: i32 = 32768 - 256;

fn offscreen(stick: &StickState) -> bool {
    let at_edge = |a: Axis| {
        let v = stick.axis(a) as i32;
        v <= -EDGE || v >= EDGE - 1
    };
    stick.pressed(button::B2) || at_edge(Axis::LeftX) || at_edge(Axis::LeftY)
}

/// -32768..=32767 -> 0..=16384.
fn position(v: i16) -> u16 {
    ((v as i32 + 32768) * 16384 / 65535) as u16
}

pub(crate) fn init() {
    let Ok(name) = std::env::var("WAL_TYPEX_GUNS") else { return };
    let Some(game) = GAMES.iter().find(|g| g.name == name) else {
        log!("guns: unknown game {name:?}");
        return;
    };
    serial::install_sink(&std::env::var("WAL_TYPEX_GUN_PORT").unwrap_or_else(|_| "COM1".into()));
    log!("guns: {} ({} players)", game.name, game.guns.len());
    std::thread::spawn(move || run(game));
}

fn run(game: &'static Game) {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let byte = |rva: u32, v: u8| unsafe { std::ptr::write_volatile((base + rva as usize) as *mut u8, v) };
    let word = |rva: u32, v: u16| unsafe { std::ptr::write_volatile((base + rva as usize) as *mut u16, v) };
    let mut held = [false; 4];
    // virtual player of each gun
    let players: Vec<usize> = std::env::var("WAL_TYPEX_GUN_PLAYERS")
        .map(|v| v.split(',').filter_map(|p| p.trim().parse::<usize>().ok()).filter(|p| (1..=4).contains(p)).map(|p| p - 1).collect())
        .unwrap_or_default();
    if !players.is_empty() {
        log!("guns: players {players:?} drive the guns 1..");
    }
    loop {
        for &(rva, v) in game.constants {
            byte(rva, v);
        }
        for (player, gun) in game.guns.iter().enumerate() {
            let stick = wal_payload_common::input(players.get(player).copied().unwrap_or(player));
            let trigger = stick.pressed(button::B1);
            for &rva in gun.trigger {
                byte(rva, trigger as u8);
            }
            if let Some(rva) = gun.auto_fire {
                byte(rva, trigger as u8);
            }
            if let Some(rva) = gun.trigger_edge {
                byte(rva, (trigger && !held[player]) as u8);
            }
            held[player] = trigger;
            let off = offscreen(&stick) as u8;
            for &rva in gun.offscreen {
                byte(rva, off);
            }
            let (x, y) = (position(stick.axis(Axis::LeftX)), position(stick.axis(Axis::LeftY)));
            for &rva in gun.x {
                word(rva, x);
            }
            for &rva in gun.y {
                word(rva, y);
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
