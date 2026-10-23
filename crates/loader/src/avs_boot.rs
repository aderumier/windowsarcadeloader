//! `wal-avs-boot <game.dll> [args...]`
//!
//! The boot program of Konami's AVS games, whose code is a DLL (Road Fighters 3D: `jgt.dll`,
//! exports `dll_entry_init` / `dll_entry_main`) that the cabinet's launcher runs. Same steps
//! as the launcher (and as spice2x's, the open-source reference):
//!
//! 0. `kbt.dll` and `kld.dll` loaded when the game has them (boot checks: Road Fighters 3D
//!    calls kbt's `bt_get_ikey_status` through `GetModuleHandleA`).
//! 1. AVS (`libavs-win32.dll`) booted with `prop/avs-config.xml`: file systems (`/dev/nvram`,
//!    `/dev/raw`), threads, the log.
//! 2. `libavs-win32-ea3.dll` and the game DLL loaded (its `libacio.dll` import: the payload).
//! 3. The e-Amusement identity of `prop/eamuse-config.xml` (`ea3-config.xml` when there is one)
//!    set in the AVS environment (`/env/profile/...`: security code, PCB id, soft id code).
//! 4. `dll_entry_init(<model><dest><spec><rev><ext>, prop/app-config.xml's /param)`.
//! 5. `ea3_boot` (e-Amusement: offline without Konami's servers, the game boots on).
//! 6. `dll_entry_main()`: the game, until it exits; then `ea3_shutdown`, `avs_shutdown`.
//!
//! Run from the game directory. A game whose DLLs are in `modules/` (Winning Eleven 2012:
//! `contents/modules/`, the cabinet's layout) runs from the directory above, which holds
//! `prop/`, `data/` and `dev/`: the boot is started in `modules/` (beside the DLLs, which load
//! from the program's directory) and moves up. `WAL_KONAMI_AVS_LOG=1`: the AVS log in
//! `avs.log` there. `WAL_KONAMI_AVS_HEAP_MB`: the size of each AVS heap (16 MB by default;
//! Winning Eleven 2012 needs 30). `WAL_KONAMI_DEST`: the destination (region letter) in place of
//! the dump's `/ea3/soft/dest` (Winning Eleven 2012: `J` Japan, `A` Asia, `C` China).

#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::{CString, c_void};
use std::io::Write;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

const NODE_TYPE_S32: u32 = 6;
const NODE_TYPE_STR: u32 = 11;
const NODE_TYPE_BOOL: u32 = 52;
const HEAP_SIZE_MB: usize = 16;

type Reader = unsafe extern "C" fn(u32, *mut u8, usize) -> i32;
type LogWriter = unsafe extern "C" fn(*mut c_void, *const u8, u32) -> u32;

/// The AVS functions the boot uses (cdecl).
struct Avs {
    boot: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_void, usize, LogWriter, *mut c_void),
    shutdown: unsafe extern "C" fn(),
    read_query_memsize: unsafe extern "C" fn(Reader, u32, *mut c_void, *mut c_void) -> i32,
    create: unsafe extern "C" fn(u32, *mut c_void, u32) -> *mut c_void,
    insert_read: unsafe extern "C" fn(*mut c_void, *mut c_void, Reader, u32) -> i32,
    search: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8) -> *mut c_void,
    node_refer: unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, u32, *mut u8, u32) -> i32,
    node_remove: unsafe extern "C" fn(*mut c_void) -> i32,
    // varargs: (property, node, type, path, value); the value is a string pointer or an integer
    node_create: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *const u8, usize) -> *mut c_void,
    setenv: unsafe extern "C" fn(*const u8, *const u8) -> i32,
}

fn proc(module: HMODULE, dll: &str, name: &str) -> usize {
    let c = CString::new(name).unwrap();
    match unsafe { GetProcAddress(module, c.as_ptr().cast()) } {
        Some(f) => f as usize,
        None => fail(&format!("{dll}: {name} missing")),
    }
}

fn load(dll: &str) -> HMODULE {
    let c = CString::new(dll).unwrap();
    let module = unsafe { LoadLibraryA(c.as_ptr().cast()) };
    if module.is_null() {
        fail(&format!("{dll}: cannot load (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() }));
    }
    module
}

fn fail(msg: &str) -> ! {
    eprintln!("wal-avs-boot: {msg}");
    std::process::exit(1);
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// The XML file being read by the AVS property reader.
static READING: Mutex<(Vec<u8>, usize)> = Mutex::new((Vec::new(), 0));

unsafe extern "C" fn reader(_ctx: u32, buf: *mut u8, count: usize) -> i32 {
    let mut guard = READING.lock().unwrap();
    let (data, pos) = &mut *guard;
    let n = count.min(data.len() - *pos);
    unsafe { std::ptr::copy_nonoverlapping(data[*pos..].as_ptr(), buf, n) };
    *pos += n;
    n as i32
}

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

unsafe extern "C" fn log_writer(_ctx: *mut c_void, chars: *const u8, n: u32) -> u32 {
    if let Some(file) = LOG.lock().unwrap().as_mut()
        && !chars.is_null()
    {
        let bytes = unsafe { std::slice::from_raw_parts(chars, n as usize) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let _ = file.write_all(&bytes[..end]);
        if !bytes[..end].ends_with(b"\n") {
            let _ = file.write_all(b"\n");
        }
    }
    n
}

/// The AVS functions' export names: plain in the older AVS 2 builds; AVS 2.13 (Winning Eleven
/// 2012) exports them as numbered `XC058ba5...` names (spice2x's table for that version).
struct AvsNames {
    boot: &'static str,
    shutdown: &'static str,
    read_query_memsize: &'static str,
    create: &'static str,
    insert_read: &'static str,
    search: &'static str,
    node_refer: &'static str,
    node_remove: &'static str,
    node_create: &'static str,
    setenv: &'static str,
    ea3_boot: &'static str,
    ea3_shutdown: &'static str,
}

const AVS_LEGACY: AvsNames = AvsNames {
    boot: "avs_boot",
    shutdown: "avs_shutdown",
    read_query_memsize: "property_read_query_memsize",
    create: "property_create",
    insert_read: "property_insert_read",
    search: "property_search",
    node_refer: "property_node_refer",
    node_remove: "property_node_remove",
    node_create: "property_node_create",
    setenv: "std_setenv",
    ea3_boot: "ea3_boot",
    ea3_shutdown: "ea3_shutdown",
};

const AVS_2_13: AvsNames = AvsNames {
    boot: "XC058ba50000f4",
    shutdown: "XC058ba5000154",
    read_query_memsize: "XC058ba5000066",
    create: "XC058ba5000107",
    insert_read: "XC058ba5000016",
    search: "XC058ba50000fb",
    node_refer: "XC058ba5000113",
    node_remove: "XC058ba5000085",
    node_create: "XC058ba5000143",
    setenv: "XC058ba5000075",
    ea3_boot: "XEb552d500005d",
    ea3_shutdown: "XEb552d5000060",
};

impl AvsNames {
    /// The names of this libavs-win32.dll's build.
    fn of(m: HMODULE) -> &'static AvsNames {
        if unsafe { GetProcAddress(m, c"avs_boot".as_ptr().cast()) }.is_some() { &AVS_LEGACY } else { &AVS_2_13 }
    }
}

impl Avs {
    fn new(m: HMODULE, n: &AvsNames) -> Avs {
        let d = "libavs-win32.dll";
        unsafe {
            Avs {
                boot: std::mem::transmute(proc(m, d, n.boot)),
                shutdown: std::mem::transmute(proc(m, d, n.shutdown)),
                read_query_memsize: std::mem::transmute(proc(m, d, n.read_query_memsize)),
                create: std::mem::transmute(proc(m, d, n.create)),
                insert_read: std::mem::transmute(proc(m, d, n.insert_read)),
                search: std::mem::transmute(proc(m, d, n.search)),
                node_refer: std::mem::transmute(proc(m, d, n.node_refer)),
                node_remove: std::mem::transmute(proc(m, d, n.node_remove)),
                node_create: std::mem::transmute(proc(m, d, n.node_create)),
                setenv: std::mem::transmute(proc(m, d, n.setenv)),
            }
        }
    }

    /// An XML file as a property (with `extra` bytes free for new nodes).
    fn read(&self, path: &str, extra: usize) -> *mut c_void {
        let data = std::fs::read(path).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
        *READING.lock().unwrap() = (data, 0);
        let size = unsafe { (self.read_query_memsize)(reader, 0, std::ptr::null_mut(), std::ptr::null_mut()) };
        if size <= 0 {
            fail(&format!("{path}: not a property file ({size:#x})"));
        }
        let size = size as usize + extra;
        let buf = Box::leak(vec![0u8; size].into_boxed_slice());
        let prop = unsafe { (self.create)(23, buf.as_mut_ptr().cast(), size as u32) };
        if prop.is_null() {
            fail(&format!("{path}: property_create failed"));
        }
        READING.lock().unwrap().1 = 0;
        if unsafe { (self.insert_read)(prop, std::ptr::null_mut(), reader, 0) } == 0 {
            fail(&format!("{path}: property_insert_read failed"));
        }
        prop
    }

    fn node(&self, prop: *mut c_void, path: &str) -> *mut c_void {
        unsafe { (self.search)(prop, std::ptr::null_mut(), c(path).as_ptr().cast()) }
    }

    fn str(&self, prop: *mut c_void, path: &str) -> String {
        let mut buf = [0u8; 64];
        let n = unsafe {
            (self.node_refer)(prop, std::ptr::null_mut(), c(path).as_ptr().cast(), NODE_TYPE_STR, buf.as_mut_ptr(), buf.len() as u32)
        };
        if n <= 0 {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    /// Sets `path` (removed first when there).
    fn set(&self, prop: *mut c_void, path: &str, kind: u32, value: usize) {
        let node = self.node(prop, path);
        if !node.is_null() {
            unsafe { (self.node_remove)(node) };
        }
        unsafe { (self.node_create)(prop, std::ptr::null_mut(), kind, c(path).as_ptr().cast(), value) };
    }

    fn set_str(&self, prop: *mut c_void, path: &str, value: &str) {
        let v = c(value);
        self.set(prop, path, NODE_TYPE_STR, v.as_ptr() as usize);
    }

    fn setenv(&self, name: &str, value: &str) {
        unsafe { (self.setenv)(c(name).as_ptr().cast(), c(value).as_ptr().cast()) };
    }
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

fn main() {
    let game = std::env::args().nth(1).unwrap_or_else(|| fail("usage: wal-avs-boot <game.dll> [args...]"));
    // the game's DLLs in modules/, its files in the directory above
    if !exists("prop/avs-config.xml") && exists("../prop/avs-config.xml") {
        std::env::set_current_dir("..").unwrap_or_else(|e| fail(&format!("..: {e}")));
    }
    let heap_size = std::env::var("WAL_KONAMI_AVS_HEAP_MB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(HEAP_SIZE_MB) * 1024 * 1024;
    if std::env::var("WAL_KONAMI_AVS_LOG").is_ok_and(|v| v.trim() == "1") {
        *LOG.lock().unwrap() = std::fs::File::create("avs.log").ok();
    }

    // the cabinet's boot check libraries, which the game finds loaded (`GetModuleHandleA`)
    for dll in ["kbt.dll", "kld.dll"] {
        if exists(dll) {
            load(dll);
        }
    }

    // 1. AVS
    let avs_dll = load("libavs-win32.dll");
    let names = AvsNames::of(avs_dll);
    let avs = Avs::new(avs_dll, names);
    let config = avs.read("prop/avs-config.xml", 0x1000);
    let config_node = avs.node(config, "/config");
    if config_node.is_null() {
        fail("prop/avs-config.xml: no /config");
    }
    // the e-Amusement time server off (no network: the boot would wait for it)
    avs.set(config, "/config/sntp/ea_on", NODE_TYPE_BOOL, 0);
    // the PC's time zone (the cabinet's is Tokyo's)
    let offset = unsafe {
        let mut tz: windows_sys::Win32::System::Time::TIME_ZONE_INFORMATION = std::mem::zeroed();
        windows_sys::Win32::System::Time::GetTimeZoneInformation(&mut tz);
        -tz.Bias * 60
    };
    avs.set(config, "/config/time/gmt_offset", NODE_TYPE_S32, offset as usize);
    let heap1 = Box::leak(vec![0u8; heap_size].into_boxed_slice());
    let heap2 = Box::leak(vec![0u8; heap_size].into_boxed_slice());
    unsafe {
        (avs.boot)(config_node, heap1.as_mut_ptr().cast(), heap_size, heap2.as_mut_ptr().cast(), heap_size, log_writer, std::ptr::null_mut())
    };

    // 2. e-Amusement library, the game
    let ea3_dll = load("libavs-win32-ea3.dll");
    let ea3_boot: unsafe extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(proc(ea3_dll, "libavs-win32-ea3.dll", names.ea3_boot)) };
    let ea3_shutdown: unsafe extern "C" fn() = unsafe { std::mem::transmute(proc(ea3_dll, "libavs-win32-ea3.dll", names.ea3_shutdown)) };
    let game_dll = load(&game);
    let entry_init: unsafe extern "C" fn(*mut u8, *mut c_void) -> bool = unsafe { std::mem::transmute(proc(game_dll, &game, "dll_entry_init")) };
    let entry_main: unsafe extern "C" fn() -> bool = unsafe { std::mem::transmute(proc(game_dll, &game, "dll_entry_main")) };

    // 3. identity
    let ea3_path = ["prop/ea3-config.xml", "prop/ea3-cfg.xml", "prop/eamuse-config.xml"]
        .into_iter()
        .find(|p| exists(p))
        .unwrap_or_else(|| fail("no prop/ea3-config.xml or prop/eamuse-config.xml"));
    let ea3 = avs.read(ea3_path, 0x40000);
    let mut soft: Vec<String> = ["model", "dest", "spec", "rev", "ext"].iter().map(|k| avs.str(ea3, &format!("/ea3/soft/{k}"))).collect();
    if let Some(dest) = std::env::var("WAL_KONAMI_DEST").ok().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) {
        avs.set_str(ea3, "/ea3/soft/dest", &dest);
        soft[1] = dest;
    }
    let [model, dest, spec, rev, ext] = [&soft[0], &soft[1], &soft[2], &soft[3], &soft[4]];
    if model.is_empty() {
        fail(&format!("{ea3_path}: no /ea3/soft/model"));
    }
    let mut pcbid = avs.str(ea3, "/ea3/id/pcbid");
    if pcbid.is_empty() {
        pcbid = "01201000000000010101".into();
        avs.set_str(ea3, "/ea3/id/pcbid", &pcbid);
    }
    let mut hardid = avs.str(ea3, "/ea3/id/hardid");
    if hardid.is_empty() {
        hardid = "0100DEADBEEF".into();
        avs.set_str(ea3, "/ea3/id/hardid", &hardid);
    }
    let mut softid = avs.str(ea3, "/ea3/id/softid");
    if softid.is_empty() {
        softid = "012199999999".into();
        avs.set_str(ea3, "/ea3/id/softid", &softid);
    }
    let mut accountid = avs.str(ea3, "/ea3/id/accountid");
    if accountid.is_empty() {
        accountid = "012018008135".into();
        avs.set_str(ea3, "/ea3/id/accountid", &accountid);
    }
    let security_code = format!("G*{model}{dest}{spec}{rev}");
    let soft_id_code = format!("{model}:{dest}:{spec}:{rev}:{ext}");
    for (name, value) in [
        ("/env/boot/build", "windows-arcade-loader"),
        ("/env/boot/version", "windows-arcade-loader"),
        ("/env/boot/tag", "windows-arcade-loader"),
        ("/env/profile/security_code", &security_code),
        ("/env/profile/secplug_b_security_code", &security_code),
        ("/env/profile/system_id", &pcbid),
        ("/env/profile/hardware_id", &hardid),
        ("/env/profile/license_id", &softid),
        ("/env/profile/software_id", &softid),
        ("/env/profile/account_id", &accountid),
        ("/env/profile/soft_id_code", &soft_id_code),
    ] {
        avs.setenv(name, value);
    }

    // 4. the game's init
    let app = if exists("prop/app-config.xml") { avs.read("prop/app-config.xml", 0x1000) } else { std::ptr::null_mut() };
    let param = if app.is_null() { std::ptr::null_mut() } else { avs.node(app, "/param") };
    let mut init_code = format!("{model}{dest}{spec}{rev}{ext}\0").into_bytes();
    if !unsafe { entry_init(init_code.as_mut_ptr(), param) } {
        eprintln!("wal-avs-boot: dll_entry_init failed");
    }

    // 5. e-Amusement
    unsafe { ea3_boot(avs.node(ea3, "/ea3")) };

    // 6. the game
    let ok = unsafe { entry_main() };
    unsafe {
        ea3_shutdown();
        (avs.shutdown)();
    }
    std::process::exit(if ok { 0 } else { 1 });
}
