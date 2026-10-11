//! NESYS network service emulation.
//!
//! Corrected with FakeNesicaService
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
    pub const CARD_REISSUE_TEST_REQUEST: u32 = 0x10;
    pub const CARD_REISSUE_REQUEST: u32 = 0x11;
    pub const RANKING_DATA_REQUEST: u32 = 0x13;
    pub const LOCALNW_INFO_REQUEST: u32 = 0x14;
    pub const GLOBALADDR_REQUEST: u32 = 0x15;
    pub const ADAPTER_INFO_REQUEST: u32 = 0x17;
    pub const SERVICE_VERSION_REQUEST: u32 = 0x18;
    pub const HTTPACCESS_GET_REQUEST: u32 = 0x1A;
    pub const HTTPACCESS_POST_REQUEST: u32 = 0x1B;
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

    pub const NW_ERROR_NOTICE: u32 = 0x101;
    pub const NWRECOVER_NOTICE: u32 = 0x103;
    pub const CERT_INIT_NOTICE: u32 = 0x107;
    pub const CLIENT_START_REPLY: u32 = 0x10D;
    pub const CONNECT_REPLY: u32 = 0x10E;
    pub const DISCONNECT_REPLY: u32 = 0x10F;
    pub const GAME_STATUS_REPLY: u32 = 0x110;
    pub const CARD_SELECT_REPLY: u32 = 0x111;
    pub const CARD_INSERT_REPLY: u32 = 0x112;
    pub const CARD_UPDATE_REPLY: u32 = 0x113;
    // 0x114 buys item, 0x115 takeover, 0x116 decrease (no reply for the force takeover request)
    pub const CARD_REISSUE_TEST_REPLY: u32 = 0x117;
    pub const CARD_REISSUE_REPLY: u32 = 0x118;
    pub const RANKING_DATA_REPLY: u32 = 0x11A;
    pub const LOCALNW_INFO_REPLY: u32 = 0x11B;
    pub const LOCALNW_INFO_NOTICE: u32 = 0x11C;
    pub const GLOBALADDR_REPLY: u32 = 0x11D;
    pub const ADAPTER_INFO_REPLY: u32 = 0x11F;
    pub const SERVICE_VERSION_REPLY: u32 = 0x120;
    pub const HTTPACCESS_START: u32 = 0x121;
    pub const HTTPACCESS_REPLY: u32 = 0x122;
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

fn c_str(d: &[u8]) -> String {
    String::from_utf8_lossy(d.split(|b| *b == 0).next().unwrap_or_default()).into_owned()
}

/// HTTP request the game sends through the service (NESiCAxLive 2 games: `ms` server of the
/// certificate). Request (as sent by Million Arthur / SNK games): `{ char url[1024],
/// proxy[1024], authName[32], authPassword[32]; u32 ?, port (+0x844), sequence?, timeout?,
/// mode?, dataSize (+0x854); char httpHeader[1025]; data (+0xC59) }`. Answered
/// `HTTPACCESS_START` {u32 1}, then `HTTPACCESS_REPLY` `{ char httpHeader[1025] (aligned 4);
/// u32 httpStatusCode, errorCode, sequence, dataSize; data }`.
fn http_access(pipe: &Pipe, data: &[u8]) {
    use cmd::{HTTPACCESS_REPLY, HTTPACCESS_START};
    const HEADER: usize = 0x858;
    const BODY: usize = HEADER + 1025;
    let url = c_str(data.get(..1024).unwrap_or_default());
    let port = u32_at(data, 0x844);
    let sequence = u32_at(data, 0x848);
    let size = (u32_at(data, 0x854) as usize).min(data.len().saturating_sub(BODY));
    let header = c_str(data.get(HEADER..BODY).unwrap_or_default());
    let body = data.get(BODY..BODY + size).unwrap_or_default();
    let fields: Vec<String> = (0..6).map(|i| format!("{:#x}", u32_at(data, 0x840 + 4 * i))).collect();
    log!("nesys: http {url} port {port} seq {sequence} fields {} header {header:?}", fields.join(" "));
    log!("nesys: http body {}", String::from_utf8_lossy(body));
    if std::env::var("WAL_NESYS_HTTP_DUMP").is_ok() {
        static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::write(format!("{}\\http-{n:02}.bin", crate::drive::data_dir()), data);
    }
    // `WAL_NESYS_OBJECT_REPLY=none`: object storage requests unanswered (the request then
    // times out in the game)
    if header.contains("multipart/form-data") && std::env::var("WAL_NESYS_OBJECT_REPLY").is_ok_and(|v| v == "none") {
        log!("nesys: http object storage request left unanswered");
        return;
    }
    send(pipe, HTTPACCESS_START, &Out::default().u32(1).0);
    // card service protocol (`card_id=...&command=...&data=<xml>&type=...` form posts):
    // result `1` (OK) and a newline (the game skips these 2 bytes), then the XML data. Million
    // Arthur's first request (type 8456) reads `/root/match_conf` (`record` children): no
    // online match configuration.
    // Multipart posts (`object_data`, `bucket_id`, `index`: the card object storage, Samurai
    // Shodown's character data before a fight): 404, no stored object.
    let object_storage = header.contains("multipart/form-data");
    // `WAL_NESYS_OBJECT_REPLY=<status>:<body>` (`\n` escapes): experiments
    let object_reply = std::env::var("WAL_NESYS_OBJECT_REPLY").ok().and_then(|v| {
        let (status, body) = v.split_once(':')?;
        Some((status.parse::<u32>().ok()?, body.replace("\\n", "\n").into_bytes()))
    });
    let custom_body;
    let (status, status_line, reply_body): (u32, &str, &[u8]) = if object_storage && let Some((status, body)) = object_reply {
        custom_body = body;
        (status, "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n", &custom_body)
    } else if object_storage {
        (404, "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n", b"")
    } else {
        (
            200,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n",
            b"1\n<?xml version='1.0' encoding='UTF-8'?>\n<root>\n<match_conf></match_conf>\n</root>\n",
        )
    };
    let reply = Out::default()
        .str(status_line, 1025)
        .align4();
    // `WAL_NESYS_HTTP_SWAP=1`: the status in the second field (experiment)
    let swap = object_storage && std::env::var("WAL_NESYS_HTTP_SWAP").is_ok_and(|v| v == "1");
    let reply = if swap { reply.u32(0).u32(status) } else { reply.u32(status).u32(0) };
    let reply = reply
        .u32(sequence)
        .u32(reply_body.len() as u32)
        .bytes(reply_body);
    send(pipe, HTTPACCESS_REPLY, &reply.0);
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

/// Card data type of a session read (Fighting EX Layer, after the card is accepted): no
/// session (no data) is "this NESiCA cannot be used".
const CARD_GET_SESSION: u32 = 401;

/// The session of a session read, whose parameter is `<root><info>...<card_id>`,
/// `<mac_addr>...`: the card server's `/root/session` answer.
fn session_xml(param: &[u8]) -> String {
    let param = String::from_utf8_lossy(param);
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = param.find(&open).map(|i| i + open.len());
        start.and_then(|s| param[s..].find('<').map(|e| param[s..s + e].to_string())).unwrap_or_default()
    };
    format!(
        "<?xml version='1.0' encoding='UTF-8'?>\r\n<root>\r\n\t<session>\r\n\t\t<card_id>{}</card_id>\r\n\t\t<mac_addr>{}</mac_addr>\r\n\t\t<session_id>12345678901234567890123456789012</session_id>\r\n\t\t<expires>9999</expires>\r\n\t\t<player_id>1</player_id>\r\n\t</session>\r\n</root>\r\n",
        tag("card_id"),
        tag("mac_addr")
    )
}

/// Card server XML data types (Fighting EX Layer): a write is its read + 512, its `<data>`
/// fields are those of the read's answer, `/root/<element>`.
const CARD_XML_READS: [(u32, &str); 2] = [(259, "card"), (264, "card_bdata")];

/// A card server XML write (`<root><info>...</info><data>fields</data></root>`, the card id in
/// `<info>`): the read type it answers and that answer, `<root><element>fields</element></root>`
/// with the card id.
fn xml_card_write(card: &[u8], data_type: u32) -> Option<(u32, String)> {
    let (read_type, element) = CARD_XML_READS.iter().find(|(t, _)| *t + 512 == data_type)?;
    let text = String::from_utf8_lossy(card);
    let inner = |name: &str| {
        let (open, close) = (format!("<{name}>"), format!("</{name}>"));
        let start = text.find(&open)? + open.len();
        Some(text[start..start + text[start..].find(&close)?].to_string())
    };
    let card_id = inner("card_id").unwrap_or_default();
    let fields = inner("data")?.replace("<card_id/>", "");
    let fields = match fields.contains("<card_id>") {
        true => fields,
        false => format!("\r\n\t\t<card_id>{card_id}</card_id>{fields}"),
    };
    Some((
        *read_type,
        format!("<?xml version='1.0' encoding='UTF-8'?>\r\n<root>\r\n\t<{element}>{fields}</{element}>\r\n</root>\r\n"),
    ))
}

/// `WAL_NESICA_PLAYER_NAME`: the player name of a saved card's answer (`<player_name>`; the
/// game registers a new card as NO-NAME and has no name entry: the card server's web site set it).
fn with_player_name(card: Vec<u8>) -> Vec<u8> {
    let Some(name) = std::env::var("WAL_NESICA_PLAYER_NAME").ok().filter(|n| !n.is_empty()) else {
        return card;
    };
    let text = String::from_utf8_lossy(&card);
    let (open, close) = ("<player_name>", "</player_name>");
    let Some(start) = text.find(open).map(|i| i + open.len()) else {
        return card;
    };
    let Some(end) = text[start..].find(close).map(|e| start + e) else {
        return card;
    };
    let name = name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("{}{name}{}", &text[..start], &text[end..]).into_bytes()
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
            // `WAL_NESICA_NESYS_OFFLINE=1`: the service runs but the network is down (NW_ERROR,
            // code of a real service capture): the game plays offline (no online downloads),
            // instead of waiting for the service as with no service at all
            if std::env::var("WAL_NESICA_NESYS_OFFLINE").is_ok_and(|v| v == "1") {
                send(pipe, NW_ERROR_NOTICE, &Out::default().u32(0x2000_0011).0);
                return;
            }
            // the network is up
            send(pipe, NWRECOVER_NOTICE, &[]);
            thread::sleep(Duration::from_millis(30));
            // the server's `key=value,...` list (`WAL_NESICA_NESYS_HOST`): KOF XIV also reads
            // `ms` and `ss` (NULL dereference without them)
            let server = std::env::var("WAL_NESICA_NESYS_HOST").unwrap_or_else(|_| "card_id=7020392010281502".into());
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
            log!("nesys: server {server}");
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
            if std::env::var("WAL_NESYS_HTTP_DUMP").is_ok_and(|v| v == "1") {
                let param = &data[24.min(data.len())..];
                log!("nesys: card select type {} param {:?}", u32_at(data, 16), String::from_utf8_lossy(param));
            }
            let mut card = std::fs::read(&file).unwrap_or_default();
            if card.is_empty() && u32_at(data, 16) == CARD_GET_SESSION {
                card = session_xml(&data[24.min(data.len())..]).into_bytes();
            }
            if u32_at(data, 16) == CARD_XML_READS[0].0 && !card.is_empty() {
                card = with_player_name(card);
            }
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
            if std::env::var("WAL_NESYS_HTTP_DUMP").is_ok_and(|v| v == "1") {
                log!("nesys: card update type {} data {:?}", u32_at(data, 24), String::from_utf8_lossy(card));
            }
            // card server XML writes: saved as the answer of their read
            let (file, saved) = match xml_card_write(card, u32_at(data, 24)) {
                Some((read_type, answer)) => (card_file(data, read_type), answer.into_bytes()),
                None => (file, card.to_vec()),
            };
            if let Err(e) = std::fs::write(&file, &saved) {
                log!("nesys: cannot save {file}: {e}");
            }
            send(pipe, CARD_UPDATE_REPLY, &card_status(100, card));
        }
        // Fighting EX Layer, at a card insertion: { char cardId[16]; u32 }. The reply is a card
        // status as the other card replies (the client parses 0x111-0x119 alike); the game only
        // checks its status (1), without it it waits on the card screen until its timeout.
        CARD_REISSUE_TEST_REQUEST => send(pipe, CARD_REISSUE_TEST_REPLY, &card_status(100, &[])),
        CARD_REISSUE_REQUEST => send(pipe, CARD_REISSUE_REPLY, &card_status(100, &[])),
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
        HTTPACCESS_GET_REQUEST | HTTPACCESS_POST_REQUEST => http_access(pipe, data),
        _ => log!("nesys: unhandled command {command:#x}"),
    }
}
