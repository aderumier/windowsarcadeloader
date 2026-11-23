//! Network outputs, MAME's protocol (`WAL_OUTPUTS`, the GUI's Common options): the game's
//! lamps, recoil and drive board for MAMEHooker, BackForceFeeder or any tool reading MAME's
//! network outputs, without a plugin in the game. The payload sends the values by name
//! (`Message::Named`, payload-common `outputs.rs`); here:
//!
//! * a TCP server (`WAL_OUTPUTS_PORT`, 8000) sends every client `name = value\r` lines, and
//!   `mame_start = <game>\r` with every known value when it connects;
//! * one UDP datagram `mame_start = <game>\rtcp = <port>\r` to 127.0.0.1:8001 at the start;
//! * `mame_stop = 1\r` when the game ends.
//!
//! The game's force (`FFB_CONSTANT`) also goes out as a Sega servo board's frames on
//! `RawDrive` (80, 81, 83 once, then 84 the torque), unless the game sends a drive board's
//! own bytes.

use std::io::Write;
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wal_protocol::{Output, output};

pub const ENV: &str = "WAL_OUTPUTS";
pub const PORT_ENV: &str = "WAL_OUTPUTS_PORT";
/// The game's name for the tools (`mame_start`, MAMEHooker's ini file): a profile's own, else
/// its file name.
pub const NAME_ENV: &str = "WAL_OUTPUTS_NAME";
const UDP_PORT: u16 = 8001;

#[derive(Default)]
struct State {
    game: String,
    /// Known values, in the order first set.
    values: Vec<(String, i32)>,
    clients: Vec<TcpStream>,
    /// The game sent a drive board's own bytes: no frames made from the force.
    raw_drive: bool,
    servo_started: bool,
    torque: Option<i32>,
}

impl State {
    fn store(&mut self, name: &str, value: i32) {
        match self.values.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value,
            None => self.values.push((name.to_string(), value)),
        }
    }

    fn broadcast(&mut self, text: &str) {
        self.clients.retain_mut(|c| c.write_all(text.as_bytes()).is_ok());
    }

    /// A Sega servo board frame: command, two 7-bit bytes, checksum.
    fn servo_frame(&mut self, command: u8, first: u8, second: u8) {
        let f = [command, first & 0x7F, second & 0x7F, (command as u16 + (first & 0x7F) as u16 + (second & 0x7F) as u16) as u8 & 0x7F];
        let mut text = String::new();
        for b in f {
            self.store("RawDrive", b as i32);
            text += &format!("RawDrive = {b}\r");
        }
        self.broadcast(&text);
    }

    fn force(&mut self, value: i32) {
        // the servo board's torque: 84's value - 0x80, > 0 pushing left (FFB_CONSTANT < 0)
        let torque = (-value as i64 * 127 / output::FFB_MAX as i64) as i32;
        if self.raw_drive || self.torque == Some(torque) {
            return;
        }
        if !self.servo_started {
            self.servo_frame(0x80, 0x00, 0x01); // effects on, permanent
            self.servo_frame(0x81, 0x7F, 0x7F); // no cap
            self.servo_frame(0x83, 0x7F, 0x00); // full gain
            self.servo_started = true;
        }
        let raw = (torque + 0x80) as u8;
        self.servo_frame(0x84, raw >> 7, raw);
        self.torque = Some(torque);
    }
}

pub struct NetOutputs {
    state: Arc<Mutex<State>>,
}

impl NetOutputs {
    /// Starts the server for `game` (the name tools know it by) on TCP `port`; `None` when it
    /// cannot listen.
    pub fn start(game: &str, port: u16) -> Option<NetOutputs> {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("outputs: cannot listen on TCP port {port} ({e}), no network outputs");
                return None;
            }
        };
        let state = Arc::new(Mutex::new(State { game: game.to_string(), ..State::default() }));
        let s = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                let mut st = s.lock().unwrap();
                let mut text = format!("mame_start = {}\r", st.game);
                for (n, v) in &st.values {
                    text += &format!("{n} = {v}\r");
                }
                let mut stream = stream;
                if stream.write_all(text.as_bytes()).is_ok() {
                    eprintln!("outputs: a client connected ({})", stream.peer_addr().map(|a| a.to_string()).unwrap_or_default());
                    st.clients.push(stream);
                }
            }
        });
        if let Ok(udp) = UdpSocket::bind(("127.0.0.1", 0)) {
            let _ = udp.send_to(format!("mame_start = {game}\rtcp = {port}\r").as_bytes(), ("127.0.0.1", UDP_PORT));
        }
        eprintln!("outputs: listening on TCP port {port} (game \"{game}\"), start announced to 127.0.0.1:{UDP_PORT}");
        Some(NetOutputs { state })
    }

    /// A value of the game, sent when it changed (`RawDrive`: each byte).
    pub fn set(&self, name: &str, value: i32) {
        let mut st = self.state.lock().unwrap();
        if name == "RawDrive" {
            st.raw_drive = true;
        } else if st.values.iter().any(|(n, v)| n == name && *v == value) {
            return;
        }
        st.store(name, value);
        st.broadcast(&format!("{name} = {value}\r"));
    }

    /// The game's force feedback outputs: player 1's constant force as servo board frames.
    pub fn output(&self, o: &Output) {
        if o.player == 0 && o.id == output::FFB_CONSTANT {
            self.state.lock().unwrap().force(o.value);
        }
    }

    /// The game ended: no torque left, `mame_stop = 1`, the clients closed.
    pub fn stop(&self) {
        let mut st = self.state.lock().unwrap();
        if st.servo_started && !st.raw_drive && st.torque != Some(0) {
            st.servo_frame(0x84, 0x01, 0x00);
        }
        st.broadcast("mame_stop = 1\r");
        st.clients.clear();
        eprintln!("outputs: game stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn protocol() {
        let out = NetOutputs::start("sr3", 0).unwrap();
        // port 0: not reachable this way, the state alone
        out.set("lamp0", 1);
        out.set("lamp0", 1);
        out.output(&Output { player: 0, id: output::FFB_CONSTANT, value: -10000 });
        let st = out.state.lock().unwrap();
        assert_eq!(st.values[0], ("lamp0".to_string(), 1));
        // 80 00 01 01, 81 7F 7F 7F, 83 7F 00 02, 84 01 7F 04 (0x80 + 127 = 0xFF)
        assert_eq!(st.torque, Some(127));
        assert_eq!(st.values.iter().find(|(n, _)| n == "RawDrive").unwrap().1, 0x04);
    }

    #[test]
    fn client() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let out = NetOutputs::start("daytona", port).unwrap();
        out.set("RawLamps", 0x80);
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while !String::from_utf8_lossy(&got).contains("RawLamps = 128\r") {
            let n = c.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert!(String::from_utf8_lossy(&got).starts_with("mame_start = daytona\r"));
        std::thread::sleep(Duration::from_millis(50));
        out.set("lamp1", 1);
        out.stop();
        let mut rest = String::new();
        let _ = c.read_to_string(&mut rest);
        assert!(rest.contains("lamp1 = 1\r") && rest.ends_with("mame_stop = 1\r"), "{rest:?}");
    }
}
