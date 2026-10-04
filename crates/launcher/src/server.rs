//! TCP server the game payload connects to (127.0.0.1 only).

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use wal_protocol::{InputFrame, Message, VERSION};

pub struct Server {
    clients: Arc<Mutex<Vec<TcpStream>>>,
    last: Arc<Mutex<InputFrame>>,
}

impl Server {
    pub fn start(port: u16) -> Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("binding 127.0.0.1:{port}"))?;
        let clients: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let last: Arc<Mutex<InputFrame>> = Arc::default();
        let (c, l) = (clients.clone(), last.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_nodelay(true);
                let mut writer = match stream.try_clone() {
                    Ok(w) => w,
                    Err(_) => continue,
                };
                let hello = Message::Hello { version: VERSION, name: "wal-launcher".into() };
                let frame = Message::Input(*l.lock().unwrap());
                if writer.write_all(&[hello.encode(), frame.encode()].concat()).is_ok() {
                    c.lock().unwrap().push(writer);
                    std::thread::spawn(move || read_client(stream));
                }
            }
        });
        Ok(Server { clients, last })
    }

    pub fn broadcast(&self, frame: &InputFrame) {
        *self.last.lock().unwrap() = *frame;
        let bytes = Message::Input(*frame).encode();
        self.clients.lock().unwrap().retain_mut(|c| c.write_all(&bytes).is_ok());
    }
}

fn read_client(mut stream: TcpStream) {
    while let Ok(msg) = Message::read_from(&mut stream) {
        match msg {
            Message::Hello { version, name } => eprintln!("server: payload '{name}' connected (protocol v{version})"),
            Message::Output(o) => eprintln!("server: output player {} id {} = {}", o.player, o.id, o.value),
            Message::Input(_) => {}
        }
    }
    eprintln!("server: payload disconnected");
}
