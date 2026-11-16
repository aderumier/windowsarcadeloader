//! Magician's Dead's Leap Motion (`magiciansdead.rs`): the game's `Leap.dll` (the Leap SDK 3.2
//! C++ API, delay loaded by the Unreal plugin) is this module: its 223 imported functions are
//! exported here under their MSVC names (x64: `this` in rcx, a returned class or vector through
//! the hidden pointer in rdx).
//!
//! Every Leap object is the SDK's `Interface`: a vtable pointer and `m_object`. Here `m_object`
//! is a value, not a pointer: the object's kind, its hand / finger / bone indices, list flags and
//! the serial of the frame it belongs to ([`Ref`]); 0 is the invalid object. The frames are
//! snapshots kept in a ring ([`RING`] frames): a new one at most every 8 ms, made when the plugin
//! asks the controller for its frame (its tick, on the game thread).
//!
//! The device is always connected, the right hand always over it (the cabinet's player): open,
//! the palm towards the screen and the fingers up (the pose the game's attract shows; the game
//! aims a reticle where the hand is). The left hand only while the `both` gesture is held. The
//! hand's position follows player 1's right stick (x across, y the height; a mouse pointer
//! mapped there by the profile), or the touchpad's first finger ([`touch_gesture`]: its finger
//! count shapes the hand), moving at most [`MAX_SPEED`] so that aiming is never read as one of
//! the speed gestures.
//!
//! Gestures: the game reads the hand's shape and motion (its force skills: "bend the thumb",
//! "clench then open", "fingers in an L"; a rock / paper / scissors hand shape). Each gesture
//! input changes the hand ([`Gesture`]): the plugin's detectors see it as a real hand's and press
//! the game's Leap keys (`LeapRightThumbFallDown`, `LeapRightHandGrab`...: Unreal keys bound to
//! its actions). `WAL_LEAP_KEYS=1` also presses the matching key itself, through the detectors'
//! own key press / release functions (Slate key events, on the game thread: from
//! `Controller::frame`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use wal_payload_common::log;
use wal_protocol::Axis;

use crate::magiciansdead::{Native, for_each_pressed};

/// The gesture inputs (natives of `magiciansdead.rs`), as long as they are held.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Gesture {
    /// The thumb flicked: bent across the palm and straightened again, over and over while held
    /// (親指まげ: the shots; the detector wants the bend and its return within 150 ms).
    Thumb,
    /// The fist clenched (にぎる), opened on release (ひらく).
    Fist,
    /// The thumb and the index finger out, an L (指をL字).
    LShape,
    /// The index finger alone.
    Point,
    /// The index and middle fingers (scissors).
    Scissors,
    /// The hand thrust towards the screen.
    Push,
    /// The left hand over the sensor too, in the same shape.
    Both,
}

const GESTURES: [Gesture; 7] = [Gesture::Thumb, Gesture::Fist, Gesture::LShape, Gesture::Point, Gesture::Scissors, Gesture::Push, Gesture::Both];

/// The game's Leap keys (their `FKey` statics, executable RVAs).
mod key {
    pub const RIGHT_THUMB_FALL_DOWN: usize = 0x42cdec0;
    pub const RIGHT_FINGER_UP: usize = 0x42ce3e0;
    pub const RIGHT_FINGER_TAP: usize = 0x42cdf28;
    pub const RIGHT_PUSH_SPEED: usize = 0x42cdf40;
    pub const RIGHT_HAND_GRAB: usize = 0x42ce268;
    pub const BOTH_DEPTH_NEAR: usize = 0x42cddb8;
}

/// The keys a gesture presses while held (`WAL_LEAP_KEYS=1`).
fn keys(g: Gesture) -> &'static [usize] {
    match g {
        Gesture::Thumb => &[key::RIGHT_THUMB_FALL_DOWN],
        Gesture::Fist => &[key::RIGHT_HAND_GRAB],
        Gesture::LShape => &[key::RIGHT_FINGER_UP],
        Gesture::Point => &[key::RIGHT_FINGER_TAP],
        Gesture::Scissors => &[],
        Gesture::Push => &[key::RIGHT_PUSH_SPEED],
        Gesture::Both => &[key::BOTH_DEPTH_NEAR],
    }
}

/// The detectors' key press and release functions (an `FKey` copy, one reference of it consumed).
const KEY_PRESS: usize = 0x2857fa0;
const KEY_RELEASE: usize = 0x2858200;
/// Their first bytes (`mov [rsp+8], rbx; push rdi; sub rsp, 0x40; mov rax, [rcx]; mov rbx, rcx`).
const KEY_FN_PROLOGUE: [u8; 16] = [0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x40, 0x48, 0x8b, 0x01, 0x48, 0x8b, 0xd9];

/// Snapshots kept (frame history: `Controller::frame(n)`, objects kept by the plugin).
const RING: usize = 256;
/// The hand's speed limit, mm/s (the plugin's speed gestures start far above).
const MAX_SPEED: f32 = 250.0;
/// The hand's range from the rest position, mm.
const REACH_X: f32 = 110.0;
const REACH_Y: f32 = 90.0;
const REST: V = V { x: 0.0, y: 200.0, z: 0.0 };
/// The open hand's palm normal (towards the screen) and direction (fingers up).
const NORMAL: V = V { x: 0.0, y: 0.0, z: -1.0 };
const DIRECTION: V = V { x: 0.0, y: 1.0, z: 0.0 };
/// A thrust: how far towards the screen (mm) and how fast (mm/s).
const PUSH_DEPTH: f32 = 80.0;
const PUSH_SPEED: f32 = 900.0;
/// How fast a finger bends or straightens (curl per second: a full bend in 150 ms; the
/// detectors drop a shape that changes at once as noise).
const CURL_SPEED: f32 = 6.5;
/// A thumb flick: the bend's and the return's duration, then the pause before the next (s).
const FLICK: f32 = 0.07;
const FLICK_PAUSE: f32 = 0.25;
/// How long the touchpad's finger count must stay the same to change the hand's shape (s).
const TOUCH_SETTLE: f32 = 0.08;

pub(crate) fn init() {
    let exe = wal_payload_common::iat::game_module();
    if std::env::var("WAL_LEAP_KEYS").is_ok_and(|v| v == "1") {
        let ok = [KEY_PRESS, KEY_RELEASE].iter().all(|rva| unsafe { ((exe + rva) as *const [u8; 16]).read_unaligned() } == KEY_FN_PROLOGUE);
        INJECT.store(ok as u32, Ordering::Relaxed);
        log!("leap: gesture keys {}", if ok { "pressed by the payload" } else { "not pressed: unknown executable" });
    }
}

static INJECT: AtomicU32 = AtomicU32::new(0);

// ---- objects ----

#[derive(Clone, Copy, Default, PartialEq, Debug)]
#[repr(C)]
pub struct V {
    x: f32,
    y: f32,
    z: f32,
}

impl V {
    const fn new(x: f32, y: f32, z: f32) -> V {
        V { x, y, z }
    }
    fn add(self, o: V) -> V {
        V::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    fn sub(self, o: V) -> V {
        V::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn mul(self, k: f32) -> V {
        V::new(self.x * k, self.y * k, self.z * k)
    }
    fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
    fn norm(self) -> V {
        let l = self.len();
        if l > 1e-6 { self.mul(1.0 / l) } else { V::new(0.0, 0.0, -1.0) }
    }
    fn cross(self, o: V) -> V {
        V::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
}

/// Leap::Matrix: x, y, z bases and origin.
#[repr(C)]
pub struct M {
    x: V,
    y: V,
    z: V,
    o: V,
}

/// Leap::Interface: vtable, m_object.
#[repr(C)]
pub struct Obj {
    vtable: *const usize,
    m: u64,
}

/// The objects' vtable: the scalar deleting destructor (nothing to free).
static VTABLE: [unsafe extern "C" fn(*mut Obj, u32) -> *mut Obj; 1] = [deleting_destructor];

unsafe extern "C" fn deleting_destructor(this: *mut Obj, _flags: u32) -> *mut Obj {
    this
}

mod kind {
    pub const FRAME: u8 = 1;
    pub const HAND_LIST: u8 = 2;
    pub const HAND: u8 = 3;
    pub const FINGER_LIST: u8 = 4;
    pub const FINGER: u8 = 5;
    pub const POINTABLE_LIST: u8 = 6;
    pub const BONE: u8 = 7;
    pub const ARM: u8 = 8;
    pub const DEVICE_LIST: u8 = 9;
    pub const DEVICE: u8 = 10;
    pub const INTERACTION_BOX: u8 = 11;
    pub const CONFIG: u8 = 12;
}

/// All hands / fingers of the frame (a list's hand field).
const ALL: u8 = 0xff;
/// A list's flag: extended fingers only.
const EXTENDED: u8 = 1;

/// A decoded `m_object`.
#[derive(Clone, Copy, PartialEq)]
struct Ref {
    kind: u8,
    hand: u8,
    finger: u8,
    bone: u8,
    flags: u8,
    serial: u32,
}

impl Ref {
    fn decode(m: u64) -> Ref {
        Ref { kind: m as u8, hand: (m >> 8) as u8, finger: (m >> 16) as u8, bone: (m >> 24) as u8, flags: (m >> 32) as u8, serial: (m >> 40) as u32 }
    }
    fn encode(self) -> u64 {
        u64::from(self.kind) | u64::from(self.hand) << 8 | u64::from(self.finger) << 16 | u64::from(self.bone) << 24 | u64::from(self.flags) << 32 | u64::from(self.serial & 0xff_ffff) << 40
    }
    fn of(kind: u8, serial: u32) -> Ref {
        Ref { kind, hand: ALL, finger: 0, bone: 0, flags: 0, serial }
    }
}

unsafe fn get(this: *const Obj) -> Option<Ref> {
    let m = unsafe { (*this).m };
    (m != 0).then(|| Ref::decode(m))
}

unsafe fn put(ret: *mut Obj, r: Option<Ref>) -> *mut Obj {
    unsafe { ret.write(Obj { vtable: VTABLE.as_ptr().cast(), m: r.map_or(0, Ref::encode) }) };
    ret
}

unsafe fn put_v(ret: *mut V, v: V) -> *mut V {
    unsafe { ret.write(v) };
    ret
}

// ---- frames ----

#[derive(Clone, Copy, Default)]
struct Hand {
    present: bool,
    left: bool,
    palm: V,
    velocity: V,
    normal: V,
    direction: V,
    grab: f32,
    /// Each finger's curl, thumb to little finger: 0 straight, 1 bent (extended below 0.5).
    curl: [f32; 5],
    since: f32,
}

#[derive(Clone, Copy, Default)]
struct Snap {
    serial: u32,
    id: i64,
    timestamp: i64,
    /// Right, left.
    hands: [Hand; 2],
}

struct State {
    ring: Vec<Snap>,
    serial: u32,
    last: Option<Instant>,
    start: Option<Instant>,
    right_since: Option<Instant>,
    left_since: Option<Instant>,
    palm: V,
    /// The thrust's depth now (mm towards the screen).
    push: f32,
    /// The fingers' curl now (moving towards the gesture's at [`CURL_SPEED`]).
    curl: [f32; 5],
    /// The gestures held (their keys pressed).
    held: Vec<Gesture>,
    /// Where the touchpad last placed the hand.
    touch: Option<V>,
    /// The fingers down on the touchpad, since when.
    touch_count: (usize, Option<Instant>),
    /// The touchpad's gesture (its finger count once settled).
    touch_gesture: Option<Gesture>,
    /// Since when the thumb flicks.
    flick: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    ring: Vec::new(),
    serial: 0,
    last: None,
    start: None,
    right_since: None,
    left_since: None,
    palm: REST,
    push: 0.0,
    curl: [0.0; 5],
    held: Vec::new(),
    touch: None,
    touch_count: (0, None),
    touch_gesture: None,
    flick: None,
});

/// The touchpad's fingers down -> the hand's shape: one aims (open hand), two bend the thumb
/// (the shots), three clench the fist (opened when they lift), four make the L.
fn touch_gesture(fingers: usize) -> Option<Gesture> {
    match fingers {
        2 => Some(Gesture::Thumb),
        3 => Some(Gesture::Fist),
        4.. => Some(Gesture::LShape),
        _ => None,
    }
}

fn snap(serial: u32) -> Option<Snap> {
    let st = STATE.lock().unwrap();
    st.ring.get(serial as usize % RING).filter(|s| s.serial == serial && serial != 0).copied()
}

fn hand_of(r: Ref) -> Option<Hand> {
    let s = snap(r.serial)?;
    s.hands.get(r.hand as usize).filter(|h| h.present).copied()
}

/// A new snapshot from the inputs when 8 ms have passed; the latest serial.
fn update() -> u32 {
    let now = Instant::now();
    let mut held = Vec::new();
    for_each_pressed(|n| {
        if let Native::Hand(g) = n
            && !held.contains(&g)
        {
            held.push(g);
        }
    });
    let mut st = STATE.lock().unwrap();
    if st.ring.is_empty() {
        st.ring = vec![Snap::default(); RING];
    }
    if st.last.is_some_and(|l| now.duration_since(l).as_secs_f32() < 0.008) {
        return st.serial;
    }
    let dt = st.last.map_or(0.016, |l| now.duration_since(l).as_secs_f32()).min(0.1);
    let start = *st.start.get_or_insert(now);
    st.last = Some(now);
    // the right stick: the hand's target, reached at MAX_SPEED
    let stick = wal_payload_common::input(0);
    let unit = |a: Axis| f32::from(stick.axis(a)) / 32767.0;
    let (sx, sy) = (unit(Axis::RightX), unit(Axis::RightY));
    let mut target = REST.add(V::new(sx * REACH_X, -sy * REACH_Y, 0.0));
    // the touchpad (profile `input.touch`): its first finger places the hand (kept there when
    // the fingers lift, until the stick moves), the number of fingers down shapes it once it
    // has not changed for TOUCH_SETTLE (lifting three fingers is not a two finger gesture)
    let touches = wal_payload_common::touches();
    if let Some(&(x, y)) = touches.first() {
        let n = |v: u16| f32::from(v) / 65535.0 * 2.0 - 1.0;
        st.touch = Some(REST.add(V::new(n(x) * REACH_X, -n(y) * REACH_Y, 0.0)));
    } else if sx.abs() > 0.1 || sy.abs() > 0.1 {
        st.touch = None;
    }
    if let Some(t) = st.touch {
        target = t;
    }
    if st.touch_count.0 != touches.len() {
        st.touch_count = (touches.len(), Some(now));
    }
    if st.touch_count.1.is_none_or(|t| now.duration_since(t).as_secs_f32() >= TOUCH_SETTLE) {
        st.touch_gesture = touch_gesture(touches.len());
    }
    if let Some(g) = st.touch_gesture
        && !held.contains(&g)
    {
        held.push(g);
    }
    let step = target.sub(st.palm);
    let max = MAX_SPEED * dt;
    let moved = if step.len() > max { step.norm().mul(max) } else { step };
    st.palm = st.palm.add(moved);
    // a thrust: fast towards the screen while held, back at the aiming speed
    let push_target = if held.contains(&Gesture::Push) { PUSH_DEPTH } else { 0.0 };
    let push_step = (push_target - st.push).clamp(-MAX_SPEED * dt, PUSH_SPEED * dt);
    st.push += push_step;
    let palm = st.palm.add(V::new(0.0, 0.0, -st.push));
    let velocity = moved.add(V::new(0.0, 0.0, -push_step)).mul(1.0 / dt);
    let right_since = *st.right_since.get_or_insert(now);
    if held.contains(&Gesture::Both) {
        st.left_since.get_or_insert(now);
    } else {
        st.left_since = None;
    }
    let has = |g| held.contains(&g);
    // the fingers out, thumb to little finger
    let extended = if has(Gesture::Fist) {
        [false; 5]
    } else if has(Gesture::LShape) {
        [true, true, false, false, false]
    } else if has(Gesture::Point) {
        [false, true, false, false, false]
    } else if has(Gesture::Scissors) {
        [false, true, true, false, false]
    } else {
        [true; 5]
    };
    for (c, e) in st.curl.iter_mut().zip(extended) {
        let target = if e { 0.0 } else { 1.0 };
        *c += (target - *c).clamp(-CURL_SPEED * dt, CURL_SPEED * dt);
    }
    // the thumb's flicks, from the open hand
    if has(Gesture::Thumb) && extended[0] {
        let t = now.duration_since(*st.flick.get_or_insert(now)).as_secs_f32() % (2.0 * FLICK + FLICK_PAUSE);
        st.curl[0] = if t < FLICK { t / FLICK } else if t < 2.0 * FLICK { 2.0 - t / FLICK } else { 0.0 };
    } else {
        st.flick = None;
    }
    let curl = st.curl;
    let shape = |left: bool, palm: V, since: Instant| Hand {
        present: true,
        left,
        palm,
        velocity,
        normal: NORMAL,
        direction: DIRECTION,
        grab: curl[1..].iter().sum::<f32>() / 4.0,
        curl,
        since: now.duration_since(since).as_secs_f32(),
    };
    let right = shape(false, palm, right_since);
    let left = match st.left_since {
        Some(since) => shape(true, V::new(-palm.x - 60.0, palm.y, palm.z), since),
        None => Hand::default(),
    };
    st.serial = st.serial.wrapping_add(1) & 0xff_ffff;
    if st.serial == 0 {
        st.serial = 1;
    }
    let serial = st.serial;
    st.ring[serial as usize % RING] =
        Snap { serial, id: i64::from(serial), timestamp: now.duration_since(start).as_micros() as i64, hands: [right, left] };
    // the gestures' keys
    let before = std::mem::replace(&mut st.held, held.clone());
    drop(st);
    for g in GESTURES {
        let (was, is) = (before.contains(&g), held.contains(&g));
        if was != is {
            log!("leap: {g:?} {}", if is { "on" } else { "off" });
            if INJECT.load(Ordering::Relaxed) != 0 {
                for k in keys(g) {
                    unsafe { send_key(*k, is) };
                }
            }
        }
    }
    serial
}

/// Presses or releases a game Leap key as the plugin's detectors do.
unsafe fn send_key(rva: usize, pressed: bool) {
    let exe = wal_payload_common::iat::game_module();
    let key = (exe + rva) as *const [usize; 3];
    let mut copy = unsafe { key.read() };
    if copy[0] == 0 {
        log!("leap: key +{rva:#x} not registered");
        return;
    }
    // the copy's reference (TSharedPtr<FKeyDetails>: its controller's shared count at +8)
    if copy[2] != 0 {
        unsafe { *((copy[2] + 8) as *mut i32) += 1 };
    }
    let f: unsafe extern "C" fn(*mut [usize; 3]) -> u8 = unsafe { std::mem::transmute(exe + if pressed { KEY_PRESS } else { KEY_RELEASE }) };
    unsafe { f(&mut copy) };
}

fn hands(s: &Snap) -> Vec<u8> {
    (0..2u8).filter(|&i| s.hands[i as usize].present).collect()
}

/// The elements of a hand, finger or pointable list.
fn elements(r: Ref) -> Vec<Ref> {
    let Some(s) = snap(r.serial) else { return Vec::new() };
    match r.kind {
        kind::HAND_LIST => hands(&s).into_iter().map(|h| Ref { kind: kind::HAND, hand: h, ..Ref::of(kind::HAND, r.serial) }).collect(),
        kind::FINGER_LIST | kind::POINTABLE_LIST => {
            let which: Vec<u8> = if r.hand == ALL { hands(&s) } else { vec![r.hand] };
            let mut out = Vec::new();
            for h in which {
                let hand = s.hands[h as usize];
                for f in 0..5u8 {
                    if r.flags & EXTENDED == 0 || extended(&hand, f as usize) {
                        out.push(Ref { kind: kind::FINGER, hand: h, finger: f, bone: 0, flags: 0, serial: r.serial });
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

// ---- hand geometry ----

/// The hand's side axis (towards the little finger for a right hand palm down).
/// A finger counts as extended while less than half bent.
fn extended(h: &Hand, f: usize) -> bool {
    h.curl[f] < 0.5
}

fn side(h: &Hand) -> V {
    h.normal.cross(h.direction)
}

/// The finger's five joints: carpal, knuckle, then the ends of its three bones (the thumb's
/// metacarpal has no length: Leap's convention).
fn joints(h: &Hand, f: usize) -> [V; 5] {
    const SIDE: [f32; 5] = [-38.0, -22.0, -3.0, 15.0, 30.0];
    const FORWARD: [f32; 5] = [-5.0, 45.0, 48.0, 44.0, 38.0];
    const BONES: [[f32; 3]; 5] = [[45.0, 30.0, 22.0], [40.0, 23.0, 17.0], [45.0, 27.0, 18.0], [41.0, 26.0, 18.0], [33.0, 18.0, 16.0]];
    let hs = if h.left { -1.0 } else { 1.0 };
    let (d, n, s) = (h.direction, h.normal, side(h).mul(hs));
    let knuckle = h.palm.add(d.mul(FORWARD[f])).add(s.mul(SIDE[f]));
    let carpal = if f == 0 { knuckle } else { h.palm.sub(d.mul(35.0)).add(s.mul(SIDE[f] * 0.6)) };
    let c = h.curl[f];
    let dirs: [V; 3] = if f == 0 {
        // out to the side, bent across the palm
        let out = d.mul(0.6).sub(s.mul(0.8));
        let across = s.mul(0.7).add(n.mul(0.7));
        [out.mul(1.0 - c).add(across.mul(c)).norm(); 3]
    } else {
        let bend = |deg: f32| d.mul((deg * c).to_radians().cos()).add(n.mul((deg * c).to_radians().sin()));
        [bend(80.0), bend(170.0), bend(250.0)]
    };
    let mut j = [carpal, knuckle, knuckle, knuckle, knuckle];
    for b in 0..3 {
        j[b + 2] = j[b + 1].add(dirs[b].mul(BONES[f][b]));
    }
    j
}

fn basis(x: V, y: V, z: V, o: V) -> M {
    M { x, y, z, o }
}

fn hand_basis(h: &Hand) -> M {
    let x = if h.left { side(h).mul(-1.0) } else { side(h) };
    basis(x, h.normal.mul(-1.0), h.direction.mul(-1.0), h.palm)
}

fn finger_ref(this: *const Obj) -> Option<(Hand, Ref)> {
    let r = unsafe { get(this) }?;
    (r.kind == kind::FINGER || r.kind == kind::BONE).then_some(())?;
    Some((hand_of(r)?, r))
}

fn bone(this: *const Obj) -> Option<(V, V)> {
    let (h, r) = finger_ref(this)?;
    let j = joints(&h, r.finger as usize);
    Some((j[r.bone as usize], j[r.bone as usize + 1]))
}

fn arm(this: *const Obj) -> Option<(V, V, Hand)> {
    let r = unsafe { get(this) }?;
    let h = hand_of(r)?;
    let wrist = h.palm.sub(h.direction.mul(45.0));
    let elbow = wrist.sub(h.direction.mul(250.0)).add(V::new(0.0, 0.0, 80.0));
    Some((wrist, elbow, h))
}

fn hand(this: *const Obj) -> Option<Hand> {
    let r = unsafe { get(this) }?;
    hand_of(r)
}

/// The same hand in another frame (`Frame const&`).
fn same_hand(this: *const Obj, frame: *const Obj) -> Option<(Hand, Hand)> {
    let r = unsafe { get(this) }?;
    let f = unsafe { get(frame) }?;
    Some((hand_of(r)?, hand_of(Ref { serial: f.serial, ..r })?))
}

// ---- the exports ----

macro_rules! ctor {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *mut Obj) -> *mut Obj {
            unsafe { put(this, None) }
        }
    )*};
}

ctor! {
    "??0Arm@Leap@@QEAA@XZ" => arm_new,
    "??0Bone@Leap@@QEAA@XZ" => bone_new,
    "??0CircleGesture@Leap@@QEAA@XZ" => circle_new,
    "??0Finger@Leap@@QEAA@XZ" => finger_new,
    "??0FingerList@Leap@@QEAA@XZ" => finger_list_new,
    "??0Frame@Leap@@QEAA@XZ" => frame_new,
    "??0Gesture@Leap@@QEAA@XZ" => gesture_new,
    "??0GestureList@Leap@@QEAA@XZ" => gesture_list_new,
    "??0Hand@Leap@@QEAA@XZ" => hand_new,
    "??0HandList@Leap@@QEAA@XZ" => hand_list_new,
    "??0Image@Leap@@QEAA@XZ" => image_new,
    "??0ImageList@Leap@@QEAA@XZ" => image_list_new,
    "??0InteractionBox@Leap@@QEAA@XZ" => interaction_box_new,
    "??0KeyTapGesture@Leap@@QEAA@XZ" => key_tap_new,
    "??0Pointable@Leap@@QEAA@XZ" => pointable_new,
    "??0PointableList@Leap@@QEAA@XZ" => pointable_list_new,
    "??0ScreenTapGesture@Leap@@QEAA@XZ" => screen_tap_new,
    "??0SwipeGesture@Leap@@QEAA@XZ" => swipe_new,
    "??0Tool@Leap@@QEAA@XZ" => tool_new,
    "??0ToolList@Leap@@QEAA@XZ" => tool_list_new,
}

/// The gesture types from a gesture: none (no gesture is ever made).
macro_rules! from_gesture {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *mut Obj, _gesture: *const Obj) -> *mut Obj {
            unsafe { put(this, None) }
        }
    )*};
}

from_gesture! {
    "??0CircleGesture@Leap@@QEAA@AEBVGesture@1@@Z" => circle_from,
    "??0KeyTapGesture@Leap@@QEAA@AEBVGesture@1@@Z" => key_tap_from,
    "??0ScreenTapGesture@Leap@@QEAA@AEBVGesture@1@@Z" => screen_tap_from,
    "??0SwipeGesture@Leap@@QEAA@AEBVGesture@1@@Z" => swipe_from,
}

#[unsafe(export_name = "??0Controller@Leap@@QEAA@XZ")]
pub unsafe extern "C" fn controller_new(this: *mut Obj) -> *mut Obj {
    log!("leap: controller created (device connected)");
    unsafe { put(this, Some(Ref::of(kind::FRAME, 0))) }
}

#[unsafe(export_name = "??1Controller@Leap@@UEAA@XZ")]
pub unsafe extern "C" fn controller_drop(_this: *mut Obj) {}

#[unsafe(export_name = "??0Interface@Leap@@IEAA@AEBV01@@Z")]
pub unsafe extern "C" fn interface_copy(this: *mut Obj, other: *const Obj) -> *mut Obj {
    unsafe { this.write(Obj { vtable: VTABLE.as_ptr().cast(), m: (*other).m }) };
    this
}

#[unsafe(export_name = "??4Interface@Leap@@IEAAAEAV01@AEBV01@@Z")]
pub unsafe extern "C" fn interface_assign(this: *mut Obj, other: *const Obj) -> *mut Obj {
    unsafe { (*this).m = (*other).m };
    this
}

#[unsafe(export_name = "??1Interface@Leap@@MEAA@XZ")]
pub unsafe extern "C" fn interface_drop(_this: *mut Obj) {}

/// Every validity test: a valid reference to an existing object.
macro_rules! valid {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj) -> bool {
            unsafe { get(this) }.is_some_and(|r| r.kind == kind::FRAME && snap(r.serial).is_some() || r.kind != kind::FRAME && (hand_of(r).is_some() || r.kind == kind::INTERACTION_BOX))
        }
    )*};
}

valid! {
    "?isValid@Arm@Leap@@QEBA_NXZ" => arm_valid,
    "?isValid@Bone@Leap@@QEBA_NXZ" => bone_valid,
    "?isValid@Frame@Leap@@QEBA_NXZ" => frame_valid,
    "?isValid@InteractionBox@Leap@@QEBA_NXZ" => interaction_box_valid,
    "?isValid@Pointable@Leap@@QEBA_NXZ" => pointable_valid,
}

macro_rules! never {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj) -> bool {
            false
        }
    )*};
}

never! {
    "?isValid@Gesture@Leap@@QEBA_NXZ" => gesture_valid,
    "?isValid@Image@Leap@@QEBA_NXZ" => image_valid,
    "?isPaused@Controller@Leap@@QEBA_NXZ" => controller_paused,
    "?isSmudged@Device@Leap@@QEBA_NXZ" => device_smudged,
    "?isTool@Pointable@Leap@@QEBA_NXZ" => pointable_is_tool,
}

macro_rules! always {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj) -> bool {
            true
        }
    )*};
}

always! {
    "?hasFocus@Controller@Leap@@QEBA_NXZ" => controller_focus,
    "?isConnected@Controller@Leap@@QEBA_NXZ" => controller_connected,
    "?isServiceConnected@Controller@Leap@@QEBA_NXZ" => controller_service,
    "?isEmpty@GestureList@Leap@@QEBA_NXZ" => gesture_list_empty,
    "?isEmpty@ImageList@Leap@@QEBA_NXZ" => image_list_empty,
    "?isEmpty@ToolList@Leap@@QEBA_NXZ" => tool_list_empty,
}

#[unsafe(export_name = "?isFinger@Pointable@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn pointable_is_finger(this: *const Obj) -> bool {
    finger_ref(this).is_some()
}

// -- Controller

#[unsafe(export_name = "?frame@Controller@Leap@@QEBA?AVFrame@2@H@Z")]
pub unsafe extern "C" fn controller_frame(_this: *const Obj, ret: *mut Obj, history: i32) -> *mut Obj {
    let latest = update();
    let serial = latest.wrapping_sub(history.clamp(0, RING as i32 - 1) as u32) & 0xff_ffff;
    unsafe { put(ret, snap(serial).map(|_| Ref::of(kind::FRAME, serial))) }
}

#[unsafe(export_name = "?config@Controller@Leap@@QEBA?AVConfig@2@XZ")]
pub unsafe extern "C" fn controller_config(_this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, Some(Ref::of(kind::CONFIG, 0))) }
}

#[unsafe(export_name = "?devices@Controller@Leap@@QEBA?AVDeviceList@2@XZ")]
pub unsafe extern "C" fn controller_devices(_this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, Some(Ref::of(kind::DEVICE_LIST, 0))) }
}

#[unsafe(export_name = "?enableGesture@Controller@Leap@@QEBAXW4Type@Gesture@2@_N@Z")]
pub unsafe extern "C" fn controller_enable_gesture(_this: *const Obj, _kind: i32, _enable: bool) {}

#[unsafe(export_name = "?setPolicy@Controller@Leap@@QEBAXW4PolicyFlag@12@@Z")]
pub unsafe extern "C" fn controller_set_policy(_this: *const Obj, policy: i32) {
    log!("leap: policy {policy:#x}");
}

#[unsafe(export_name = "?clearPolicy@Controller@Leap@@QEBAXW4PolicyFlag@12@@Z")]
pub unsafe extern "C" fn controller_clear_policy(_this: *const Obj, _policy: i32) {}

#[unsafe(export_name = "?setPaused@Controller@Leap@@QEAAX_N@Z")]
pub unsafe extern "C" fn controller_set_paused(_this: *mut Obj, _paused: bool) {}

#[unsafe(export_name = "?save@Config@Leap@@QEAA_NXZ")]
pub unsafe extern "C" fn config_save(_this: *mut Obj) -> bool {
    true
}

#[unsafe(export_name = "?setBoolCString@Config@Leap@@AEAA_NPEBD_N@Z")]
pub unsafe extern "C" fn config_set_bool(_this: *mut Obj, _key: *const u8, _value: bool) -> bool {
    true
}

#[unsafe(export_name = "?count@DeviceList@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn device_list_count(_this: *const Obj) -> i32 {
    1
}

#[unsafe(export_name = "??ADeviceList@Leap@@QEBA?AVDevice@1@H@Z")]
pub unsafe extern "C" fn device_list_at(_this: *const Obj, ret: *mut Obj, index: i32) -> *mut Obj {
    unsafe { put(ret, (index == 0).then(|| Ref::of(kind::DEVICE, 0))) }
}

// -- Frame

fn frame(this: *const Obj) -> Option<(Ref, Snap)> {
    let r = unsafe { get(this) }?;
    Some((r, snap(r.serial)?))
}

#[unsafe(export_name = "?id@Frame@Leap@@QEBA_JXZ")]
pub unsafe extern "C" fn frame_id(this: *const Obj) -> i64 {
    frame(this).map_or(-1, |(_, s)| s.id)
}

#[unsafe(export_name = "?timestamp@Frame@Leap@@QEBA_JXZ")]
pub unsafe extern "C" fn frame_timestamp(this: *const Obj) -> i64 {
    frame(this).map_or(0, |(_, s)| s.timestamp)
}

#[unsafe(export_name = "?currentFramesPerSecond@Frame@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn frame_fps(_this: *const Obj) -> f32 {
    110.0
}

#[unsafe(export_name = "?hands@Frame@Leap@@QEBA?AVHandList@2@XZ")]
pub unsafe extern "C" fn frame_hands(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, frame(this).map(|(r, _)| Ref::of(kind::HAND_LIST, r.serial))) }
}

#[unsafe(export_name = "?fingers@Frame@Leap@@QEBA?AVFingerList@2@XZ")]
pub unsafe extern "C" fn frame_fingers(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, frame(this).map(|(r, _)| Ref::of(kind::FINGER_LIST, r.serial))) }
}

#[unsafe(export_name = "?pointables@Frame@Leap@@QEBA?AVPointableList@2@XZ")]
pub unsafe extern "C" fn frame_pointables(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, frame(this).map(|(r, _)| Ref::of(kind::POINTABLE_LIST, r.serial))) }
}

/// Hand ids: right 1, left 2; finger ids: the hand's id * 10 + its type.
fn hand_id(h: u8) -> i32 {
    i32::from(h) + 1
}

#[unsafe(export_name = "?hand@Frame@Leap@@QEBA?AVHand@2@H@Z")]
pub unsafe extern "C" fn frame_hand(this: *const Obj, ret: *mut Obj, id: i32) -> *mut Obj {
    let found = frame(this).and_then(|(r, s)| hands(&s).into_iter().find(|&h| hand_id(h) == id).map(|h| Ref { hand: h, ..Ref::of(kind::HAND, r.serial) }));
    unsafe { put(ret, found) }
}

fn finger_by_id(this: *const Obj, id: i32) -> Option<Ref> {
    let (r, s) = frame(this)?;
    let (h, f) = ((id / 10 - 1) as u8, (id % 10) as u8);
    (f < 5 && hands(&s).contains(&h)).then(|| Ref { kind: kind::FINGER, hand: h, finger: f, bone: 0, flags: 0, serial: r.serial })
}

#[unsafe(export_name = "?finger@Frame@Leap@@QEBA?AVFinger@2@H@Z")]
pub unsafe extern "C" fn frame_finger(this: *const Obj, ret: *mut Obj, id: i32) -> *mut Obj {
    unsafe { put(ret, finger_by_id(this, id)) }
}

#[unsafe(export_name = "?pointable@Frame@Leap@@QEBA?AVPointable@2@H@Z")]
pub unsafe extern "C" fn frame_pointable(this: *const Obj, ret: *mut Obj, id: i32) -> *mut Obj {
    unsafe { put(ret, finger_by_id(this, id)) }
}

macro_rules! invalid_object {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj, ret: *mut Obj) -> *mut Obj {
            unsafe { put(ret, None) }
        }
    )*};
}

// no tools, gestures nor images; the gestures' pointables, hands and frames invalid
invalid_object! {
    "?tools@Frame@Leap@@QEBA?AVToolList@2@XZ" => frame_tools,
    "?gestures@Frame@Leap@@QEBA?AVGestureList@2@XZ" => frame_gestures,
    "?images@Frame@Leap@@QEBA?AVImageList@2@XZ" => frame_images,
    "?hands@Gesture@Leap@@QEBA?AVHandList@2@XZ" => gesture_hands,
    "?frame@Gesture@Leap@@QEBA?AVFrame@2@XZ" => gesture_frame,
    "?pointables@Gesture@Leap@@QEBA?AVPointableList@2@XZ" => gesture_pointables,
    "?pointable@CircleGesture@Leap@@QEBA?AVPointable@2@XZ" => circle_pointable,
    "?pointable@KeyTapGesture@Leap@@QEBA?AVPointable@2@XZ" => key_tap_pointable,
    "?pointable@ScreenTapGesture@Leap@@QEBA?AVPointable@2@XZ" => screen_tap_pointable,
    "?pointable@SwipeGesture@Leap@@QEBA?AVPointable@2@XZ" => swipe_pointable,
    "?frontmost@ToolList@Leap@@QEBA?AVTool@2@XZ" => tool_list_frontmost,
    "?leftmost@ToolList@Leap@@QEBA?AVTool@2@XZ" => tool_list_leftmost,
    "?rightmost@ToolList@Leap@@QEBA?AVTool@2@XZ" => tool_list_rightmost,
}

macro_rules! invalid_indexed {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj, ret: *mut Obj, _index: i32) -> *mut Obj {
            unsafe { put(ret, None) }
        }
    )*};
}

invalid_indexed! {
    "?tool@Frame@Leap@@QEBA?AVTool@2@H@Z" => frame_tool,
    "?gesture@Frame@Leap@@QEBA?AVGesture@2@H@Z" => frame_gesture,
    "??AGestureList@Leap@@QEBA?AVGesture@1@H@Z" => gesture_list_at,
    "??AImageList@Leap@@QEBA?AVImage@1@H@Z" => image_list_at,
    "??AToolList@Leap@@QEBA?AVTool@1@H@Z" => tool_list_at,
}

#[unsafe(export_name = "?gestures@Frame@Leap@@QEBA?AVGestureList@2@AEBV12@@Z")]
pub unsafe extern "C" fn frame_gestures_since(_this: *const Obj, ret: *mut Obj, _since: *const Obj) -> *mut Obj {
    unsafe { put(ret, None) }
}

#[unsafe(export_name = "?interactionBox@Frame@Leap@@QEBA?AVInteractionBox@2@XZ")]
pub unsafe extern "C" fn frame_interaction_box(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, frame(this).map(|(r, _)| Ref::of(kind::INTERACTION_BOX, r.serial))) }
}

/// The frames' motion: the right hand's (or the first one's).
fn frame_palms(this: *const Obj, since: *const Obj) -> Option<(V, V)> {
    let (_, now) = frame(this)?;
    let (_, then) = frame(since)?;
    let h = hands(&now).into_iter().find(|&h| then.hands[h as usize].present)?;
    Some((now.hands[h as usize].palm, then.hands[h as usize].palm))
}

#[unsafe(export_name = "?translation@Frame@Leap@@QEBA?AUVector@2@AEBV12@@Z")]
pub unsafe extern "C" fn frame_translation(this: *const Obj, ret: *mut V, since: *const Obj) -> *mut V {
    unsafe { put_v(ret, frame_palms(this, since).map_or(V::default(), |(a, b)| a.sub(b))) }
}

#[unsafe(export_name = "?translationProbability@Frame@Leap@@QEBAMAEBV12@@Z")]
pub unsafe extern "C" fn frame_translation_probability(this: *const Obj, since: *const Obj) -> f32 {
    frame_palms(this, since).map_or(0.0, |_| 1.0)
}

#[unsafe(export_name = "?rotationAxis@Frame@Leap@@QEBA?AUVector@2@AEBV12@@Z")]
pub unsafe extern "C" fn frame_rotation_axis(_this: *const Obj, ret: *mut V, _since: *const Obj) -> *mut V {
    unsafe { put_v(ret, V::new(0.0, 0.0, 1.0)) }
}

macro_rules! since_value {
    ($($name:literal => $f:ident = $v:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj, _since: *const Obj) -> f32 {
            $v
        }
    )*};
}

since_value! {
    "?rotationAngle@Frame@Leap@@QEBAMAEBV12@@Z" => frame_rotation_angle = 0.0,
    "?rotationProbability@Frame@Leap@@QEBAMAEBV12@@Z" => frame_rotation_probability = 0.0,
    "?scaleFactor@Frame@Leap@@QEBAMAEBV12@@Z" => frame_scale = 1.0,
    "?scaleProbability@Frame@Leap@@QEBAMAEBV12@@Z" => frame_scale_probability = 0.0,
    "?rotationAngle@Hand@Leap@@QEBAMAEBVFrame@2@@Z" => hand_rotation_angle = 0.0,
    "?rotationProbability@Hand@Leap@@QEBAMAEBVFrame@2@@Z" => hand_rotation_probability = 0.0,
    "?scaleFactor@Hand@Leap@@QEBAMAEBVFrame@2@@Z" => hand_scale = 1.0,
    "?scaleProbability@Hand@Leap@@QEBAMAEBVFrame@2@@Z" => hand_scale_probability = 0.0,
}

#[unsafe(export_name = "?rotationAngle@Frame@Leap@@QEBAMAEBV12@AEBUVector@2@@Z")]
pub unsafe extern "C" fn frame_rotation_angle_axis(_this: *const Obj, _since: *const Obj, _axis: *const V) -> f32 {
    0.0
}

#[unsafe(export_name = "?rotationAngle@Hand@Leap@@QEBAMAEBVFrame@2@AEBUVector@2@@Z")]
pub unsafe extern "C" fn hand_rotation_angle_axis(_this: *const Obj, _since: *const Obj, _axis: *const V) -> f32 {
    0.0
}

// -- lists

#[unsafe(export_name = "?count@HandList@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn hand_list_count(this: *const Obj) -> i32 {
    unsafe { get(this) }.map_or(0, |r| elements(r).len() as i32)
}

#[unsafe(export_name = "?count@FingerList@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn finger_list_count(this: *const Obj) -> i32 {
    unsafe { hand_list_count(this) }
}

#[unsafe(export_name = "?count@PointableList@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn pointable_list_count(this: *const Obj) -> i32 {
    unsafe { hand_list_count(this) }
}

#[unsafe(export_name = "?isEmpty@HandList@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn hand_list_empty(this: *const Obj) -> bool {
    unsafe { hand_list_count(this) == 0 }
}

#[unsafe(export_name = "?isEmpty@FingerList@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn finger_list_empty(this: *const Obj) -> bool {
    unsafe { hand_list_count(this) == 0 }
}

#[unsafe(export_name = "?isEmpty@PointableList@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn pointable_list_empty(this: *const Obj) -> bool {
    unsafe { hand_list_count(this) == 0 }
}

macro_rules! zero_count {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj) -> i32 {
            0
        }
    )*};
}

zero_count! {
    "?count@GestureList@Leap@@QEBAHXZ" => gesture_list_count,
    "?count@ImageList@Leap@@QEBAHXZ" => image_list_count,
    "?count@ToolList@Leap@@QEBAHXZ" => tool_list_count,
}

unsafe fn list_at(this: *const Obj, ret: *mut Obj, index: i32) -> *mut Obj {
    let e = unsafe { get(this) }.and_then(|r| usize::try_from(index).ok().and_then(|i| elements(r).get(i).copied()));
    unsafe { put(ret, e) }
}

#[unsafe(export_name = "??AHandList@Leap@@QEBA?AVHand@1@H@Z")]
pub unsafe extern "C" fn hand_list_at(this: *const Obj, ret: *mut Obj, index: i32) -> *mut Obj {
    unsafe { list_at(this, ret, index) }
}

#[unsafe(export_name = "??AFingerList@Leap@@QEBA?AVFinger@1@H@Z")]
pub unsafe extern "C" fn finger_list_at(this: *const Obj, ret: *mut Obj, index: i32) -> *mut Obj {
    unsafe { list_at(this, ret, index) }
}

#[unsafe(export_name = "??APointableList@Leap@@QEBA?AVPointable@1@H@Z")]
pub unsafe extern "C" fn pointable_list_at(this: *const Obj, ret: *mut Obj, index: i32) -> *mut Obj {
    unsafe { list_at(this, ret, index) }
}

/// The list's element at the extreme of `key` (smallest first).
unsafe fn list_extreme(this: *const Obj, ret: *mut Obj, key: fn(&Ref) -> f32) -> *mut Obj {
    let e = unsafe { get(this) }.and_then(|r| elements(r).into_iter().min_by(|a, b| key(a).total_cmp(&key(b))));
    unsafe { put(ret, e) }
}

/// An element's position: a hand's palm, a finger's tip.
fn position(r: &Ref) -> V {
    let Some(h) = hand_of(*r) else { return V::default() };
    if r.kind == kind::FINGER { joints(&h, r.finger as usize)[4] } else { h.palm }
}

macro_rules! extremes {
    ($($name:literal => $f:ident = $key:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj, ret: *mut Obj) -> *mut Obj {
            unsafe { list_extreme(this, ret, $key) }
        }
    )*};
}

extremes! {
    "?leftmost@HandList@Leap@@QEBA?AVHand@2@XZ" => hand_list_leftmost = |r| position(r).x,
    "?rightmost@HandList@Leap@@QEBA?AVHand@2@XZ" => hand_list_rightmost = |r| -position(r).x,
    "?frontmost@HandList@Leap@@QEBA?AVHand@2@XZ" => hand_list_frontmost = |r| position(r).z,
    "?leftmost@FingerList@Leap@@QEBA?AVFinger@2@XZ" => finger_list_leftmost = |r| position(r).x,
    "?rightmost@FingerList@Leap@@QEBA?AVFinger@2@XZ" => finger_list_rightmost = |r| -position(r).x,
    "?frontmost@FingerList@Leap@@QEBA?AVFinger@2@XZ" => finger_list_frontmost = |r| position(r).z,
    "?leftmost@PointableList@Leap@@QEBA?AVPointable@2@XZ" => pointable_list_leftmost = |r| position(r).x,
    "?rightmost@PointableList@Leap@@QEBA?AVPointable@2@XZ" => pointable_list_rightmost = |r| -position(r).x,
    "?frontmost@PointableList@Leap@@QEBA?AVPointable@2@XZ" => pointable_list_frontmost = |r| position(r).z,
}

unsafe fn list_extended(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, get(this).map(|r| Ref { flags: r.flags | EXTENDED, ..r })) }
}

#[unsafe(export_name = "?extended@FingerList@Leap@@QEBA?AV12@XZ")]
pub unsafe extern "C" fn finger_list_extended(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { list_extended(this, ret) }
}

#[unsafe(export_name = "?extended@PointableList@Leap@@QEBA?AV12@XZ")]
pub unsafe extern "C" fn pointable_list_extended(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { list_extended(this, ret) }
}

/// Appending: an empty list becomes the other one (lists are views of a frame, not containers).
unsafe fn list_append(this: *mut Obj, other: *const Obj, kind: u8) -> *mut Obj {
    unsafe {
        if (*this).m == 0 || hand_list_count(this) == 0 {
            (*this).m = get(other).map_or(0, |r| Ref { kind, ..r }.encode());
        }
    }
    this
}

#[unsafe(export_name = "?append@FingerList@Leap@@QEAAAEAV12@AEBV12@@Z")]
pub unsafe extern "C" fn finger_list_append(this: *mut Obj, other: *const Obj) -> *mut Obj {
    unsafe { list_append(this, other, kind::FINGER_LIST) }
}

#[unsafe(export_name = "?append@PointableList@Leap@@QEAAAEAV12@AEBV12@@Z")]
pub unsafe extern "C" fn pointable_list_append(this: *mut Obj, other: *const Obj) -> *mut Obj {
    unsafe { list_append(this, other, kind::POINTABLE_LIST) }
}

#[unsafe(export_name = "?append@PointableList@Leap@@QEAAAEAV12@AEBVFingerList@2@@Z")]
pub unsafe extern "C" fn pointable_list_append_fingers(this: *mut Obj, other: *const Obj) -> *mut Obj {
    unsafe { list_append(this, other, kind::POINTABLE_LIST) }
}

#[unsafe(export_name = "?append@PointableList@Leap@@QEAAAEAV12@AEBVToolList@2@@Z")]
pub unsafe extern "C" fn pointable_list_append_tools(this: *mut Obj, _other: *const Obj) -> *mut Obj {
    this
}

#[unsafe(export_name = "?append@ToolList@Leap@@QEAAAEAV12@AEBV12@@Z")]
pub unsafe extern "C" fn tool_list_append(this: *mut Obj, _other: *const Obj) -> *mut Obj {
    this
}

// -- Hand

#[unsafe(export_name = "?id@Hand@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn hand_get_id(this: *const Obj) -> i32 {
    unsafe { get(this) }.filter(|_| hand(this).is_some()).map_or(-1, |r| hand_id(r.hand))
}

#[unsafe(export_name = "?isLeft@Hand@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn hand_is_left(this: *const Obj) -> bool {
    hand(this).is_some_and(|h| h.left)
}

#[unsafe(export_name = "?isRight@Hand@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn hand_is_right(this: *const Obj) -> bool {
    hand(this).is_some_and(|h| !h.left)
}

#[unsafe(export_name = "??8Hand@Leap@@QEBA_NAEBV01@@Z")]
pub unsafe extern "C" fn hand_eq(this: *const Obj, other: *const Obj) -> bool {
    unsafe { (*this).m == (*other).m }
}

macro_rules! hand_vector {
    ($($name:literal => $f:ident = $v:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj, ret: *mut V) -> *mut V {
            let f: fn(&Hand) -> V = $v;
            unsafe { put_v(ret, hand(this).map_or(V::default(), |h| f(&h))) }
        }
    )*};
}

hand_vector! {
    "?palmPosition@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_palm = |h| h.palm,
    "?stabilizedPalmPosition@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_stabilized_palm = |h| h.palm,
    "?palmVelocity@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_velocity = |h| h.velocity,
    "?palmNormal@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_normal = |h| h.normal,
    "?direction@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_direction = |h| h.direction,
    "?sphereCenter@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_sphere_center = |h| h.palm.add(h.normal.mul(40.0)),
    "?wristPosition@Hand@Leap@@QEBA?AUVector@2@XZ" => hand_wrist = |h| h.palm.sub(h.direction.mul(45.0)),
}

macro_rules! hand_float {
    ($($name:literal => $f:ident = $v:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj) -> f32 {
            let f: fn(&Hand) -> f32 = $v;
            hand(this).map_or(0.0, |h| f(&h))
        }
    )*};
}

hand_float! {
    "?grabStrength@Hand@Leap@@QEBAMXZ" => hand_grab = |h| h.grab,
    "?pinchStrength@Hand@Leap@@QEBAMXZ" => hand_pinch = |_| 0.0,
    "?confidence@Hand@Leap@@QEBAMXZ" => hand_confidence = |_| 1.0,
    "?palmWidth@Hand@Leap@@QEBAMXZ" => hand_palm_width = |_| 85.0,
    "?sphereRadius@Hand@Leap@@QEBAMXZ" => hand_sphere_radius = |h| 90.0 - 50.0 * h.grab,
    "?timeVisible@Hand@Leap@@QEBAMXZ" => hand_time_visible = |h| h.since,
}

#[unsafe(export_name = "?basis@Hand@Leap@@QEBA?AUMatrix@2@XZ")]
pub unsafe extern "C" fn hand_get_basis(this: *const Obj, ret: *mut M) -> *mut M {
    let m = hand(this).map_or_else(identity, |h| hand_basis(&h));
    unsafe { ret.write(m) };
    ret
}

fn identity() -> M {
    basis(V::new(1.0, 0.0, 0.0), V::new(0.0, 1.0, 0.0), V::new(0.0, 0.0, 1.0), V::default())
}

#[unsafe(export_name = "?rotationMatrix@Hand@Leap@@QEBA?AUMatrix@2@AEBVFrame@2@@Z")]
pub unsafe extern "C" fn hand_rotation_matrix(_this: *const Obj, ret: *mut M, _since: *const Obj) -> *mut M {
    unsafe { ret.write(identity()) };
    ret
}

#[unsafe(export_name = "?rotationAxis@Hand@Leap@@QEBA?AUVector@2@AEBVFrame@2@@Z")]
pub unsafe extern "C" fn hand_rotation_axis(_this: *const Obj, ret: *mut V, _since: *const Obj) -> *mut V {
    unsafe { put_v(ret, V::new(0.0, 0.0, 1.0)) }
}

#[unsafe(export_name = "?translation@Hand@Leap@@QEBA?AUVector@2@AEBVFrame@2@@Z")]
pub unsafe extern "C" fn hand_translation(this: *const Obj, ret: *mut V, since: *const Obj) -> *mut V {
    unsafe { put_v(ret, same_hand(this, since).map_or(V::default(), |(a, b)| a.palm.sub(b.palm))) }
}

#[unsafe(export_name = "?translationProbability@Hand@Leap@@QEBAMAEBVFrame@2@@Z")]
pub unsafe extern "C" fn hand_translation_probability(this: *const Obj, since: *const Obj) -> f32 {
    same_hand(this, since).map_or(0.0, |_| 1.0)
}

#[unsafe(export_name = "?fingers@Hand@Leap@@QEBA?AVFingerList@2@XZ")]
pub unsafe extern "C" fn hand_fingers(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    let r = unsafe { get(this) }.filter(|_| hand(this).is_some());
    unsafe { put(ret, r.map(|r| Ref { kind: kind::FINGER_LIST, flags: 0, ..r })) }
}

#[unsafe(export_name = "?arm@Hand@Leap@@QEBA?AVArm@2@XZ")]
pub unsafe extern "C" fn hand_arm(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    let r = unsafe { get(this) }.filter(|_| hand(this).is_some());
    unsafe { put(ret, r.map(|r| Ref { kind: kind::ARM, ..r })) }
}

#[unsafe(export_name = "?frame@Hand@Leap@@QEBA?AVFrame@2@XZ")]
pub unsafe extern "C" fn hand_frame(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, get(this).map(|r| Ref::of(kind::FRAME, r.serial))) }
}

// -- Arm

macro_rules! arm_vector {
    ($($name:literal => $f:ident = $v:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj, ret: *mut V) -> *mut V {
            let f: fn(V, V) -> V = $v;
            unsafe { put_v(ret, arm(this).map_or(V::default(), |(w, e, _)| f(w, e))) }
        }
    )*};
}

arm_vector! {
    "?wristPosition@Arm@Leap@@QEBA?AUVector@2@XZ" => arm_wrist = |w, _| w,
    "?elbowPosition@Arm@Leap@@QEBA?AUVector@2@XZ" => arm_elbow = |_, e| e,
    "?center@Arm@Leap@@QEBA?AUVector@2@XZ" => arm_center = |w, e| w.add(e).mul(0.5),
    "?direction@Arm@Leap@@QEBA?AUVector@2@XZ" => arm_direction = |w, e| w.sub(e).norm(),
}

#[unsafe(export_name = "?width@Arm@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn arm_width(this: *const Obj) -> f32 {
    arm(this).map_or(0.0, |_| 60.0)
}

#[unsafe(export_name = "?basis@Arm@Leap@@QEBA?AUMatrix@2@XZ")]
pub unsafe extern "C" fn arm_basis(this: *const Obj, ret: *mut M) -> *mut M {
    let m = arm(this).map_or_else(identity, |(w, e, h)| {
        let z = e.sub(w).norm();
        let x = if h.left { side(&h).mul(-1.0) } else { side(&h) };
        basis(x, z.cross(x).norm(), z, w.add(e).mul(0.5))
    });
    unsafe { ret.write(m) };
    ret
}

// -- Pointable / Finger

#[unsafe(export_name = "??0Finger@Leap@@QEAA@AEBVPointable@1@@Z")]
pub unsafe extern "C" fn finger_from_pointable(this: *mut Obj, pointable: *const Obj) -> *mut Obj {
    unsafe { put(this, get(pointable).filter(|r| r.kind == kind::FINGER)) }
}

#[unsafe(export_name = "?id@Pointable@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn pointable_id(this: *const Obj) -> i32 {
    finger_ref(this).map_or(-1, |(_, r)| hand_id(r.hand) * 10 + i32::from(r.finger))
}

#[unsafe(export_name = "?type@Finger@Leap@@QEBA?AW4Type@12@XZ")]
pub unsafe extern "C" fn finger_type(this: *const Obj) -> i32 {
    finger_ref(this).map_or(0, |(_, r)| i32::from(r.finger))
}

#[unsafe(export_name = "?isExtended@Pointable@Leap@@QEBA_NXZ")]
pub unsafe extern "C" fn pointable_extended(this: *const Obj) -> bool {
    finger_ref(this).is_some_and(|(h, r)| extended(&h, r.finger as usize))
}

#[unsafe(export_name = "??8Pointable@Leap@@QEBA_NAEBV01@@Z")]
pub unsafe extern "C" fn pointable_eq(this: *const Obj, other: *const Obj) -> bool {
    unsafe { (*this).m == (*other).m }
}

#[unsafe(export_name = "??9Pointable@Leap@@QEBA_NAEBV01@@Z")]
pub unsafe extern "C" fn pointable_ne(this: *const Obj, other: *const Obj) -> bool {
    unsafe { (*this).m != (*other).m }
}

fn tip(this: *const Obj) -> Option<(Hand, [V; 5])> {
    let (h, r) = finger_ref(this)?;
    Some((h, joints(&h, r.finger as usize)))
}

#[unsafe(export_name = "?tipPosition@Pointable@Leap@@QEBA?AUVector@2@XZ")]
pub unsafe extern "C" fn pointable_tip(this: *const Obj, ret: *mut V) -> *mut V {
    unsafe { put_v(ret, tip(this).map_or(V::default(), |(_, j)| j[4])) }
}

#[unsafe(export_name = "?stabilizedTipPosition@Pointable@Leap@@QEBA?AUVector@2@XZ")]
pub unsafe extern "C" fn pointable_stabilized_tip(this: *const Obj, ret: *mut V) -> *mut V {
    unsafe { pointable_tip(this, ret) }
}

#[unsafe(export_name = "?tipVelocity@Pointable@Leap@@QEBA?AUVector@2@XZ")]
pub unsafe extern "C" fn pointable_tip_velocity(this: *const Obj, ret: *mut V) -> *mut V {
    unsafe { put_v(ret, tip(this).map_or(V::default(), |(h, _)| h.velocity)) }
}

#[unsafe(export_name = "?direction@Pointable@Leap@@QEBA?AUVector@2@XZ")]
pub unsafe extern "C" fn pointable_direction(this: *const Obj, ret: *mut V) -> *mut V {
    unsafe { put_v(ret, tip(this).map_or(V::default(), |(_, j)| j[4].sub(j[3]).norm())) }
}

#[unsafe(export_name = "?length@Pointable@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn pointable_length(this: *const Obj) -> f32 {
    tip(this).map_or(0.0, |(_, j)| j[4].sub(j[3]).len() + j[3].sub(j[2]).len() + j[2].sub(j[1]).len())
}

#[unsafe(export_name = "?width@Pointable@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn pointable_width(this: *const Obj) -> f32 {
    tip(this).map_or(0.0, |_| 18.0)
}

#[unsafe(export_name = "?timeVisible@Pointable@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn pointable_time_visible(this: *const Obj) -> f32 {
    tip(this).map_or(0.0, |(h, _)| h.since)
}

#[unsafe(export_name = "?touchDistance@Pointable@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn pointable_touch_distance(_this: *const Obj) -> f32 {
    1.0
}

#[unsafe(export_name = "?touchZone@Pointable@Leap@@QEBA?AW4Zone@12@XZ")]
pub unsafe extern "C" fn pointable_touch_zone(_this: *const Obj) -> i32 {
    0
}

#[unsafe(export_name = "?hand@Pointable@Leap@@QEBA?AVHand@2@XZ")]
pub unsafe extern "C" fn pointable_hand(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, finger_ref(this).map(|(_, r)| Ref { kind: kind::HAND, finger: 0, bone: 0, flags: 0, ..r })) }
}

#[unsafe(export_name = "?frame@Pointable@Leap@@QEBA?AVFrame@2@XZ")]
pub unsafe extern "C" fn pointable_frame(this: *const Obj, ret: *mut Obj) -> *mut Obj {
    unsafe { put(ret, get(this).map(|r| Ref::of(kind::FRAME, r.serial))) }
}

#[unsafe(export_name = "?bone@Finger@Leap@@QEBA?AVBone@2@W4Type@32@@Z")]
pub unsafe extern "C" fn finger_bone(this: *const Obj, ret: *mut Obj, bone: i32) -> *mut Obj {
    let r = finger_ref(this).filter(|_| (0..4).contains(&bone)).map(|(_, r)| Ref { kind: kind::BONE, bone: bone as u8, ..r });
    unsafe { put(ret, r) }
}

// -- Bone

#[unsafe(export_name = "?type@Bone@Leap@@QEBA?AW4Type@12@XZ")]
pub unsafe extern "C" fn bone_type(this: *const Obj) -> i32 {
    unsafe { get(this) }.map_or(0, |r| i32::from(r.bone))
}

#[unsafe(export_name = "??8Bone@Leap@@QEBA_NAEBV01@@Z")]
pub unsafe extern "C" fn bone_eq(this: *const Obj, other: *const Obj) -> bool {
    unsafe { (*this).m == (*other).m }
}

#[unsafe(export_name = "??9Bone@Leap@@QEBA_NAEBV01@@Z")]
pub unsafe extern "C" fn bone_ne(this: *const Obj, other: *const Obj) -> bool {
    unsafe { (*this).m != (*other).m }
}

macro_rules! bone_vector {
    ($($name:literal => $f:ident = $v:expr),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(this: *const Obj, ret: *mut V) -> *mut V {
            let f: fn(V, V) -> V = $v;
            unsafe { put_v(ret, bone(this).map_or(V::default(), |(a, b)| f(a, b))) }
        }
    )*};
}

bone_vector! {
    "?prevJoint@Bone@Leap@@QEBA?AUVector@2@XZ" => bone_prev = |a, _| a,
    "?nextJoint@Bone@Leap@@QEBA?AUVector@2@XZ" => bone_next = |_, b| b,
    "?center@Bone@Leap@@QEBA?AUVector@2@XZ" => bone_center = |a, b| a.add(b).mul(0.5),
    "?direction@Bone@Leap@@QEBA?AUVector@2@XZ" => bone_direction = |a, b| b.sub(a).norm(),
}

#[unsafe(export_name = "?length@Bone@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn bone_length(this: *const Obj) -> f32 {
    bone(this).map_or(0.0, |(a, b)| b.sub(a).len())
}

#[unsafe(export_name = "?width@Bone@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn bone_width(this: *const Obj) -> f32 {
    bone(this).map_or(0.0, |_| 18.0)
}

#[unsafe(export_name = "?basis@Bone@Leap@@QEBA?AUMatrix@2@XZ")]
pub unsafe extern "C" fn bone_basis(this: *const Obj, ret: *mut M) -> *mut M {
    let m = finger_ref(this).zip(bone(this)).map_or_else(identity, |((h, _), (a, b))| {
        let z = a.sub(b).norm();
        let x = if h.left { side(&h).mul(-1.0) } else { side(&h) };
        basis(x, z.cross(x).norm(), z, a.add(b).mul(0.5))
    });
    unsafe { ret.write(m) };
    ret
}

// -- InteractionBox

const BOX_CENTER: V = V { x: 0.0, y: 200.0, z: 0.0 };
const BOX_SIZE: V = V { x: 235.0, y: 235.0, z: 147.0 };

#[unsafe(export_name = "?center@InteractionBox@Leap@@QEBA?AUVector@2@XZ")]
pub unsafe extern "C" fn box_center(_this: *const Obj, ret: *mut V) -> *mut V {
    unsafe { put_v(ret, BOX_CENTER) }
}

#[unsafe(export_name = "?width@InteractionBox@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn box_width(_this: *const Obj) -> f32 {
    BOX_SIZE.x
}

#[unsafe(export_name = "?height@InteractionBox@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn box_height(_this: *const Obj) -> f32 {
    BOX_SIZE.y
}

#[unsafe(export_name = "?depth@InteractionBox@Leap@@QEBAMXZ")]
pub unsafe extern "C" fn box_depth(_this: *const Obj) -> f32 {
    BOX_SIZE.z
}

#[unsafe(export_name = "?normalizePoint@InteractionBox@Leap@@QEBA?AUVector@2@AEBU32@_N@Z")]
pub unsafe extern "C" fn box_normalize(_this: *const Obj, ret: *mut V, p: *const V, clamp: bool) -> *mut V {
    let p = unsafe { *p };
    let n = |v: f32, c: f32, s: f32| {
        let x = (v - c) / s + 0.5;
        if clamp { x.clamp(0.0, 1.0) } else { x }
    };
    unsafe { put_v(ret, V::new(n(p.x, BOX_CENTER.x, BOX_SIZE.x), n(p.y, BOX_CENTER.y, BOX_SIZE.y), n(p.z, BOX_CENTER.z, BOX_SIZE.z))) }
}

#[unsafe(export_name = "?denormalizePoint@InteractionBox@Leap@@QEBA?AUVector@2@AEBU32@@Z")]
pub unsafe extern "C" fn box_denormalize(_this: *const Obj, ret: *mut V, p: *const V) -> *mut V {
    let p = unsafe { *p };
    let d = |v: f32, c: f32, s: f32| (v - 0.5) * s + c;
    unsafe { put_v(ret, V::new(d(p.x, BOX_CENTER.x, BOX_SIZE.x), d(p.y, BOX_CENTER.y, BOX_SIZE.y), d(p.z, BOX_CENTER.z, BOX_SIZE.z))) }
}

// -- gestures and images: none

#[unsafe(export_name = "?id@Gesture@Leap@@QEBAHXZ")]
pub unsafe extern "C" fn gesture_id(_this: *const Obj) -> i32 {
    -1
}

#[unsafe(export_name = "?type@Gesture@Leap@@QEBA?AW4Type@12@XZ")]
pub unsafe extern "C" fn gesture_type(_this: *const Obj) -> i32 {
    -1
}

#[unsafe(export_name = "?state@Gesture@Leap@@QEBA?AW4State@12@XZ")]
pub unsafe extern "C" fn gesture_state(_this: *const Obj) -> i32 {
    -1
}

#[unsafe(export_name = "?duration@Gesture@Leap@@QEBA_JXZ")]
pub unsafe extern "C" fn gesture_duration(_this: *const Obj) -> i64 {
    0
}

macro_rules! zero_float {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj) -> f32 {
            0.0
        }
    )*};
}

zero_float! {
    "?durationSeconds@Gesture@Leap@@QEBAMXZ" => gesture_duration_seconds,
    "?progress@CircleGesture@Leap@@QEBAMXZ" => circle_progress,
    "?radius@CircleGesture@Leap@@QEBAMXZ" => circle_radius,
    "?progress@KeyTapGesture@Leap@@QEBAMXZ" => key_tap_progress,
    "?progress@ScreenTapGesture@Leap@@QEBAMXZ" => screen_tap_progress,
    "?speed@SwipeGesture@Leap@@QEBAMXZ" => swipe_speed,
    "?rayOffsetX@Image@Leap@@QEBAMXZ" => image_ray_offset_x,
    "?rayOffsetY@Image@Leap@@QEBAMXZ" => image_ray_offset_y,
    "?rayScaleX@Image@Leap@@QEBAMXZ" => image_ray_scale_x,
    "?rayScaleY@Image@Leap@@QEBAMXZ" => image_ray_scale_y,
}

macro_rules! zero_vector {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj, ret: *mut V) -> *mut V {
            unsafe { put_v(ret, V::default()) }
        }
    )*};
}

zero_vector! {
    "?center@CircleGesture@Leap@@QEBA?AUVector@2@XZ" => circle_center,
    "?normal@CircleGesture@Leap@@QEBA?AUVector@2@XZ" => circle_normal,
    "?direction@KeyTapGesture@Leap@@QEBA?AUVector@2@XZ" => key_tap_direction,
    "?position@KeyTapGesture@Leap@@QEBA?AUVector@2@XZ" => key_tap_position,
    "?direction@ScreenTapGesture@Leap@@QEBA?AUVector@2@XZ" => screen_tap_direction,
    "?position@ScreenTapGesture@Leap@@QEBA?AUVector@2@XZ" => screen_tap_position,
    "?direction@SwipeGesture@Leap@@QEBA?AUVector@2@XZ" => swipe_direction,
    "?position@SwipeGesture@Leap@@QEBA?AUVector@2@XZ" => swipe_position,
    "?startPosition@SwipeGesture@Leap@@QEBA?AUVector@2@XZ" => swipe_start,
}

#[unsafe(export_name = "?rectify@Image@Leap@@QEBA?AUVector@2@AEBU32@@Z")]
pub unsafe extern "C" fn image_rectify(_this: *const Obj, ret: *mut V, _p: *const V) -> *mut V {
    unsafe { put_v(ret, V::default()) }
}

#[unsafe(export_name = "?warp@Image@Leap@@QEBA?AUVector@2@AEBU32@@Z")]
pub unsafe extern "C" fn image_warp(_this: *const Obj, ret: *mut V, _p: *const V) -> *mut V {
    unsafe { put_v(ret, V::default()) }
}

macro_rules! zero_int {
    ($($name:literal => $f:ident),* $(,)?) => {$(
        #[unsafe(export_name = $name)]
        pub unsafe extern "C" fn $f(_this: *const Obj) -> i32 {
            0
        }
    )*};
}

zero_int! {
    "?id@Image@Leap@@QEBAHXZ" => image_id,
    "?width@Image@Leap@@QEBAHXZ" => image_width,
    "?height@Image@Leap@@QEBAHXZ" => image_height,
    "?distortionWidth@Image@Leap@@QEBAHXZ" => image_distortion_width,
    "?distortionHeight@Image@Leap@@QEBAHXZ" => image_distortion_height,
}

#[unsafe(export_name = "?data@Image@Leap@@QEBAPEBEXZ")]
pub unsafe extern "C" fn image_data(_this: *const Obj) -> *const u8 {
    std::ptr::null()
}

#[unsafe(export_name = "?distortion@Image@Leap@@QEBAPEBMXZ")]
pub unsafe extern "C" fn image_distortion(_this: *const Obj) -> *const f32 {
    std::ptr::null()
}
