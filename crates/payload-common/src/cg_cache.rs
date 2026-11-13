//! `WAL_CG_CACHE=<folder>`: the programs a game compiles with NVIDIA's Cg runtime (`cg.dll`)
//! kept in that folder (relative to the game's working directory), compiled once.
//!
//! A game using Cg only as its shader compiler (`cgCreateProgram` from source, then
//! `cgGetProgramString` for the compiled assembly, `cgDestroyProgram`) pays the Cg compiler at
//! every boot: Wonderland Wars builds its ~9700 shaders for about 10 minutes. Each program is
//! keyed by its type, source, profile, entry and compiler arguments; the strings the game asks
//! from it (`cgGetProgramString`) and the compiler's listing (`cgGetLastListing`) are saved. A
//! known program is answered by a fake program that serves those strings, the compiler not run
//! (a string not saved yet compiles it then). Failed compilations are not kept.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::{iat, log};

type CreateProgram = unsafe extern "C" fn(usize, u32, *const c_char, u32, *const c_char, *const *const c_char) -> usize;
type GetProgramString = unsafe extern "C" fn(usize, u32) -> *const c_char;
type DestroyProgram = unsafe extern "C" fn(usize);
type GetLastListing = unsafe extern "C" fn(usize) -> *const c_char;
type GetError = unsafe extern "C" fn() -> u32;

static CREATE: AtomicUsize = AtomicUsize::new(0);
static GET_STRING: AtomicUsize = AtomicUsize::new(0);
static DESTROY: AtomicUsize = AtomicUsize::new(0);
static LISTING: AtomicUsize = AtomicUsize::new(0);
static GET_ERROR: AtomicUsize = AtomicUsize::new(0);

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// A program's creation arguments, to compile it when a string is asked that was not saved.
struct Source {
    context: usize,
    kind: u32,
    source: CString,
    profile: u32,
    entry: Option<CString>,
    args: Vec<CString>,
}

/// A program: the real one (the strings asked saved under its key), or a fake one answered from
/// the cache.
struct Program {
    key: Vec<u8>,
    source: Source,
    /// The real program, once compiled.
    real: usize,
    /// The saved strings by name, and the copies handed to the game.
    strings: HashMap<u32, CString>,
}

static PROGRAMS: Mutex<Vec<(usize, Program)>> = Mutex::new(Vec::new());
/// Fake program handles: odd, never Cg's.
static NEXT_FAKE: AtomicUsize = AtomicUsize::new(0x7ac9_0001);
/// The last creation was answered from the cache: no Cg error, its saved listing.
static LAST_CACHED: AtomicBool = AtomicBool::new(false);
static LAST_LISTING: Mutex<Option<CString>> = Mutex::new(None);
static HITS: AtomicUsize = AtomicUsize::new(0);
static MISSES: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let Ok(dir) = std::env::var("WAL_CG_CACHE") else { return };
    let dir = PathBuf::from(dir.trim());
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return log!("cg_cache: {}: {e}", dir.display());
    }
    let _ = DIR.set(dir.clone());
    let hooks: [(&str, usize, &AtomicUsize); 5] = [
        ("cgCreateProgram", create_program as *const () as usize, &CREATE),
        ("cgGetProgramString", get_program_string as *const () as usize, &GET_STRING),
        ("cgDestroyProgram", destroy_program as *const () as usize, &DESTROY),
        ("cgGetLastListing", get_last_listing as *const () as usize, &LISTING),
        ("cgGetError", get_error as *const () as usize, &GET_ERROR),
    ];
    let mut hooked = 0;
    for (name, f, slot) in hooks {
        if let Some(orig) = unsafe { iat::hook("cg.dll", name, f) } {
            slot.store(orig, Ordering::Relaxed);
            hooked += 1;
        }
    }
    log!("cg_cache: Cg programs cached in {} ({hooked} functions)", dir.display());
}

fn cstr(p: *const c_char) -> Option<CString> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_owned())
}

/// The program's key: its creation arguments.
fn key(s: &Source) -> Vec<u8> {
    let mut k = format!("{} {}\n", s.kind, s.profile).into_bytes();
    for part in [Some(&s.source), s.entry.as_ref()].into_iter().chain(s.args.iter().map(Some)) {
        match part {
            Some(p) => {
                k.extend_from_slice(&(p.as_bytes().len() as u32).to_le_bytes());
                k.extend_from_slice(p.as_bytes());
            }
            None => k.extend_from_slice(&u32::MAX.to_le_bytes()),
        }
    }
    k
}

/// The key's SHA-256: the cache file's name (its first 16 bytes) and its check (all of it).
fn digest(k: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(k).into()
}

fn path(d: &[u8; 32]) -> Option<PathBuf> {
    let name: String = d[..16].iter().map(|b| format!("{b:02x}")).collect();
    DIR.get().map(|dir| dir.join(format!("{name}.cg")))
}

/// File: magic, the key's digest, then (name, length, bytes) per string; the listing under
/// name 0.
fn load(k: &[u8]) -> Option<HashMap<u32, CString>> {
    let d = digest(k);
    let data = std::fs::read(path(&d)?).ok()?;
    let rest = data.strip_prefix(b"WALCG2")?;
    if rest.get(..32)? != d {
        return None;
    }
    let mut at = 32;
    let mut strings = HashMap::new();
    while at < rest.len() {
        let name = u32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?);
        let len = u32::from_le_bytes(rest.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let s = CString::new(rest.get(at + 8..at + 8 + len)?.to_vec()).ok()?;
        strings.insert(name, s);
        at += 8 + len;
    }
    Some(strings)
}

fn save(k: &[u8], strings: &HashMap<u32, CString>) {
    let d = digest(k);
    let Some(path) = path(&d) else { return };
    let mut data = b"WALCG2".to_vec();
    data.extend_from_slice(&d);
    for (name, s) in strings {
        data.extend_from_slice(&name.to_le_bytes());
        data.extend_from_slice(&(s.as_bytes().len() as u32).to_le_bytes());
        data.extend_from_slice(s.as_bytes());
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &data).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Compiles the program for real (Cg's `cgCreateProgram`), its listing saved.
fn compile(s: &Source) -> (usize, Option<CString>) {
    let create: CreateProgram = unsafe { std::mem::transmute(CREATE.load(Ordering::Relaxed)) };
    let mut args: Vec<*const c_char> = s.args.iter().map(|a| a.as_ptr()).collect();
    args.push(std::ptr::null());
    let entry = s.entry.as_ref().map_or(std::ptr::null(), |e| e.as_ptr());
    let program = unsafe { create(s.context, s.kind, s.source.as_ptr(), s.profile, entry, args.as_ptr()) };
    let listing: GetLastListing = unsafe { std::mem::transmute(LISTING.load(Ordering::Relaxed)) };
    let listing = if LISTING.load(Ordering::Relaxed) != 0 { cstr(unsafe { listing(s.context) }) } else { None };
    (program, listing)
}

unsafe extern "C" fn create_program(context: usize, kind: u32, program: *const c_char, profile: u32, entry: *const c_char, args: *const *const c_char) -> usize {
    let Some(source) = cstr(program) else {
        let create: CreateProgram = unsafe { std::mem::transmute(CREATE.load(Ordering::Relaxed)) };
        return unsafe { create(context, kind, program, profile, entry, args) };
    };
    let mut list = Vec::new();
    if !args.is_null() {
        let mut i = 0;
        while let Some(a) = cstr(unsafe { *args.add(i) }) {
            list.push(a);
            i += 1;
        }
    }
    let source = Source { context, kind, source, profile, entry: cstr(entry), args: list };
    let k = key(&source);
    if let Some(strings) = load(&k) {
        let fake = NEXT_FAKE.fetch_add(2, Ordering::Relaxed);
        *LAST_LISTING.lock().unwrap() = strings.get(&0).cloned();
        LAST_CACHED.store(true, Ordering::Relaxed);
        PROGRAMS.lock().unwrap().push((fake, Program { key: k, source, real: 0, strings }));
        count(&HITS);
        return fake;
    }
    LAST_CACHED.store(false, Ordering::Relaxed);
    let (real, listing) = compile(&source);
    if real != 0 {
        let mut strings = HashMap::new();
        if let Some(l) = listing {
            strings.insert(0, l);
        }
        PROGRAMS.lock().unwrap().push((real, Program { key: k, source, real, strings }));
        count(&MISSES);
    }
    real
}

/// Every 500 programs: how many came from the cache.
fn count(which: &AtomicUsize) {
    which.fetch_add(1, Ordering::Relaxed);
    let (hits, misses) = (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed));
    if (hits + misses) % 500 == 0 {
        log!("cg_cache: {} programs, {hits} from the cache, {misses} compiled", hits + misses);
    }
}

unsafe extern "C" fn get_program_string(program: usize, name: u32) -> *const c_char {
    let real_get: GetProgramString = unsafe { std::mem::transmute(GET_STRING.load(Ordering::Relaxed)) };
    let mut programs = PROGRAMS.lock().unwrap();
    let Some((_, p)) = programs.iter_mut().find(|(h, _)| *h == program) else {
        drop(programs);
        return unsafe { real_get(program, name) };
    };
    if let Some(s) = p.strings.get(&name) {
        return s.as_ptr();
    }
    // a string not saved: from the real program (compiled now for a fake one), then saved
    if p.real == 0 {
        p.real = compile(&p.source).0;
        if p.real == 0 {
            return c"".as_ptr();
        }
    }
    let Some(s) = cstr(unsafe { real_get(p.real, name) }) else { return std::ptr::null() };
    p.strings.insert(name, s);
    save(&p.key, &p.strings);
    p.strings[&name].as_ptr()
}

unsafe extern "C" fn destroy_program(program: usize) {
    let removed = {
        let mut programs = PROGRAMS.lock().unwrap();
        programs.iter().position(|(h, _)| *h == program).map(|i| programs.remove(i).1)
    };
    let destroy: DestroyProgram = unsafe { std::mem::transmute(DESTROY.load(Ordering::Relaxed)) };
    match removed {
        Some(p) if p.real != 0 => unsafe { destroy(p.real) },
        Some(_) => {}
        None => unsafe { destroy(program) },
    }
}

unsafe extern "C" fn get_last_listing(context: usize) -> *const c_char {
    if LAST_CACHED.load(Ordering::Relaxed) {
        return LAST_LISTING.lock().unwrap().as_ref().map_or(std::ptr::null(), |l| l.as_ptr());
    }
    let listing: GetLastListing = unsafe { std::mem::transmute(LISTING.load(Ordering::Relaxed)) };
    unsafe { listing(context) }
}

unsafe extern "C" fn get_error() -> u32 {
    if LAST_CACHED.load(Ordering::Relaxed) {
        return 0;
    }
    let get: GetError = unsafe { std::mem::transmute(GET_ERROR.load(Ordering::Relaxed)) };
    unsafe { get() }
}
