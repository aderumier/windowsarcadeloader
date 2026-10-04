//! NESYS network service emulation.
//!
//! Based on WindowsLoader's NesysEmu, corrected with FakeNesicaService
//! (github.com/ArcadeMachinist/FakeNesicaService), whose layouts come from captures of the
//! real service: the network must be reported up (`NWRECOVER_NOTICE`) between the connect
//! reply and the certificate, or games such as KOF XIII Climax stay on "initializing network".
//!
//! NESiCA games talk to the NESiCAxLive service through the message-mode named pipe
//! `\\.\pipe\nesys_games`: `{u32 command, u32 length, data}` requests, same framing for
//! replies. Every request is answered as if the cabinet were online, with free play.
//! Card data is stored in `card_<id>_<type>.bin` in the game's D: data folder.

use std::thread;
use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};

mod cmd {
    pub const CLIENT_START: u32 = 0x1;
    pub const CONNECT_REQUEST: u32 = 0x2;
    pub const DISCONNECT_REQUEST: u32 = 0x3;
    pub const GAME_START_REQUEST: u32 = 0x4;
    pub const GAME_END_REQUEST: u32 = 0x5;
    pub const GAME_CONTINUE_REQUEST: u32 = 0x6;
    pub const CARD_SELECT_REQUEST: u32 = 0x9;
    pub const CARD_INSERT_REQUEST: u32 = 0xA;
    pub const CARD_UPDATE_REQUEST: u32 = 0xB;
    pub const RANKING_DATA_REQUEST: u32 = 0x13;
    pub const LOCALNW_INFO_REQUEST: u32 = 0x14;
    pub const GLOBALADDR_REQUEST: u32 = 0x15;
    pub const ADAPTER_INFO_REQUEST: u32 = 0x17;
    pub const SERVICE_VERSION_REQUEST: u32 = 0x18;
    pub const UPLOAD_CONFIG_REQUEST: u32 = 0x1C;
    pub const INCOME_START_REQUEST: u32 = 0x1D;
    pub const INCOME_END_REQUEST: u32 = 0x1E;
    pub const INCOME_CONTINUE_REQUEST: u32 = 0x1F;
    pub const SET_INCOME_MODE_REQUEST: u32 = 0x20;
    pub const GAMESTATUS_RESET_REQUEST: u32 = 0x23;
    pub const ROW_EVENTDATA_LIST_REQUEST: u32 = 0x24;
    pub const GAME_FREE_START_REQUEST: u32 = 0x29;
    pub const GAME_FREE_END_REQUEST: u32 = 0x2A;
    pub const INCOME_FREE_START_REQUEST: u32 = 0x2B;

    pub const NWRECOVER_NOTICE: u32 = 0x103;
    pub const CERT_INIT_NOTICE: u32 = 0x107;
    pub const CLIENT_START_REPLY: u32 = 0x10D;
    pub const CONNECT_REPLY: u32 = 0x10E;
    pub const DISCONNECT_REPLY: u32 = 0x10F;
    pub const GAME_STATUS_REPLY: u32 = 0x110;
    pub const CARD_SELECT_REPLY: u32 = 0x111;
    pub const CARD_INSERT_REPLY: u32 = 0x112;
    pub const CARD_UPDATE_REPLY: u32 = 0x113;
    pub const RANKING_DATA_REPLY: u32 = 0x11A;
    pub const LOCALNW_INFO_REPLY: u32 = 0x11B;
    pub const LOCALNW_INFO_NOTICE: u32 = 0x11C;
    pub const GLOBALADDR_REPLY: u32 = 0x11D;
    pub const ADAPTER_INFO_REPLY: u32 = 0x11F;
    pub const SERVICE_VERSION_REPLY: u32 = 0x120;
    pub const UPLOAD_CONFIG_REPLY: u32 = 0x123;
    pub const INCOME_STATUS_REPLY: u32 = 0x124;
    pub const SET_INCOME_MODE_REPLY: u32 = 0x125;
    pub const GAMESTATUS_RESET_REPLY: u32 = 0x127;
    pub const ROW_EVENTDATA_LIST_REPLY: u32 = 0x128;
}

const BUFFER: usize = 8192;
const MAC: &str = "DEADBABECAFE";

struct Pipe(HANDLE);
// The handle is only used by the thread owning the connection.
unsafe impl Send for Pipe {}

pub(crate) fn start() {
    thread::spawn(|| {
        let name: Vec<u16> = "\\\\.\\pipe\\nesys_games\0".encode_utf16().collect();
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    BUFFER as u32,
                    BUFFER as u32,
                    0,
                    std::ptr::null(),
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                log!("nesys: cannot create pipe");
                return;
            }
            let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !connected {
                unsafe { CloseHandle(pipe) };
                continue;
            }
            log!("nesys: client connected");
            let pipe = Pipe(pipe);
            thread::spawn(move || serve(pipe));
        }
    });
}

fn serve(pipe: Pipe) {
    let mut buf = vec![0u8; BUFFER];
    loop {
        let Some(message) = read_message(&pipe, &mut buf) else { break };
        let mut data = &message[..];
        while data.len() >= 8 {
            let command = u32_at(data, 0);
            let len = (u32_at(data, 4) as usize).min(data.len() - 8);
            if command != 0 {
                handle(&pipe, command, &data[8..8 + len]);
            }
            data = &data[8 + len..];
        }
        thread::sleep(Duration::from_millis(150));
    }
    unsafe { CloseHandle(pipe.0) };
    log!("nesys: client disconnected");
}

/// Reads one pipe message, whatever its size (BBCF uploads ~60 KB messages).
fn read_message(pipe: &Pipe, buf: &mut [u8]) -> Option<Vec<u8>> {
    let mut message = Vec::new();
    loop {
        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe.0, buf.as_mut_ptr(), buf.len() as u32, &mut read, std::ptr::null_mut()) };
        message.extend_from_slice(&buf[..read as usize]);
        if ok != 0 && read > 0 {
            return Some(message);
        }
        let error = unsafe { GetLastError() };
        if ok == 0 && error == ERROR_MORE_DATA {
            continue;
        }
        log!("nesys: read ended: ok {ok} read {read} error {error}");
        return None;
    }
}

fn u32_at(d: &[u8], off: usize) -> u32 {
    d.get(off..off + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn send(pipe: &Pipe, command: u32, data: &[u8]) {
    let mut out = Vec::with_capacity(8 + data.len());
    out.extend_from_slice(&command.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    let mut written = 0u32;
    let ok = unsafe { WriteFile(pipe.0, out.as_ptr(), out.len() as u32, &mut written, std::ptr::null_mut()) };
    if ok == 0 {
        log!("nesys: reply {command:#x} ({} bytes) failed: error {}", data.len(), unsafe { GetLastError() });
    } else {
        log!("nesys: reply {command:#x} ({} bytes)", data.len());
    }
}

/// Little-endian struct builder following the C layout of the replies.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    /// Fixed size, zero padded C string.
    fn str(mut self, s: &str, size: usize) -> Self {
        let bytes = s.as_bytes();
        let n = bytes.len().min(size - 1);
        self.0.extend_from_slice(&bytes[..n]);
        self.0.resize(self.0.len() + size - n, 0);
        self
    }
    fn bytes(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(b);
        self
    }
    fn align4(mut self) -> Self {
        self.0.resize((self.0.len() + 3) & !3, 0);
        self
    }
}

fn net(name: &str, default: &str) -> String {
    std::env::var(format!("WAL_NESICA_{name}")).unwrap_or_else(|_| default.to_string())
}

fn card_file(card_id: &[u8], data_type: u32) -> String {
    let id: String = card_id.iter().take(16).take_while(|c| **c != 0).map(|c| *c as char).collect();
    format!("{}\\card_{id}_{data_type}.bin", crate::drive::data_dir())
}

fn card_status(times: u32, data: &[u8]) -> Vec<u8> {
    Out::default().u32(1).u32(times).u32(times).u32(data.len() as u32).bytes(data).0
}

fn handle(pipe: &Pipe, command: u32, data: &[u8]) {
    use cmd::*;
    log!("nesys: command {command:#x} ({} bytes)", data.len());
    match command {
        CLIENT_START => send(pipe, CLIENT_START_REPLY, &[]),
        CONNECT_REQUEST => {
            send(pipe, CONNECT_REPLY, &[]);
            thread::sleep(Duration::from_millis(30));
            // the network is up
            send(pipe, NWRECOVER_NOTICE, &[]);
            thread::sleep(Duration::from_millis(30));
            let server = "card_id=7020392010281502";
            let news = "./WindowsLoader/news.png";
            let reply = Out::default()
                // NESYS_TENPO_TABLE
                .u32(1337)
                .str("leet", 31)
                .str("l33t", 33)
                .str("teel", 33)
                .str("t33l", 17)
                .align4()
                // NESYS_NEWS_TABLE (values of a real service capture)
                .u32(8)
                .u32(1337)
                .str(news, 1024)
                .u32(server.len() as u32)
                .bytes(server.as_bytes());
            send(pipe, CERT_INIT_NOTICE, &reply.0);
            // experiments: extra notices after the certificate, `0x105,0x10c[:payload hex]`
            if let Ok(extra) = std::env::var("WAL_NESICA_NESYS_EXTRA") {
                for item in extra.split(',').map(str::trim).filter(|i| !i.is_empty()) {
                    let (cmd, data) = item.split_once(':').unwrap_or((item, ""));
                    let Ok(cmd) = u32::from_str_radix(cmd.trim_start_matches("0x"), 16) else { continue };
                    let data: Vec<u8> = (0..data.len() / 2)
                        .filter_map(|i| u8::from_str_radix(&data[i * 2..i * 2 + 2], 16).ok())
                        .collect();
                    thread::sleep(Duration::from_millis(100));
                    send(pipe, cmd, &data);
                }
            }
        }
        DISCONNECT_REQUEST => send(pipe, DISCONNECT_REPLY, &[]),
        GAME_START_REQUEST | GAME_END_REQUEST | GAME_CONTINUE_REQUEST | GAME_FREE_START_REQUEST
        | GAME_FREE_END_REQUEST => {
            send(pipe, GAME_STATUS_REPLY, &[])
        }
        CARD_SELECT_REQUEST => {
            // { char cardId[16]; u32 dataType; u32 paramSize; ... }
            let file = card_file(data, u32_at(data, 16));
            let mut card = std::fs::read(&file).unwrap_or_default();
            card.truncate(16383);
            card.push(0);
            if card.len() == 1 {
                card = vec![0; 16384];
            }
            send(pipe, CARD_SELECT_REPLY, &card_status(100, &card));
        }
        CARD_INSERT_REQUEST => send(pipe, CARD_INSERT_REPLY, &Out::default().u32(1).u32(0).u32(0).u32(0).0),
        CARD_UPDATE_REQUEST => {
            // { char cardId[16]; u32 frequency; u32 transferMode; u32 dataType; u32 dataSize; data }
            let size = (u32_at(data, 28) as usize).min(data.len().saturating_sub(32));
            let card = &data[32.min(data.len())..32 + size];
            let file = card_file(data, u32_at(data, 24));
            if let Err(e) = std::fs::write(&file, card) {
                log!("nesys: cannot save {file}: {e}");
            }
            send(pipe, CARD_UPDATE_REPLY, &card_status(100, card));
        }
        INCOME_START_REQUEST | INCOME_END_REQUEST | INCOME_CONTINUE_REQUEST | INCOME_FREE_START_REQUEST => {
            send(pipe, INCOME_STATUS_REPLY, &Out::default().u32(0).u32(1).0)
        }
        RANKING_DATA_REQUEST => send(pipe, RANKING_DATA_REPLY, &Out::default().u32(1).u32(0).0),
        UPLOAD_CONFIG_REQUEST => send(pipe, UPLOAD_CONFIG_REPLY, &Out::default().u32(1).0),
        SERVICE_VERSION_REQUEST => {
            send(pipe, SERVICE_VERSION_REPLY, &Out::default().str("13.37 1337/01/01", 33).0)
        }
        SET_INCOME_MODE_REQUEST => send(pipe, SET_INCOME_MODE_REPLY, &[]),
        GAMESTATUS_RESET_REQUEST => send(pipe, GAMESTATUS_RESET_REPLY, &[]),
        ROW_EVENTDATA_LIST_REQUEST => {
            // { u32 size, data[size] }: no event
            send(pipe, ROW_EVENTDATA_LIST_REPLY, &Out::default().u32(0x840).bytes(&[0; 0x840]).0)
        }
        LOCALNW_INFO_REQUEST => {
            send(pipe, LOCALNW_INFO_REPLY, &[]);
            thread::sleep(Duration::from_millis(100));
            // one interface block, layout of a real service capture
            let table = Out::default()
                .str(MAC, 16)
                .str(&net("IP", "192.168.1.2"), 16)
                .str(&net("GATEWAY", "192.168.1.1"), 16)
                .str(&net("DNS", "192.168.1.1"), 16)
                .u32(0x157c)
                .u32(0);
            let reply = Out::default().u32(1).u32(table.0.len() as u32).bytes(&table.0);
            send(pipe, LOCALNW_INFO_NOTICE, &reply.0);
        }
        GLOBALADDR_REQUEST => {
            send(pipe, GLOBALADDR_REPLY, &Out::default().u32(1).u32(0x1a6).str(&net("GLOBAL_IP", "127.1.3.37"), 16).0)
        }
        ADAPTER_INFO_REQUEST => {
            let reply = Out::default()
                .u32(1)
                .u32(0) // DHCP disabled
                .str(&net("GATEWAY", "192.168.1.1"), 32)
                .str(&net("IP", "192.168.1.2"), 32)
                .str(&net("MASK", "255.255.255.0"), 32)
                .str(&net("DNS", "192.168.1.1"), 32)
                .str(&net("DNS", "192.168.1.1"), 32)
                .str(MAC, 64);
            send(pipe, ADAPTER_INFO_REPLY, &reply.0);
        }
        _ => log!("nesys: unhandled command {command:#x}"),
    }
}
