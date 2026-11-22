//! Cabinets linked across PCs (`WAL_LINK=1`, off by default: the game stays alone, as before,
//! no hook).
//!
//! The cabinets find each other on their own ([`start`]): starting, a cabinet listens (3 s) for a
//! master announcing this game on the network (a broadcast to UDP port [`ANNOUNCE_PORT`] every
//! half second, with the game's id: another game's master is not taken). None heard: it is the
//! master, cabinet 1, and announces itself ([`serve_master`]). One heard: it joins it (UDP port
//! [`PORT`]) and gets its number from it, the first free one from 2 ([`serve_cabinet`]); it tells
//! the master its number every second and the master answers each cabinet with the table of the
//! cabinets it heard (their addresses as it saw them). `WAL_CABINET` gives the cabinet its
//! number (`auto`, the default: as found; 1: the master without listening; 2..: joins as that
//! one), `WAL_LINK_MASTER` the master's address instead of its announcements (a network that does
//! not pass broadcasts). A cabinet's address is that of the card the route to the master takes;
//! `WAL_LINK_ADDRESS` names another one (loopback addresses: instances of a game on one PC, a
//! test).
//!
//! Linked, the game's sockets (its import table's `ws2_32` / `wsock32`, by name or by number, and
//! `iphlpapi`, [`hook_module`]) make the cabinets' PCs its network:
//!
//! * a broadcast (`255.255.255.255`, `x.x.x.255`) or a multicast datagram goes to each cabinet's
//!   PC, its own too (a broadcast comes back to its sender), as one datagram each: the PCs'
//!   firewalls see plain datagrams, a PC on another network is reached too;
//! * the PC's cards (`SIO_GET_INTERFACE_LIST`, `GetAdaptersInfo`, `GetAdaptersAddresses`) are the
//!   card of its address (and the loopback), and its own name (`gethostbyname`) is that address
//!   alone: a VPN's or a virtual card's address would be given to the other cabinets otherwise;
//! * a game whose cabinets have addresses of their own ([`virtual_net`]: cabinet k at
//!   `net.(first + k - 1)`) has them made the cabinets' PCs both ways (`net.255` a broadcast).
//!
//! With loopback addresses each instance binds its own (its binds to every address made its own).

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::NetworkManagement::IpHelper::{GetIpAddrTable, IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_INFO, MIB_IPADDRTABLE};

use crate::{iat, log};

pub const LINK_ENV: &str = "WAL_LINK";
pub const CABINET_ENV: &str = "WAL_CABINET";
pub const MASTER_ENV: &str = "WAL_LINK_MASTER";
pub const ADDRESS_ENV: &str = "WAL_LINK_ADDRESS";
/// The game's id (`systemprofiles/<system>/<id>.yaml`), set by the launcher.
pub const GAME_ENV: &str = wal_protocol::env::GAME_ID;
/// The cabinets' table, on the master's PC.
pub const PORT: u16 = 33790;
/// The master's announcements.
pub const ANNOUNCE_PORT: u16 = 33791;
const MAX: usize = 8;
const MAGIC: &str = "WALLINK1";
/// How long a starting cabinet listens for a master.
const LISTEN: Duration = Duration::from_secs(3);

struct Link {
    /// This cabinet, 1.. (1: the master).
    cabinet: usize,
    /// The game's id: the masters of other games are not taken.
    game: String,
    /// `WAL_LINK_ADDRESS`: this PC's address, not the route's.
    address: Option<[u8; 4]>,
    /// The master's address, once known (the master: its own).
    master: Mutex<Option<[u8; 4]>>,
    /// The cabinets' PCs by number (index 0: cabinet 1, the master), this one's always there.
    table: Mutex<[Option<[u8; 4]>; MAX]>,
}

static LINK: OnceLock<Option<Link>> = OnceLock::new();
/// The game's own addresses ([`virtual_net`]): the first three bytes and cabinet 1's host
/// (`0`: none).
static VIRTUAL: AtomicU32 = AtomicU32::new(0);
static NAME_IS_ADDRESS: AtomicBool = AtomicBool::new(false);
/// The first table came from the master.
static READY: AtomicBool = AtomicBool::new(false);

fn env(name: &str) -> String {
    std::env::var(name).map(|v| v.trim().to_string()).unwrap_or_default()
}

fn ipv4(text: &str) -> Option<[u8; 4]> {
    text.parse::<Ipv4Addr>().ok().filter(|a| !a.is_unspecified() && !a.is_broadcast()).map(|a| a.octets())
}

/// The address this PC reaches `to` from (the route's card); `None` without a route.
fn route_to(to: [u8; 4]) -> Option<[u8; 4]> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((Ipv4Addr::from(to), PORT)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(a) if !a.is_unspecified() => Some(a.octets()),
        _ => None,
    }
}

/// This PC's address without a master known: the default route's card (nothing is sent), else
/// the first card that is not the loopback.
fn default_address() -> Option<[u8; 4]> {
    if let Some(a) = route_to([192, 0, 2, 1]).filter(|a| a[0] != 127) {
        return Some(a);
    }
    let mut size = 0u32;
    unsafe { GetIpAddrTable(std::ptr::null_mut(), &mut size, 0) };
    let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
    let t = buf.as_mut_ptr() as *mut MIB_IPADDRTABLE;
    if size == 0 || unsafe { GetIpAddrTable(t, &mut size, 0) } != 0 {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*t).table.as_ptr(), (*t).dwNumEntries as usize) };
    rows.iter().map(|r| r.dwAddr.to_ne_bytes()).find(|a| a[0] != 127 && *a != [0; 4])
}

/// The settings: linked, the cabinet's number asked (`None`: auto), the master's and this PC's
/// addresses when set.
struct Settings {
    cabinet: Option<usize>,
    master: Option<[u8; 4]>,
    address: Option<[u8; 4]>,
}

fn settings(enabled: &str, cabinet: &str, master: &str, address: &str) -> Option<Settings> {
    if !matches!(enabled.to_ascii_lowercase().as_str(), "1" | "true" | "on") {
        return None;
    }
    let cabinet = match cabinet.parse::<usize>() {
        Ok(k @ 1..=MAX) => Some(k),
        _ if cabinet.is_empty() || cabinet.eq_ignore_ascii_case("auto") => None,
        _ => {
            log!("link: {CABINET_ENV} {cabinet:?} is not auto or 1 to {MAX}: auto");
            None
        }
    };
    let master = ipv4(master).or_else(|| {
        if !master.is_empty() {
            log!("link: {MASTER_ENV} {master:?} is not an address: the master is found on the network");
        }
        None
    });
    Some(Settings { cabinet, master, address: ipv4(address) })
}

/// The game's id: the launcher's, else the executable's name.
fn game_id() -> String {
    let id = env(GAME_ENV);
    if !id.is_empty() {
        return id;
    }
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).unwrap_or_default()
}

/// A UDP socket on `port` of every address that other sockets may share (several cabinets of
/// one PC listening for the master: each gets its announcements).
fn shared_udp(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, INVALID_SOCKET, IPPROTO_UDP, SO_REUSEADDR, SOCK_DGRAM, SOCKADDR, SOCKADDR_IN, SOL_SOCKET, WSADATA, WSAStartup, bind, closesocket, setsockopt, socket};
    let mut data: WSADATA = unsafe { std::mem::zeroed() };
    unsafe { WSAStartup(0x202, &mut data) };
    let s = unsafe { socket(AF_INET as i32, SOCK_DGRAM, IPPROTO_UDP) };
    if s == INVALID_SOCKET {
        return Err(std::io::Error::last_os_error());
    }
    let on: i32 = 1;
    unsafe { setsockopt(s, SOL_SOCKET, SO_REUSEADDR, (&on as *const i32).cast(), 4) };
    let mut a: SOCKADDR_IN = unsafe { std::mem::zeroed() };
    a.sin_family = AF_INET;
    a.sin_port = port.to_be();
    if unsafe { bind(s, (&a as *const SOCKADDR_IN).cast::<SOCKADDR>(), size_of::<SOCKADDR_IN>() as i32) } != 0 {
        let e = std::io::Error::last_os_error();
        unsafe { closesocket(s) };
        return Err(e);
    }
    Ok(unsafe { UdpSocket::from_raw_socket(s as _) })
}

/// A master's announcement of `game` heard on `socket`: its address.
fn announcement(socket: &UdpSocket, game: &str, buf: &mut [u8]) -> Option<[u8; 4]> {
    let Ok((n, SocketAddr::V4(from))) = socket.recv_from(buf) else { return None };
    let text = String::from_utf8_lossy(&buf[..n]);
    let words: Vec<&str> = text.split_whitespace().collect();
    let [MAGIC, "A", g, shown] = words[..] else { return None };
    // its address in the announcement when it was set (a loopback test), else the datagram's
    (g == game).then(|| ipv4(shown).unwrap_or(from.ip().octets()))
}

/// Listens for a master of `game` (`for_`, at most).
fn find_master(game: &str, for_: Duration) -> Option<[u8; 4]> {
    let socket = match shared_udp(ANNOUNCE_PORT) {
        Ok(s) => s,
        Err(e) => {
            log!("link: the masters' announcements on UDP {ANNOUNCE_PORT}: {e}");
            return None;
        }
    };
    let start = Instant::now();
    let mut buf = [0u8; 512];
    while start.elapsed() < for_ {
        let _ = socket.set_read_timeout(Some((for_ - start.elapsed()).max(Duration::from_millis(10))));
        if let Some(m) = announcement(&socket, game, &mut buf) {
            return Some(m);
        }
    }
    None
}

fn bind_udp(address: Option<[u8; 4]>, port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind((Ipv4Addr::from(address.unwrap_or([0; 4])), port))
}

/// Joins the master at `m` as cabinet `want` (`0`: the first free number): the number it gave.
fn join(socket: &UdpSocket, m: [u8; 4], game: &str, want: usize, for_: Duration) -> Option<usize> {
    let start = Instant::now();
    let mut buf = [0u8; 512];
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    while start.elapsed() < for_ {
        let _ = socket.send_to(format!("{MAGIC} J {game} {want}").as_bytes(), (Ipv4Addr::from(m), PORT));
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
            if let Some((k, _)) = parse_table(&String::from_utf8_lossy(&buf[..n])) {
                return Some(k);
            }
        }
    }
    None
}

/// The master's table: the number it gave this cabinet and the others' addresses.
fn parse_table(text: &str) -> Option<(usize, Vec<(usize, [u8; 4])>)> {
    let mut words = text.split_whitespace();
    if words.next() != Some(MAGIC) || words.next() != Some("T") {
        return None;
    }
    let you = words.next()?.strip_prefix("you=")?.parse::<usize>().ok().filter(|k| (2..=MAX).contains(k))?;
    let others = words.filter_map(|w| {
        let (k, a) = w.split_once('=')?;
        Some((k.parse::<usize>().ok().filter(|k| (2..=MAX).contains(k))?, ipv4(a)?))
    });
    Some((you, others.collect()))
}

/// The cabinet's role found ([`find_master`], [`join`]): `None` alone.
fn start() -> Option<Link> {
    let s = settings(&env(LINK_ENV), &env(CABINET_ENV), &env(MASTER_ENV), &env(ADDRESS_ENV))?;
    let game = game_id();
    // the master found, unless this one is it (cabinet 1) or its address is set
    let master = match (s.cabinet, s.master) {
        (Some(1), m) => m,
        (_, Some(m)) => Some(m),
        _ => {
            log!("link: listening for a master of {game:?} (UDP {ANNOUNCE_PORT}, {} s)", LISTEN.as_secs());
            let found = find_master(&game, LISTEN);
            if found.is_none() && s.cabinet.is_none() {
                log!("link: no master heard: this cabinet is the master");
            }
            found
        }
    };
    let is_master = s.cabinet == Some(1) || (s.cabinet.is_none() && master.is_none());
    let own = s
        .address
        .or(if is_master { master } else { master.and_then(route_to) })
        .or_else(default_address)
        .unwrap_or([127, 0, 0, 1]);
    let mut cabinet = if is_master { 1 } else { s.cabinet.unwrap_or(0) };
    if let (false, Some(m)) = (is_master, master) {
        // its number from the master (asked for when set)
        let socket = bind_udp(s.address, 0).ok()?;
        match join(&socket, m, &game, cabinet, Duration::from_secs(3)) {
            Some(k) => cabinet = k,
            None => log!("link: the master {} did not answer (UDP {PORT} let in on its PC?)", dotted(m)),
        }
    }
    if cabinet == 0 {
        // no master: joins the first one found later, as the number it would get first
        cabinet = 2;
        log!("link: no master yet: cabinet 2 until one is found (its number is the master's choice then)");
    }
    let mut table = [None; MAX];
    table[0] = if is_master { Some(own) } else { master };
    table[cabinet - 1] = Some(own);
    log!(
        "link: cabinet {cabinet}{}, this PC {}, game {game:?}{}",
        if is_master { " (the master)" } else { "" },
        dotted(own),
        master.filter(|_| !is_master).map_or(String::new(), |m| format!(", the master {}", dotted(m)))
    );
    Some(Link { cabinet, game, address: s.address, master: Mutex::new(if is_master { Some(own) } else { master }), table: Mutex::new(table) })
}

fn link() -> Option<&'static Link> {
    LINK.get_or_init(start).as_ref()
}

/// The cabinets' PCs known by now, by number (index 0: cabinet 1).
fn table() -> [Option<[u8; 4]>; MAX] {
    link().map_or([None; MAX], |l| *l.table.lock().unwrap())
}

/// The cabinets' count (the highest number known by now), `0` alone.
pub fn count() -> usize {
    if !linked() {
        return 0;
    }
    table().iter().rposition(Option::is_some).map_or(0, |i| i + 1)
}

/// This cabinet's number, 1.. (`1` alone or the master).
pub fn cabinet() -> usize {
    link().map_or(1, |l| l.cabinet)
}

/// Linked (`WAL_LINK=1`).
pub fn linked() -> bool {
    link().is_some()
}

/// The PC of cabinet `k` (1..), once known.
pub fn address(k: usize) -> Option<[u8; 4]> {
    *table().get(k.checked_sub(1)?)?
}

/// This PC's address on the cabinets' network.
pub fn own() -> Option<[u8; 4]> {
    address(cabinet()).filter(|_| linked())
}

pub fn dotted(a: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

/// The cabinet a PC's address is, 1.. (`None`: not one of them).
pub fn pc_of(a: [u8; 4]) -> Option<usize> {
    table().iter().position(|&p| p == Some(a)).map(|i| i + 1)
}

/// The master: its announcements, the cabinets joining given their numbers (the one asked for
/// when free, else the first free from 2; a PC joining again keeps its own), the table sent back.
fn serve_master(l: &'static Link) {
    let socket = match bind_udp(l.address, PORT) {
        Ok(s) => s,
        Err(e) => return log!("link: the cabinets' table on UDP {PORT}: {e} (another master on this PC?)"),
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let announcer = UdpSocket::bind("0.0.0.0:0").ok();
    if let Some(a) = &announcer {
        let _ = a.set_broadcast(true);
    }
    let announcement = format!("{MAGIC} A {} {}", l.game, l.address.map_or("-".to_string(), dotted));
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut buf = [0u8; 512];
    loop {
        if last.elapsed() >= Duration::from_millis(500) {
            if let Some(a) = &announcer {
                for to in [[255, 255, 255, 255], [127, 255, 255, 255]] {
                    let _ = a.send_to(announcement.as_bytes(), (Ipv4Addr::from(to), ANNOUNCE_PORT));
                }
            }
            last = Instant::now();
        }
        let Ok((n, SocketAddr::V4(from))) = socket.recv_from(&mut buf) else { continue };
        let text = String::from_utf8_lossy(&buf[..n]);
        let words: Vec<&str> = text.split_whitespace().collect();
        // "WALLINK1 J <game> <number asked, 0: any>"
        let [MAGIC, "J", game, want] = words[..] else { continue };
        if game != l.game {
            continue;
        }
        let want = want.parse::<usize>().unwrap_or(0);
        let a = from.ip().octets();
        let reply = {
            let mut t = l.table.lock().unwrap();
            let k = match t.iter().skip(1).position(|&e| e == Some(a)).map(|i| i + 2) {
                Some(k) if want == 0 || want == k => Some(k),
                _ if (2..=MAX).contains(&want) => Some(want),
                _ => t.iter().skip(1).position(Option::is_none).map(|i| i + 2),
            };
            let Some(k) = k else {
                log!("link: {} cabinets already: {} not taken", MAX, dotted(a));
                continue;
            };
            if t[k - 1] != Some(a) {
                log!("link: cabinet {k} joined from {}", dotted(a));
                // the PC's former number, freed
                for (i, e) in t.iter_mut().enumerate().skip(1) {
                    if i != k - 1 && *e == Some(a) {
                        *e = None;
                    }
                }
                t[k - 1] = Some(a);
            }
            let mut reply = format!("{MAGIC} T you={k}");
            for (i, e) in t.iter().enumerate().skip(1) {
                if let Some(a) = e {
                    reply.push_str(&format!(" {}={}", i + 1, dotted(*a)));
                }
            }
            reply
        };
        let _ = socket.send_to(reply.as_bytes(), from);
    }
}

/// Another cabinet: the master found (its announcements, when not known yet), told this cabinet's
/// number every second, its tables kept.
fn serve_cabinet(l: &'static Link) {
    let socket = match bind_udp(l.address, 0) {
        Ok(s) => s,
        Err(e) => return log!("link: the cabinets' table: {e}"),
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let mut listener: Option<UdpSocket> = None;
    let started = Instant::now();
    let mut warned = false;
    let mut last = Instant::now() - Duration::from_secs(2);
    let mut buf = [0u8; 512];
    loop {
        let master = *l.master.lock().unwrap();
        if !warned && !READY.load(Ordering::Relaxed) && started.elapsed() >= Duration::from_secs(10) {
            warned = true;
            log!("link: no answer from a master yet (its game started? UDP {PORT} and {ANNOUNCE_PORT} let in?): still looking");
        }
        let Some(m) = master else {
            if listener.is_none() {
                listener = shared_udp(ANNOUNCE_PORT).ok();
                if let Some(s) = &listener {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                }
            }
            match listener.as_ref().and_then(|s| announcement(s, &l.game, &mut buf)) {
                Some(m) => {
                    log!("link: the master cabinet found at {}", dotted(m));
                    *l.master.lock().unwrap() = Some(m);
                    let mut t = l.table.lock().unwrap();
                    t[0] = Some(m);
                    if l.address.is_none() {
                        if let Some(own) = route_to(m) {
                            t[l.cabinet - 1] = Some(own);
                        }
                    }
                    listener = None;
                }
                None if listener.is_none() => std::thread::sleep(Duration::from_secs(1)),
                None => {}
            }
            continue;
        };
        if last.elapsed() >= Duration::from_secs(1) {
            let _ = socket.send_to(format!("{MAGIC} J {} {}", l.game, l.cabinet).as_bytes(), (Ipv4Addr::from(m), PORT));
            last = Instant::now();
        }
        let Ok((n, SocketAddr::V4(from))) = socket.recv_from(&mut buf) else { continue };
        let Some((you, others)) = (from.ip().octets() == m).then(|| parse_table(&String::from_utf8_lossy(&buf[..n]))).flatten() else { continue };
        if you != l.cabinet {
            log!("link: the master gave cabinet {you} (this one is {}: another PC took it first?)", l.cabinet);
        }
        let mut t = l.table.lock().unwrap();
        let mut fresh = [None; MAX];
        fresh[0] = Some(m);
        fresh[l.cabinet - 1] = t[l.cabinet - 1];
        for (k, a) in others {
            if k != l.cabinet {
                fresh[k - 1] = Some(a);
            }
        }
        if *t != fresh {
            let list: Vec<String> = fresh.iter().enumerate().filter_map(|(i, e)| e.map(|a| format!("{}={}", i + 1, dotted(a)))).collect();
            log!("link: the cabinets: {}", list.join(" "));
            *t = fresh;
        }
        READY.store(true, Ordering::Relaxed);
    }
}

/// The game gives cabinet k the address `net.(first + k - 1)` (its own network, not the PCs'):
/// those addresses are the listed PCs, `net.255` a broadcast.
pub fn virtual_net(net: [u8; 3], first: u8) {
    if !linked() || first == 0 {
        return;
    }
    VIRTUAL.store(u32::from_be_bytes([net[0], net[1], net[2], first]), Ordering::Relaxed);
    log!("link: the game's cabinets at {}.{}.{}.{}..: the cabinets' PCs", net[0], net[1], net[2], first);
}

/// The game tells the others its PC's name to reach it: its `gethostname` is its listed address.
pub fn name_is_address() {
    NAME_IS_ADDRESS.store(true, Ordering::Relaxed);
}

fn virtual_parts() -> Option<([u8; 3], u8)> {
    let v = VIRTUAL.load(Ordering::Relaxed).to_be_bytes();
    (v[3] != 0).then_some(([v[0], v[1], v[2]], v[3]))
}

/// A game's address made a real one: its cabinet k the PC k, its network's broadcast the broadcast.
pub fn to_real(a: [u8; 4]) -> [u8; 4] {
    let Some((net, first)) = virtual_parts() else { return a };
    if a[..3] != net {
        return a;
    }
    if a[3] == 255 {
        return [255; 4];
    }
    a[3].checked_sub(first).and_then(|k| address(k as usize + 1)).unwrap_or(a)
}

/// A PC's address as the game knows it (its own network's, [`virtual_net`]).
pub fn to_game(a: [u8; 4]) -> [u8; 4] {
    match (virtual_parts(), pc_of(a)) {
        (Some((net, first)), Some(k)) => [net[0], net[1], net[2], first + k as u8 - 1],
        _ => a,
    }
}

fn broadcast(a: [u8; 4]) -> bool {
    a[3] == 255
}

fn multicast(a: [u8; 4]) -> bool {
    (224..240).contains(&a[0])
}

/// The listed addresses are loopback ones: instances of the game on one PC.
fn on_loopback() -> bool {
    own().is_some_and(|a| a[0] == 127)
}

/// The address a payload's server for the game listens on: 127.0.0.1, or (instances on one PC)
/// this cabinet's loopback address, where the game's connections to 127.0.0.1 go (each
/// instance's game reaches its own payload).
pub fn local_server() -> [u8; 4] {
    match own() {
        Some(a) if a[0] == 127 => a,
        _ => [127, 0, 0, 1],
    }
}

/// An IPv4 `sockaddr_in`'s address (`None`: another family).
unsafe fn inet(addr: *const u8, len: i32) -> Option<[u8; 4]> {
    if addr.is_null() || len < 16 || unsafe { *addr } != 2 {
        return None;
    }
    Some(unsafe { [*addr.add(4), *addr.add(5), *addr.add(6), *addr.add(7)] })
}

/// The game's ports under 1024 (Showdown's 1000 / 1001) are 40000 + port on the network: a
/// user's process cannot bind them on Linux (Wine's under it); every linked cabinet moves them
/// the same way, the game sees its own.
const PORT_SHIFT: u16 = 40000;

fn port_out(p: u16) -> u16 {
    if (1..1024).contains(&p) { p + PORT_SHIFT } else { p }
}

fn port_in(p: u16) -> u16 {
    if (PORT_SHIFT + 1..PORT_SHIFT + 1024).contains(&p) { p - PORT_SHIFT } else { p }
}

/// `addr` (a `sockaddr_in`) to `a`: its port moved ([`port_out`]) when `a` is a cabinet's PC.
fn with_address(addr: *const u8, a: [u8; 4]) -> [u8; 16] {
    let mut m = [0u8; 16];
    unsafe { std::ptr::copy_nonoverlapping(addr, m.as_mut_ptr(), 16) };
    m[4..8].copy_from_slice(&a);
    if pc_of(a).is_some() {
        let p = port_out(u16::from_be_bytes([m[2], m[3]]));
        m[2..4].copy_from_slice(&p.to_be_bytes());
    }
    m
}

/// The first datagrams with each PC, each way, in the log, and every 10 s the datagrams counted
/// with each.
fn traffic(what: &str, pc: usize, addr: &[u8], n: i32, data: *const u8) {
    static LOGGED: [[AtomicUsize; 2]; MAX] = [const { [const { AtomicUsize::new(0) }; 2] }; MAX];
    static COUNTS: [[AtomicUsize; 2]; MAX] = [const { [const { AtomicUsize::new(0) }; 2] }; MAX];
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let way = what.starts_with('s') as usize;
    if pc == 0 || pc > MAX {
        return;
    }
    COUNTS[pc - 1][way].fetch_add(1, Ordering::Relaxed);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
            let counts: Vec<String> = (0..MAX)
                .map(|i| (i, COUNTS[i][1].swap(0, Ordering::Relaxed), COUNTS[i][0].swap(0, Ordering::Relaxed)))
                .filter(|&(_, s, r)| s + r > 0)
                .map(|(i, s, r)| format!("cabinet {}: {s} sent, {r} received", i + 1))
                .collect();
            if last.is_some() {
                log!("link: datagrams in 10 s: {}", counts.join("; "));
            }
            *last = Some(Instant::now());
        }
    }
    if LOGGED[pc - 1][way].fetch_add(1, Ordering::Relaxed) >= 12 {
        return;
    }
    let head = if data.is_null() || n <= 0 {
        String::new()
    } else {
        let b = unsafe { std::slice::from_raw_parts(data, n.min(16) as usize) };
        b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
    };
    log!("link: {what} cabinet {pc} ({}:{}): {n} bytes {head}", dotted([addr[4], addr[5], addr[6], addr[7]]), u16::from_be_bytes([addr[2], addr[3]]));
}

static BIND: AtomicUsize = AtomicUsize::new(0);
static CONNECT: AtomicUsize = AtomicUsize::new(0);
static SEND_TO: AtomicUsize = AtomicUsize::new(0);
static RECV_FROM: AtomicUsize = AtomicUsize::new(0);
static ACCEPT: AtomicUsize = AtomicUsize::new(0);
static GET_PEER_NAME: AtomicUsize = AtomicUsize::new(0);
static GET_SOCK_NAME: AtomicUsize = AtomicUsize::new(0);
static GET_HOST_BY_NAME: AtomicUsize = AtomicUsize::new(0);
static GET_HOST_NAME: AtomicUsize = AtomicUsize::new(0);
static WSA_IOCTL: AtomicUsize = AtomicUsize::new(0);
static WSA_SEND_TO: AtomicUsize = AtomicUsize::new(0);
static GET_ADAPTERS_INFO: AtomicUsize = AtomicUsize::new(0);
static GET_ADAPTERS_ADDRESSES: AtomicUsize = AtomicUsize::new(0);

type BindFn = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
type SendToFn = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
type RecvFromFn = unsafe extern "system" fn(usize, *mut u8, i32, i32, *mut u8, *mut i32) -> i32;
type AcceptFn = unsafe extern "system" fn(usize, *mut u8, *mut i32) -> usize;
type PeerFn = unsafe extern "system" fn(usize, *mut u8, *mut i32) -> i32;
type HostByNameFn = unsafe extern "system" fn(*const u8) -> *mut HostEnt;
type HostNameFn = unsafe extern "system" fn(*mut u8, i32) -> i32;
type IoctlFn = unsafe extern "system" fn(usize, u32, *const u8, u32, *mut u8, u32, *mut u32, *mut u8, usize) -> i32;
type WsaSendToFn = unsafe extern "system" fn(usize, *const WsaBuf, u32, *mut u32, u32, *const u8, i32, *mut u8, usize) -> i32;
type AdaptersInfoFn = unsafe extern "system" fn(*mut IP_ADAPTER_INFO, *mut u32) -> u32;
type AdaptersAddressesFn = unsafe extern "system" fn(u32, u32, *mut std::ffi::c_void, *mut IP_ADAPTER_ADDRESSES_LH, *mut u32) -> u32;

fn real<F: Copy>(slot: &AtomicUsize) -> F {
    let p = slot.load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

/// A bind on the game's address of this cabinet, or (instances on one PC) on every address: on
/// this PC's listed address.
unsafe extern "system" fn bind(s: usize, addr: *const u8, len: i32) -> i32 {
    let f: BindFn = real(&BIND);
    let Some(a) = (unsafe { inet(addr, len) }) else { return unsafe { f(s, addr, len) } };
    let own = own().unwrap_or(a);
    // instances on one PC: another cabinet's loopback address is not this PC's (a game telling
    // whether an address is its own by binding it: Ford Racing's race host)
    if on_loopback() && a != own && pc_of(to_real(a)).is_some() {
        log!("link: bind {}: another cabinet's address, refused", dotted(a));
        unsafe { windows_sys::Win32::Networking::WinSock::WSASetLastError(windows_sys::Win32::Networking::WinSock::WSAEADDRNOTAVAIL) };
        return -1;
    }
    let mine = to_real(a) == own || (a == [0; 4] && on_loopback());
    let mut m = with_address(addr, if mine { own } else { a });
    let p = port_out(u16::from_be_bytes([m[2], m[3]]));
    m[2..4].copy_from_slice(&p.to_be_bytes());
    let r = unsafe { f(s, m.as_ptr(), 16) };
    log!("link: bind {}:{} -> {r}{}", dotted(a), u16::from_be_bytes([m[2], m[3]]), if mine && a != own { format!(" (on {})", dotted(own)) } else { String::new() });
    r
}

unsafe extern "system" fn connect(s: usize, addr: *const u8, len: i32) -> i32 {
    let f: BindFn = real(&CONNECT);
    bind_unbound(s);
    match unsafe { inet(addr, len) } {
        Some(a) if to_real(a) != a => {
            let m = with_address(addr, to_real(a));
            log!("link: connect {}:{} (cabinet {})", dotted(to_real(a)), u16::from_be_bytes([m[2], m[3]]), pc_of(to_real(a)).unwrap_or(0));
            unsafe { f(s, m.as_ptr(), 16) }
        }
        // instances on one PC: 127.0.0.1 is this cabinet's loopback address ([`local_server`])
        Some([127, 0, 0, 1]) if on_loopback() => {
            let m = with_address(addr, local_server());
            unsafe { f(s, m.as_ptr(), 16) }
        }
        Some(a) => match pc_of(a) {
            Some(pc) => {
                let m = with_address(addr, a);
                log!("link: connect {}:{} (cabinet {pc})", dotted(a), u16::from_be_bytes([m[2], m[3]]));
                unsafe { f(s, m.as_ptr(), 16) }
            }
            None => unsafe { f(s, addr, len) },
        },
        None => unsafe { f(s, addr, len) },
    }
}

/// Sends a datagram to `a`: to each PC for a broadcast or a multicast group. The length sent
/// (the last error of a failed one).
fn send(a: [u8; 4], addr: *const u8, n: i32, data: *const u8, one: &dyn Fn(&[u8; 16]) -> i32) -> i32 {
    let a = to_real(a);
    if broadcast(a) || multicast(a) {
        let mut r = n;
        for (i, pc) in table().iter().enumerate() {
            let Some(pc) = pc else { continue };
            let m = with_address(addr, *pc);
            let sent = one(&m);
            if sent < 0 {
                r = sent;
            }
            traffic(if multicast(a) { "sent (multicast) to" } else { "sent (broadcast) to" }, i + 1, &m, sent, data);
        }
        return r;
    }
    let m = with_address(addr, a);
    let r = one(&m);
    match pc_of(a) {
        Some(pc) => traffic("sent to", pc, &m, r, data),
        None => {
            static LOGGED: AtomicUsize = AtomicUsize::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
                log!("link: sent to {}:{} (not a cabinet): {n} bytes", dotted(a), u16::from_be_bytes([m[2], m[3]]));
            }
        }
    }
    r
}

/// Instances on one PC: a socket the game sends from or connects without binding it is bound to
/// this cabinet's address first (the system would send from 127.0.0.1: Ford Racing's race search, its
/// host answering there).
fn bind_unbound(s: usize) {
    use windows_sys::Win32::Networking::WinSock::{SOCKADDR, SOCKADDR_IN, bind, getsockname};
    let Some(own) = own().filter(|_| on_loopback()) else { return };
    let mut a: SOCKADDR_IN = unsafe { std::mem::zeroed() };
    let mut len = size_of::<SOCKADDR_IN>() as i32;
    if unsafe { getsockname(s, (&mut a as *mut SOCKADDR_IN).cast::<SOCKADDR>(), &mut len) } == 0 {
        return;
    }
    a = unsafe { std::mem::zeroed() };
    a.sin_family = 2;
    a.sin_addr.S_un.S_addr = u32::from_ne_bytes(own);
    unsafe { bind(s, (&a as *const SOCKADDR_IN).cast::<SOCKADDR>(), size_of::<SOCKADDR_IN>() as i32) };
}

unsafe extern "system" fn send_to(s: usize, buf: *const u8, n: i32, flags: i32, to: *const u8, to_len: i32) -> i32 {
    let f: SendToFn = real(&SEND_TO);
    bind_unbound(s);
    match unsafe { inet(to, to_len) } {
        Some(a) => send(a, to, n, buf, &|m| unsafe { f(s, buf, n, flags, m.as_ptr(), 16) }),
        None => unsafe { f(s, buf, n, flags, to, to_len) },
    }
}

#[repr(C)]
pub struct WsaBuf {
    len: u32,
    buf: *const u8,
}

/// The datagrams sent overlapped: a broadcast's copies to the other PCs are sent at once (the
/// game's completion is the last one's).
unsafe extern "system" fn wsa_send_to(
    s: usize,
    bufs: *const WsaBuf,
    count: u32,
    sent: *mut u32,
    flags: u32,
    to: *const u8,
    to_len: i32,
    ov: *mut u8,
    done: usize,
) -> i32 {
    let f: WsaSendToFn = real(&WSA_SEND_TO);
    bind_unbound(s);
    let Some(a) = (unsafe { inet(to, to_len) }) else { return unsafe { f(s, bufs, count, sent, flags, to, to_len, ov, done) } };
    let data = if count > 0 && !bufs.is_null() { unsafe { (*bufs).buf } } else { std::ptr::null() };
    let n = if data.is_null() { 0 } else { unsafe { (*bufs).len as i32 } };
    let real_a = to_real(a);
    if broadcast(real_a) || multicast(real_a) {
        let pcs: Vec<(usize, [u8; 4])> = table().iter().enumerate().filter_map(|(i, e)| e.map(|a| (i, a))).collect();
        let mut r = 0;
        for (j, &(i, pc)) in pcs.iter().enumerate() {
            let m = with_address(to, pc);
            // the game's overlapped structure for the last one alone
            let last = j + 1 == pcs.len();
            let mut mine = 0u32;
            r = unsafe {
                f(s, bufs, count, if last { sent } else { &mut mine }, flags, m.as_ptr(), 16, if last { ov } else { std::ptr::null_mut() }, if last { done } else { 0 })
            };
            traffic("sent (broadcast) to", i + 1, &m, n, data);
        }
        return r;
    }
    let m = with_address(to, real_a);
    if let Some(pc) = pc_of(real_a) {
        traffic("sent to", pc, &m, n, data);
    }
    unsafe { f(s, bufs, count, sent, flags, m.as_ptr(), 16, ov, done) }
}

/// A peer's address as the game knows it.
unsafe fn from_game(addr: *mut u8, len: *const i32) {
    if len.is_null() {
        return;
    }
    if let Some(a) = unsafe { inet(addr, *len) } {
        if pc_of(a).is_some() {
            let p = port_in(u16::from_be_bytes(unsafe { [*addr.add(2), *addr.add(3)] }));
            unsafe { std::ptr::copy_nonoverlapping(p.to_be_bytes().as_ptr(), addr.add(2), 2) };
        }
        let g = to_game(a);
        unsafe { std::ptr::copy_nonoverlapping(g.as_ptr(), addr.add(4), 4) };
    }
}

unsafe extern "system" fn recv_from(s: usize, buf: *mut u8, n: i32, flags: i32, from: *mut u8, from_len: *mut i32) -> i32 {
    let f: RecvFromFn = real(&RECV_FROM);
    let r = unsafe { f(s, buf, n, flags, from, from_len) };
    if r >= 0 && !from_len.is_null() {
        if let Some(a) = unsafe { inet(from, *from_len) } {
            if let Some(pc) = pc_of(a) {
                traffic("received from", pc, unsafe { std::slice::from_raw_parts(from, 16) }, r, buf);
            }
            unsafe { from_game(from, from_len) };
        }
    }
    r
}

unsafe extern "system" fn accept(s: usize, addr: *mut u8, len: *mut i32) -> usize {
    let f: AcceptFn = real(&ACCEPT);
    let r = unsafe { f(s, addr, len) };
    if r != usize::MAX && !len.is_null() {
        if let Some(pc) = unsafe { inet(addr, *len) }.and_then(pc_of) {
            log!("link: accepted cabinet {pc}");
        }
        unsafe { from_game(addr, len) };
    }
    r
}

/// The socket's own address: its port as the game bound it ([`port_out`]).
unsafe extern "system" fn get_sock_name(s: usize, addr: *mut u8, len: *mut i32) -> i32 {
    let f: PeerFn = real(&GET_SOCK_NAME);
    let r = unsafe { f(s, addr, len) };
    if r == 0 && !len.is_null() && unsafe { inet(addr, *len) }.is_some() {
        let p = port_in(u16::from_be_bytes(unsafe { [*addr.add(2), *addr.add(3)] }));
        unsafe { std::ptr::copy_nonoverlapping(p.to_be_bytes().as_ptr(), addr.add(2), 2) };
    }
    r
}

unsafe extern "system" fn get_peer_name(s: usize, addr: *mut u8, len: *mut i32) -> i32 {
    let f: PeerFn = real(&GET_PEER_NAME);
    let r = unsafe { f(s, addr, len) };
    if r == 0 {
        unsafe { from_game(addr, len) };
    }
    r
}

#[repr(C)]
pub struct HostEnt {
    name: *const u8,
    aliases: *const *const u8,
    addrtype: i16,
    length: i16,
    addr_list: *const *const u8,
}

struct Host {
    name: [u8; 256],
    address: [u8; 4],
    list: [*const u8; 2],
    aliases: [*const u8; 1],
    ent: HostEnt,
}

unsafe impl Send for Host {}

/// The PC's name (as Windows gives it, not [`name_is_address`]'s).
fn host_name() -> Vec<u8> {
    let f: HostNameFn = real(&GET_HOST_NAME);
    let mut name = [0u8; 256];
    let r = if GET_HOST_NAME.load(Ordering::Relaxed) != 0 {
        unsafe { f(name.as_mut_ptr(), 256) }
    } else {
        unsafe { windows_sys::Win32::Networking::WinSock::gethostname(name.as_mut_ptr(), 256) }
    };
    if r != 0 {
        return Vec::new();
    }
    name[..name.iter().position(|&b| b == 0).unwrap_or(0)].to_vec()
}

/// The PC's own name (or an empty one): its listed address alone.
unsafe extern "system" fn get_host_by_name(name: *const u8) -> *mut HostEnt {
    let f: HostByNameFn = real(&GET_HOST_BY_NAME);
    let asked = if name.is_null() { Vec::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().to_vec() };
    let me = host_name();
    let address_text = own().map(dotted).unwrap_or_default();
    let mine = asked.is_empty() || asked.eq_ignore_ascii_case(&me) || asked == address_text.as_bytes();
    let Some(own) = own().filter(|_| mine) else { return unsafe { f(name) } };
    static HOST: std::sync::Mutex<Option<Box<Host>>> = std::sync::Mutex::new(None);
    let mut guard = HOST.lock().unwrap();
    let h = guard.get_or_insert_with(|| {
        Box::new(Host {
            name: [0; 256],
            address: [0; 4],
            list: [std::ptr::null(); 2],
            aliases: [std::ptr::null()],
            ent: HostEnt { name: std::ptr::null(), aliases: std::ptr::null(), addrtype: 2, length: 4, addr_list: std::ptr::null() },
        })
    });
    let n = asked.len().min(255);
    let shown = if asked.is_empty() { &me[..me.len().min(255)] } else { &asked[..n] };
    h.name = [0; 256];
    h.name[..shown.len()].copy_from_slice(shown);
    h.address = own;
    h.list = [h.address.as_ptr(), std::ptr::null()];
    h.ent = HostEnt { name: h.name.as_ptr(), aliases: h.aliases.as_ptr(), addrtype: 2, length: 4, addr_list: h.list.as_ptr() };
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
        log!("link: gethostbyname({:?}): {} alone", String::from_utf8_lossy(&asked), dotted(own));
    }
    &mut h.ent
}

unsafe extern "system" fn get_host_name(name: *mut u8, size: i32) -> i32 {
    let f: HostNameFn = real(&GET_HOST_NAME);
    let Some(own) = own().filter(|_| NAME_IS_ADDRESS.load(Ordering::Relaxed) && !name.is_null()) else { return unsafe { f(name, size) } };
    let t = dotted(own);
    if size <= t.len() as i32 {
        return unsafe { f(name, size) };
    }
    unsafe {
        std::ptr::copy_nonoverlapping(t.as_ptr(), name, t.len());
        *name.add(t.len()) = 0;
    }
    0
}

/// The netmask of this PC's card of its listed address (`255.255.255.0` when none has it: the
/// loopback addresses).
fn own_mask() -> [u8; 4] {
    static MASK: OnceLock<[u8; 4]> = OnceLock::new();
    *MASK.get_or_init(|| {
        let own = own().unwrap_or_default();
        let mut size = 0u32;
        unsafe { GetIpAddrTable(std::ptr::null_mut(), &mut size, 0) };
        let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let t = buf.as_mut_ptr() as *mut MIB_IPADDRTABLE;
        if size > 0 && unsafe { GetIpAddrTable(t, &mut size, 0) } == 0 {
            let n = unsafe { (*t).dwNumEntries } as usize;
            let rows = unsafe { std::slice::from_raw_parts((*t).table.as_ptr(), n) };
            if let Some(row) = rows.iter().find(|r| r.dwAddr.to_ne_bytes() == own) {
                return row.dwMask.to_ne_bytes();
            }
        }
        [255, 255, 255, 0]
    })
}

const SIO_GET_INTERFACE_LIST: u32 = 0x4004_747F;
/// `INTERFACE_INFO`: its flags and three `sockaddr_gen` (24 bytes each).
const INTERFACE_INFO: usize = 4 + 3 * 24;

fn interface(flags: u32, a: [u8; 4], mask: [u8; 4]) -> [u8; INTERFACE_INFO] {
    let mut e = [0u8; INTERFACE_INFO];
    e[..4].copy_from_slice(&flags.to_le_bytes());
    let bcast = [a[0] | !mask[0], a[1] | !mask[1], a[2] | !mask[2], a[3] | !mask[3]];
    for (i, v) in [a, bcast, mask].into_iter().enumerate() {
        let at = 4 + i * 24;
        e[at] = 2;
        e[at + 4..at + 8].copy_from_slice(&v);
    }
    e
}

/// The PC's cards: the one of its listed address (made so when no card has it: the loopback
/// addresses) and the loopback.
unsafe extern "system" fn wsa_ioctl(s: usize, code: u32, input: *const u8, in_size: u32, out: *mut u8, out_size: u32, returned: *mut u32, ov: *mut u8, done: usize) -> i32 {
    let f: IoctlFn = real(&WSA_IOCTL);
    let r = unsafe { f(s, code, input, in_size, out, out_size, returned, ov, done) };
    let Some(own) = own() else { return r };
    if r != 0 || code != SIO_GET_INTERFACE_LIST || out.is_null() || returned.is_null() || (out_size as usize) < 2 * INTERFACE_INFO {
        return r;
    }
    // IFF_UP 1, IFF_BROADCAST 2, IFF_LOOPBACK 4, IFF_MULTICAST 0x10; the loopback left out with
    // loopback addresses (Ford Racing took 127.0.0.1 for the master's then)
    let list = [interface(0x13, own, own_mask()), interface(0x15, [127, 0, 0, 1], [255, 0, 0, 0])];
    let n = if on_loopback() { 1 } else { 2 };
    unsafe {
        std::ptr::copy_nonoverlapping(list.as_ptr().cast::<u8>(), out, n * INTERFACE_INFO);
        *returned = (n * INTERFACE_INFO) as u32;
    }
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
        log!("link: the PC's cards (SIO_GET_INTERFACE_LIST): {} and the loopback", dotted(own));
    }
    r
}

/// The PC's cards: the first one made the card of its listed address, alone.
unsafe extern "system" fn get_adapters_info(info: *mut IP_ADAPTER_INFO, size: *mut u32) -> u32 {
    let f: AdaptersInfoFn = real(&GET_ADAPTERS_INFO);
    let r = unsafe { f(info, size) };
    let Some(own) = own() else { return r };
    if r != 0 || info.is_null() {
        return r;
    }
    let text = |a: [u8; 4]| {
        let mut b = [0i8; 16];
        for (i, c) in dotted(a).bytes().enumerate() {
            b[i] = c as i8;
        }
        b
    };
    let wanted = text(own);
    let mut mine = std::ptr::null_mut();
    let mut a = info;
    while !a.is_null() {
        if unsafe { (*a).IpAddressList.IpAddress.String } == wanted {
            mine = a;
            break;
        }
        a = unsafe { (*a).Next };
    }
    unsafe {
        if !mine.is_null() && mine != info {
            std::ptr::copy_nonoverlapping(mine, info, 1);
        }
        if mine.is_null() {
            (*info).IpAddressList.IpAddress.String = wanted;
            (*info).IpAddressList.IpMask.String = text(own_mask());
            (*info).IpAddressList.Next = std::ptr::null_mut();
        }
        (*info).Next = std::ptr::null_mut();
    }
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
        log!("link: the PC's cards (GetAdaptersInfo): {} alone", dotted(own));
    }
    r
}

/// The PC's cards (`GetAdaptersAddresses`, Showdown's): the card of its address alone, first (a
/// copy of it in the list's first entry); none has it (the loopback addresses): the first card
/// that is not the loopback, its first IPv4 address made this cabinet's.
unsafe extern "system" fn get_adapters_addresses(family: u32, flags: u32, reserved: *mut std::ffi::c_void, list: *mut IP_ADAPTER_ADDRESSES_LH, size: *mut u32) -> u32 {
    let f: AdaptersAddressesFn = real(&GET_ADAPTERS_ADDRESSES);
    let r = unsafe { f(family, flags, reserved, list, size) };
    let Some(own) = own() else { return r };
    if r != 0 || list.is_null() {
        return r;
    }
    // IF_TYPE_SOFTWARE_LOOPBACK
    const LOOPBACK: u32 = 24;
    let ipv4 = |u: *mut windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_UNICAST_ADDRESS_LH| -> Option<*mut u8> {
        let a = unsafe { (*u).Address.lpSockaddr } as *mut u8;
        (!a.is_null() && unsafe { *a } == 2).then_some(a)
    };
    let (mut mine, mut first): (*mut IP_ADAPTER_ADDRESSES_LH, *mut IP_ADAPTER_ADDRESSES_LH) = (std::ptr::null_mut(), std::ptr::null_mut());
    let mut a = list;
    while !a.is_null() {
        let mut u = unsafe { (*a).FirstUnicastAddress };
        while !u.is_null() {
            if let Some(sa) = ipv4(u) {
                if unsafe { std::slice::from_raw_parts(sa.add(4), 4) } == own {
                    mine = a;
                }
                if first.is_null() && unsafe { (*a).IfType } != LOOPBACK {
                    first = a;
                }
            }
            u = unsafe { (*u).Next };
        }
        if !mine.is_null() {
            break;
        }
        a = unsafe { (*a).Next };
    }
    let chosen = if !mine.is_null() { mine } else { first };
    if chosen.is_null() {
        return r;
    }
    unsafe {
        if mine.is_null() {
            // the loopback addresses: its first IPv4 address made this cabinet's
            let mut u = (*chosen).FirstUnicastAddress;
            while !u.is_null() {
                if let Some(sa) = ipv4(u) {
                    std::ptr::copy_nonoverlapping(own.as_ptr(), sa.add(4), 4);
                    break;
                }
                u = (*u).Next;
            }
        }
        if chosen != list {
            std::ptr::copy_nonoverlapping(chosen, list, 1);
        }
        (*list).Next = std::ptr::null_mut();
    }
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
        log!("link: the PC's cards (GetAdaptersAddresses): {} alone", dotted(own));
    }
    r
}

/// Makes the sockets of the module at `base` (the game executable, or a DLL of its own with
/// sockets) the link's: nothing alone.
///
/// # Safety
/// `base` must be a loaded module.
pub unsafe fn hook_module(base: usize) {
    if !linked() {
        return;
    }
    let hooks: [(u16, &str, usize, &AtomicUsize); 10] = [
        (1, "accept", accept as *const () as usize, &ACCEPT),
        (2, "bind", bind as *const () as usize, &BIND),
        (4, "connect", connect as *const () as usize, &CONNECT),
        (5, "getpeername", get_peer_name as *const () as usize, &GET_PEER_NAME),
        (6, "getsockname", get_sock_name as *const () as usize, &GET_SOCK_NAME),
        (17, "recvfrom", recv_from as *const () as usize, &RECV_FROM),
        (20, "sendto", send_to as *const () as usize, &SEND_TO),
        (52, "gethostbyname", get_host_by_name as *const () as usize, &GET_HOST_BY_NAME),
        (57, "gethostname", get_host_name as *const () as usize, &GET_HOST_NAME),
        (0, "WSAIoctl", wsa_ioctl as *const () as usize, &WSA_IOCTL),
    ];
    for dll in ["ws2_32.dll", "wsock32.dll"] {
        for (ordinal, name, f, slot) in hooks {
            let by_ordinal = || if ordinal != 0 { unsafe { iat::hook_ordinal(base, dll, ordinal, f) } } else { None };
            if let Some(o) = by_ordinal().or_else(|| unsafe { iat::hook_module(base, dll, name, f) }) {
                let _ = slot.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
            }
        }
        if let Some(o) = unsafe { iat::hook_module(base, dll, "WSASendTo", wsa_send_to as *const () as usize) } {
            let _ = WSA_SEND_TO.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
    if let Some(o) = unsafe { iat::hook_module(base, "iphlpapi.dll", "GetAdaptersInfo", get_adapters_info as *const () as usize) } {
        let _ = GET_ADAPTERS_INFO.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook_module(base, "iphlpapi.dll", "GetAdaptersAddresses", get_adapters_addresses as *const () as usize) } {
        let _ = GET_ADAPTERS_ADDRESSES.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// The cabinet's role found ([`start`]: up to 3 s listening for a master, on the calling thread),
/// its table kept ([`serve_master`], [`serve_cabinet`]) and the game executable's sockets made the
/// link's ([`hook_module`]). The cabinets' count known by now, `0` alone. From `DllMain` too.
pub fn init() -> usize {
    let Some(l) = link() else { return 0 };
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(move || if l.cabinet == 1 { serve_master(l) } else { serve_cabinet(l) });
    }
    unsafe { hook_module(iat::game_module()) };
    count()
}

/// Waits (at most `timeout`) for the master's first table: a game that reads the cabinets once
/// (not from `DllMain`: the table's thread waits for it). Whether it came (the master: at once).
pub fn wait(timeout: Duration) -> bool {
    if cabinet() == 1 {
        return true;
    }
    let start = Instant::now();
    while !READY.load(Ordering::Relaxed) && start.elapsed() < timeout {
        std::thread::sleep(Duration::from_millis(50));
    }
    READY.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_settings() {
        assert!(settings("", "1", "", "").is_none());
        assert!(settings("0", "1", "127.0.0.2", "").is_none());
        let s = settings("1", "auto", "", "").unwrap();
        assert_eq!((s.cabinet, s.master), (None, None));
        let s = settings("1", "2", "bad", "127.0.0.3").unwrap();
        assert_eq!((s.cabinet, s.master, s.address), (Some(2), None, Some([127, 0, 0, 3])));
    }

    #[test]
    fn master_table() {
        assert_eq!(parse_table("WALLINK1 T you=3 2=10.0.0.2 3=10.0.0.3"), Some((3, vec![(2, [10, 0, 0, 2]), (3, [10, 0, 0, 3])])));
        assert_eq!(parse_table("WALLINK1 T you=1"), None);
        assert_eq!(parse_table("WALLINK1 A g -"), None);
    }
}
