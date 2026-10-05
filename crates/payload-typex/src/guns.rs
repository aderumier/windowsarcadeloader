//! Type X2 lightgun games (`WAL_TYPEX_GUNS=<game>`): per-game gun inputs.
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

use std::time::Duration;

use wal_payload_common::{log, serial};
use wal_protocol::{Axis, StickState, button};

/// Addresses of one player's gun.
struct Gun {
    trigger: u32,
    offscreen: u32,
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
    // gun board connected, 4 players
    constants: &[(0x32F068, 0x02), (0xB3B820, 0x04)],
    guns: &[
        Gun { trigger: 0xB3B890, offscreen: 0xB3B830, x: &[0xB3B834, 0xB3B950], y: &[0xB3B836, 0xB3B960], trigger_edge: Some(0xB3B880), auto_fire: Some(0xB3B838) },
        Gun { trigger: 0xB3B894, offscreen: 0xB3B83A, x: &[0xB3B83E, 0xB3B954], y: &[0xB3B840, 0xB3B964], trigger_edge: Some(0xB3B884), auto_fire: Some(0xB3B842) },
        Gun { trigger: 0xB3B898, offscreen: 0xB3B844, x: &[0xB3B848, 0xB3B958], y: &[0xB3B84A, 0xB3B968], trigger_edge: Some(0xB3B888), auto_fire: Some(0xB3B84C) },
        Gun { trigger: 0xB3B89C, offscreen: 0xB3B84E, x: &[0xB3B852, 0xB3B95C], y: &[0xB3B854, 0xB3B96C], trigger_edge: Some(0xB3B88C), auto_fire: Some(0xB3B856) },
    ],
};

const MUSIC_GUNGUN_2: Game = Game {
    name: "music-gungun-2",
    // gun board connected, JVS type
    constants: &[(0x2B8128, 0x02), (0x2B3708, 0x03)],
    guns: &[
        Gun { trigger: 0x2B8108, offscreen: 0x2B8102, x: &[0x2B8104], y: &[0x2B8106], trigger_edge: None, auto_fire: None },
        Gun { trigger: 0x2B8112, offscreen: 0x2B810C, x: &[0x2B810E], y: &[0x2B8110], trigger_edge: None, auto_fire: None },
    ],
};

const GAMES: &[Game] = &[GAIA_ATTACK_4, MUSIC_GUNGUN_2];

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
    loop {
        for &(rva, v) in game.constants {
            byte(rva, v);
        }
        for (player, gun) in game.guns.iter().enumerate() {
            let stick = wal_payload_common::input(player);
            let trigger = stick.pressed(button::B1);
            byte(gun.trigger, trigger as u8);
            if let Some(rva) = gun.auto_fire {
                byte(rva, trigger as u8);
            }
            if let Some(rva) = gun.trigger_edge {
                byte(rva, (trigger && !held[player]) as u8);
            }
            held[player] = trigger;
            byte(gun.offscreen, offscreen(&stick) as u8);
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
