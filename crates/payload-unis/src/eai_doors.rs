//! Elevator Action Invasion's elevator doors: the cabinet's two doors in front of the screen,
//! driven by the game (`BIND_SetDoorData(left, right)`: each door's closed part, 0 open .. 10
//! closed). The doors move to their targets at the cabinet's pace; their positions are the
//! board's door sensors.

use std::sync::Mutex;
use std::time::Instant;

use wal_payload_common::log;

/// The doors' full travel (open to closed), seconds.
const TRAVEL: f32 = 1.5;

struct Doors {
    /// Targets and positions, 0 open .. 1 closed.
    target: [f32; 2],
    position: [f32; 2],
    last: Option<Instant>,
}

static DOORS: Mutex<Doors> = Mutex::new(Doors { target: [0.0; 2], position: [0.0; 2], last: None });

/// The game's door command.
pub(crate) fn set(left: i32, right: i32) {
    let mut d = DOORS.lock().unwrap();
    let t = [(left.clamp(0, 10) as f32) / 10.0, (right.clamp(0, 10) as f32) / 10.0];
    if t != d.target {
        log!("doors: left {left} right {right} (closed part, of 10)");
    }
    d.target = t;
}

/// The doors' positions now (0 open .. 1 closed), moved since the last call.
pub(crate) fn positions() -> [f32; 2] {
    let mut d = DOORS.lock().unwrap();
    let now = Instant::now();
    let dt = d.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32());
    d.last = Some(now);
    let step = dt / TRAVEL;
    for i in 0..2 {
        let delta = d.target[i] - d.position[i];
        d.position[i] += delta.clamp(-step, step);
    }
    d.position
}

/// The board's door sensors: each door's closed part, 0..10.
pub(crate) fn closed() -> (u8, u8) {
    let p = positions();
    ((p[0] * 10.0).round() as u8, (p[1] * 10.0).round() as u8)
}

/// `WAL_UNIS_DOORS=<folder>` (relative to the executable's directory): the doors drawn over the
/// game's picture with the folder's `DoorLeft.png`, `DoorRight.png` (each half of the picture
/// wide, its height) and `DoorsBezel.png` (the doors' frame over the picture, optional): a
/// closed door covers its half, an open one is beside the picture.
pub(crate) fn init() {
    let Ok(folder) = std::env::var("WAL_UNIS_DOORS") else { return };
    let base = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let dir = base.join(folder.trim());
    let mut images = Vec::new();
    for name in ["DoorLeft.png", "DoorRight.png"] {
        match wal_payload_common::images_d3d11::read_png(&dir.join(name)) {
            Ok(image) => images.push(image),
            Err(e) => {
                log!("doors: {e}: no doors drawn");
                return;
            }
        }
    }
    match wal_payload_common::images_d3d11::read_png(&dir.join("DoorsBezel.png")) {
        Ok(frame) => images.push(frame),
        Err(e) => log!("doors: {e}: no frame"),
    }
    log!("doors: drawn with {}", dir.display());
    wal_payload_common::images_d3d11::init(images, quads);
}

/// Each frame: the doors where they are, the frame over them.
fn quads() -> Vec<wal_payload_common::images_d3d11::Quad> {
    use wal_payload_common::images_d3d11::Quad;
    let [left, right] = positions();
    let mut q = Vec::with_capacity(3);
    if left > 0.0 {
        q.push(Quad { image: 0, rect: [-0.5 + 0.5 * left, 0.0, 0.5, 1.0] });
    }
    if right > 0.0 {
        q.push(Quad { image: 1, rect: [1.0 - 0.5 * right, 0.0, 0.5, 1.0] });
    }
    q.push(Quad { image: 2, rect: [0.0, 0.0, 1.0, 1.0] });
    q
}
