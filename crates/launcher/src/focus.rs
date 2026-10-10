//! The game's window to the front (X11): during the first seconds of the game, when another
//! window is active, the game's largest window (`_NET_WM_PID` in the game's process group) is
//! activated through the window manager (`_NET_ACTIVE_WINDOW`).
//!
//! On Batocera EmulationStation keeps the focus with its fullscreen window and no game window
//! came to the front (black screen, the game's sound only); inside Wine the game's window is
//! the foreground window already, so the game cannot ask for it itself.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window};
use x11rb::rust_connection::RustConnection;

/// How long the launcher watches the focus after the game starts.
const WATCH: Duration = Duration::from_secs(30);
const PERIOD: Duration = Duration::from_millis(500);

pub struct Focus {
    conn: RustConnection,
    root: Window,
    active: u32,
    client_list: u32,
    wm_pid: u32,
    /// The game's process group.
    group: i32,
    start: Instant,
    last: Instant,
    logged: bool,
}

impl Focus {
    /// None without an X display.
    pub fn new(group: u32) -> Option<Focus> {
        std::env::var_os("DISPLAY")?;
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &str| Some(conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom);
        let (active, client_list, wm_pid) = (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_CLIENT_LIST")?, atom("_NET_WM_PID")?);
        let now = Instant::now();
        Some(Focus { conn, root, active, client_list, wm_pid, group: group as i32, start: now, last: now, logged: false })
    }

    fn windows(&self, window: Window, property: u32) -> Vec<u32> {
        self.conn
            .get_property(false, window, property, AtomEnum::ANY, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    /// The game's largest window, None when one of its windows is active (or it has none yet).
    fn inactive_game_window(&self) -> Option<Window> {
        let active = self.windows(self.root, self.active).first().copied().unwrap_or(0);
        let mut best: Option<(Window, u32)> = None;
        for w in self.windows(self.root, self.client_list) {
            let Some(pid) = self.windows(w, self.wm_pid).first().copied() else { continue };
            if unsafe { libc::getpgid(pid as i32) } != self.group {
                continue;
            }
            if w == active {
                return None;
            }
            let Some(g) = self.conn.get_geometry(w).ok().and_then(|c| c.reply().ok()) else { continue };
            let area = g.width as u32 * g.height as u32;
            if best.is_none_or(|(_, a)| area > a) {
                best = Some((w, area));
            }
        }
        best.map(|(w, _)| w)
    }

    /// Called from the launcher's loop.
    pub fn poll(&mut self) {
        if self.start.elapsed() > WATCH || self.last.elapsed() < PERIOD {
            return;
        }
        self.last = Instant::now();
        let Some(window) = self.inactive_game_window() else { return };
        // source 2: a pager / the user's request, which window managers honor
        let event = ClientMessageEvent::new(32, window, self.active, [2, 0, 0, 0, 0]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        if self.conn.send_event(false, self.root, mask, event).is_ok() && self.conn.flush().is_ok() && !self.logged {
            eprintln!("launcher: game window {window:#x} brought to the front");
            self.logged = true;
        }
    }
}
