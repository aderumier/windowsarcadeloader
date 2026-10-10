//! TCP server the game payload connects to (127.0.0.1 only).

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use wal_protocol::{InputFrame, Message, Output, VERSION};

pub struct Server {
    clients: Arc<Mutex<Vec<TcpStream>>>,
    last: Arc<Mutex<InputFrame>>,
    outputs: Receiver<Output>,
    relaunch: Arc<Mutex<Option<String>>>,
}

impl Server {
    pub fn start(port: u16) -> Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("binding 127.0.0.1:{port}"))?;
        let clients: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let last: Arc<Mutex<InputFrame>> = Arc::default();
        let (c, l) = (clients.clone(), last.clone());
        let (tx, outputs) = channel();
        let relaunch: Arc<Mutex<Option<String>>> = Arc::default();
        let r = relaunch.clone();
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
                    let (tx, r) = (tx.clone(), r.clone());
                    std::thread::spawn(move || read_client(stream, tx, r));
                }
            }
        });
        Ok(Server { clients, last, outputs, relaunch })
    }

    pub fn broadcast(&self, frame: &InputFrame) {
        *self.last.lock().unwrap() = *frame;
        let bytes = Message::Input(*frame).encode();
        self.clients.lock().unwrap().retain_mut(|c| c.write_all(&bytes).is_ok());
    }

    /// Outputs received from the game since the last call.
    pub fn outputs(&self) -> Vec<Output> {
        self.outputs.try_iter().collect()
    }

    /// The game's latest relaunch request (arguments added to the profile's), cleared.
    pub fn take_relaunch(&self) -> Option<String> {
        self.relaunch.lock().unwrap().take()
    }
}

fn read_client(mut stream: TcpStream, outputs: Sender<Output>, relaunch: Arc<Mutex<Option<String>>>) {
    while let Ok(msg) = Message::read_from(&mut stream) {
        match msg {
            Message::Hello { version, name } => eprintln!("server: payload '{name}' connected (protocol v{version})"),
            Message::Output(o) => {
                let _ = outputs.send(o);
            }
            Message::Relaunch { args } => {
                eprintln!("server: relaunch requested: {args}");
                *relaunch.lock().unwrap() = Some(args);
            }
            Message::Input(_) => {}
        }
    }
    eprintln!("server: payload disconnected");
}
