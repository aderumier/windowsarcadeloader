//! Common protocol between the Linux launcher and the payload DLL injected in the game.
//!
//! Every game sees the same *virtual arcade stick* per player:
//!
//! ```text
//!   d-pad      [b1] [b2] [b3] [b4]      start  coin    card
//!              [b5] [b6] [b7] [b8]      service test
//!   left stick (lx, ly)   right stick (rx, ry)   pedals (accel, brake)
//! ```
//!
//! The launcher maps physical devices to this stick, the payload maps this stick
//! to the native I/O of the emulated arcade system.
//!
//! Wire format (little endian), over TCP on 127.0.0.1:
//! `[u8 kind][u8 reserved][u16 payload_len][payload]`

use std::io::{self, Read, Write};

pub const VERSION: u16 = 1;
pub const DEFAULT_PORT: u16 = 33700;
pub const MAX_PLAYERS: usize = 4;

/// Environment variables the launcher passes to the game process.
pub mod env {
    /// TCP port of the launcher input server.
    pub const PORT: &str = "WAL_PORT";
    /// Windows path of the payload log file (unset: no log).
    pub const LOG: &str = "WAL_LOG";
    /// Native mapping override, `virtual=native` pairs separated by `,`.
    pub const MAP: &str = "WAL_MAP";
}

/// Digital inputs of the virtual stick, as bits of [`StickState::buttons`].
pub mod button {
    pub const UP: u32 = 1 << 0;
    pub const DOWN: u32 = 1 << 1;
    pub const LEFT: u32 = 1 << 2;
    pub const RIGHT: u32 = 1 << 3;
    pub const B1: u32 = 1 << 4;
    pub const B2: u32 = 1 << 5;
    pub const B3: u32 = 1 << 6;
    pub const B4: u32 = 1 << 7;
    pub const B5: u32 = 1 << 8;
    pub const B6: u32 = 1 << 9;
    pub const B7: u32 = 1 << 10;
    pub const B8: u32 = 1 << 11;
    pub const START: u32 = 1 << 12;
    pub const COIN: u32 = 1 << 13;
    pub const SERVICE: u32 = 1 << 14;
    pub const TEST: u32 = 1 << 15;
    /// Insert/remove the player's card (card readers: NESiCA, ...), toggles on each press.
    pub const CARD: u32 = 1 << 16;

    pub const NAMES: [(&str, u32); 17] = [
        ("up", UP),
        ("down", DOWN),
        ("left", LEFT),
        ("right", RIGHT),
        ("b1", B1),
        ("b2", B2),
        ("b3", B3),
        ("b4", B4),
        ("b5", B5),
        ("b6", B6),
        ("b7", B7),
        ("b8", B8),
        ("start", START),
        ("coin", COIN),
        ("service", SERVICE),
        ("test", TEST),
        ("card", CARD),
    ];

    pub fn from_name(name: &str) -> Option<u32> {
        NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, b)| *b)
    }
}

/// Analog inputs of the virtual stick, index in [`StickState::axes`].
///
/// Sticks are centered on 0 (-32768..=32767, +y is down), pedals go from 0 (released)
/// to 32767 (fully pressed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Axis {
    LeftX = 0,
    LeftY = 1,
    RightX = 2,
    RightY = 3,
    Accel = 4,
    Brake = 5,
}

impl Axis {
    pub const COUNT: usize = 6;
    pub const ALL: [Axis; Axis::COUNT] =
        [Axis::LeftX, Axis::LeftY, Axis::RightX, Axis::RightY, Axis::Accel, Axis::Brake];

    pub fn name(self) -> &'static str {
        match self {
            Axis::LeftX => "lx",
            Axis::LeftY => "ly",
            Axis::RightX => "rx",
            Axis::RightY => "ry",
            Axis::Accel => "accel",
            Axis::Brake => "brake",
        }
    }

    pub fn from_name(name: &str) -> Option<Axis> {
        Axis::ALL.into_iter().find(|a| a.name().eq_ignore_ascii_case(name))
    }

    pub fn is_pedal(self) -> bool {
        matches!(self, Axis::Accel | Axis::Brake)
    }
}

/// State of one player's virtual arcade stick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StickState {
    pub buttons: u32,
    pub axes: [i16; Axis::COUNT],
}

impl StickState {
    pub const WIRE_SIZE: usize = 4 + 2 * Axis::COUNT;

    pub fn pressed(&self, button: u32) -> bool {
        self.buttons & button != 0
    }

    pub fn axis(&self, axis: Axis) -> i16 {
        self.axes[axis as usize]
    }

    /// Axis scaled to 0..=255 (128 = center for sticks, 0 = released for pedals).
    pub fn axis_u8(&self, axis: Axis) -> u8 {
        let v = self.axis(axis) as i32;
        if axis.is_pedal() { (v.max(0) * 255 / 32767) as u8 } else { ((v + 32768) >> 8) as u8 }
    }

    /// Axis scaled to 0..=65535 (32768 = center for sticks, 0 = released for pedals).
    pub fn axis_u16(&self, axis: Axis) -> u16 {
        let v = self.axis(axis) as i32;
        if axis.is_pedal() { (v.max(0) * 65535 / 32767) as u16 } else { (v + 32768) as u16 }
    }
}

/// Inputs of every player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputFrame {
    pub players: [StickState; MAX_PLAYERS],
}

impl InputFrame {
    pub const WIRE_SIZE: usize = StickState::WIRE_SIZE * MAX_PLAYERS;

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::WIRE_SIZE);
        for p in &self.players {
            out.extend_from_slice(&p.buttons.to_le_bytes());
            for a in p.axes {
                out.extend_from_slice(&a.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<InputFrame> {
        if data.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut frame = InputFrame::default();
        for (p, chunk) in frame.players.iter_mut().zip(data.chunks_exact(StickState::WIRE_SIZE)) {
            p.buttons = u32::from_le_bytes(chunk[0..4].try_into().unwrap());
            for (i, a) in p.axes.iter_mut().enumerate() {
                *a = i16::from_le_bytes([chunk[4 + i * 2], chunk[5 + i * 2]]);
            }
        }
        Some(frame)
    }
}

/// Output ids understood by the launcher. Force feedback outputs are states (sent on change):
/// the launcher plays them on the player's devices until the next value.
pub mod output {
    /// Constant force on the wheel, -10000 (pushed to the left) ..= 10000 (to the right).
    pub const FFB_CONSTANT: u16 = 1;
    /// Centering spring strength, 0 ..= 10000.
    pub const FFB_SPRING: u16 = 2;
    /// Vibration strength (a sine on wheels, rumble on gamepads), 0 ..= 10000.
    pub const FFB_VIBRATION: u16 = 3;
    /// Full scale of the force feedback values.
    pub const FFB_MAX: i32 = 10000;
}

/// Output from the game (lamps, force feedback, recoil...), identified by a
/// system-defined id ([`output`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub player: u8,
    pub id: u16,
    pub value: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// First message sent by both sides.
    Hello { version: u16, name: String },
    /// Launcher -> game: full input state.
    Input(InputFrame),
    /// Game -> launcher.
    Output(Output),
}

const KIND_HELLO: u8 = 1;
const KIND_INPUT: u8 = 2;
const KIND_OUTPUT: u8 = 3;

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Message::Hello { version, name } => {
                let mut p = version.to_le_bytes().to_vec();
                p.extend_from_slice(name.as_bytes());
                (KIND_HELLO, p)
            }
            Message::Input(frame) => (KIND_INPUT, frame.encode()),
            Message::Output(o) => {
                let mut p = vec![o.player];
                p.extend_from_slice(&o.id.to_le_bytes());
                p.extend_from_slice(&o.value.to_le_bytes());
                (KIND_OUTPUT, p)
            }
        };
        let mut out = Vec::with_capacity(4 + payload.len());
        out.push(kind);
        out.push(0);
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Decodes one message body. Unknown kinds return `None` so newer peers stay compatible.
    pub fn decode(kind: u8, payload: &[u8]) -> Option<Message> {
        match kind {
            KIND_HELLO if payload.len() >= 2 => Some(Message::Hello {
                version: u16::from_le_bytes([payload[0], payload[1]]),
                name: String::from_utf8_lossy(&payload[2..]).into_owned(),
            }),
            KIND_INPUT => InputFrame::decode(payload).map(Message::Input),
            KIND_OUTPUT if payload.len() >= 7 => Some(Message::Output(Output {
                player: payload[0],
                id: u16::from_le_bytes([payload[1], payload[2]]),
                value: i32::from_le_bytes(payload[3..7].try_into().unwrap()),
            })),
            _ => None,
        }
    }

    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&self.encode())
    }

    /// Blocking read of the next known message.
    pub fn read_from(r: &mut impl Read) -> io::Result<Message> {
        loop {
            let mut header = [0u8; 4];
            r.read_exact(&mut header)?;
            let len = u16::from_le_bytes([header[2], header[3]]) as usize;
            let mut payload = vec![0u8; len];
            r.read_exact(&mut payload)?;
            if let Some(msg) = Message::decode(header[0], &payload) {
                return Ok(msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut frame = InputFrame::default();
        frame.players[0].buttons = button::B1 | button::START;
        frame.players[1].axes[Axis::Accel as usize] = 32767;
        frame.players[3].axes[Axis::LeftY as usize] = -32768;
        for msg in [
            Message::Hello { version: VERSION, name: "nesica".into() },
            Message::Input(frame),
            Message::Output(Output { player: 1, id: 7, value: -5 }),
        ] {
            let bytes = msg.encode();
            assert_eq!(Message::read_from(&mut bytes.as_slice()).unwrap(), msg);
        }
    }

    #[test]
    fn axis_u8() {
        let mut s = StickState::default();
        assert_eq!(s.axis_u8(Axis::LeftX), 128);
        s.axes[Axis::LeftX as usize] = -32768;
        assert_eq!(s.axis_u8(Axis::LeftX), 0);
        s.axes[Axis::LeftX as usize] = 32767;
        assert_eq!(s.axis_u8(Axis::LeftX), 255);
        s.axes[Axis::Brake as usize] = 32767;
        assert_eq!(s.axis_u8(Axis::Brake), 255);
    }

    #[test]
    fn axis_u16() {
        let mut s = StickState::default();
        assert_eq!(s.axis_u16(Axis::LeftX), 32768);
        assert_eq!(s.axis_u16(Axis::Accel), 0);
        s.axes[Axis::LeftX as usize] = -32768;
        assert_eq!(s.axis_u16(Axis::LeftX), 0);
        s.axes[Axis::LeftX as usize] = 32767;
        assert_eq!(s.axis_u16(Axis::LeftX), 65535);
        s.axes[Axis::Accel as usize] = 32767;
        assert_eq!(s.axis_u16(Axis::Accel), 65535);
    }
}
