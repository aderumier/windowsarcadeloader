//! Beach Head 2000's cabinet: its frontend `UniverShell.exe` (a web browser showing the shell's
//! HTML pages, scripted through the `ShellControl.ocx` ActiveX control: attract, coins, high
//! scores, operator menus) starting the game, `Beach16.exe`, for each play. Beach Head 2002's and
//! Beach Head 2003 Desert War's the same, with the multigame shell (its control a later version)
//! and `BH2.exe` / `desertwar.exe`.
//!
//! The controls of both: `beachhead_io.rs` (Beach Head 2002's through `GVRInputRaw.dll`,
//! `turret.rs`); the shell's state kept between launches: `beachhead_save.rs`.
//!
//! In the frontend:
//! * the ActiveX controls Wine lacks, registered once its OLE is up (`amovie.rs`);
//! * the multigame shell's licenses (its control's check of the dongle and of the codes the
//!   cabinet's setup stored, for each game) all valid;
//! * the game started with the payload: the shell control's `CreateProcessA` runs it through
//!   `wal-loader` (installed next to the payload), which waits for it and exits with its exit
//!   code (the shell reads the player's score there);
//! * the pages as Internet Explorer read them: an `<object>` in a page's `<head>` (its HTML Help
//!   "close" control, the shell's control) or text between its head and its body (the broken
//!   style block of the attract pages) starts the body there for Wine's HTML parser, and the
//!   page's own `<body onload=...>` then never registers its handler (the shell's whole logic
//!   starts there). The pages are served with their body tag right after the head, these objects
//!   and that text after it (urlmon's file reads); with the other differences of Wine's browser
//!   their scripts meet (see [`fix_page`]).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, LoadLibraryW};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

static ORIG_OLE_INITIALIZE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);
/// The payload's module (its folder holds `wal-loader.exe`).
static PAYLOAD: AtomicUsize = AtomicUsize::new(0);

type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, usize, u32, u32, usize) -> usize;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// The page as Internet Explorer built it: its `<body ...>` tag right after `</head>`, followed
/// by the `<object>` elements of its head and what came between the head and the body; `None`
/// when nothing precedes the body tag but its head without objects.
fn fix_body(page: &[u8]) -> Option<Vec<u8>> {
    let lower = page.to_ascii_lowercase();
    let find = |from: usize, what: &[u8]| lower[from..].windows(what.len()).position(|w| w == what).map(|p| p + from);
    let body = find(0, b"<body")?;
    let body_end = find(body, b">")? + 1;
    let head_end = find(0, b"</head>").filter(|&h| h < body);
    let head_stop = head_end.unwrap_or(body);
    let mut head = Vec::new();
    let mut objects = Vec::new();
    let mut at = 0;
    while let Some(start) = find(at, b"<object").filter(|&s| s < head_stop) {
        let end = find(start, b"</object>")? + b"</object>".len();
        if end > head_stop {
            return None;
        }
        head.extend_from_slice(&page[at..start]);
        objects.extend_from_slice(&page[start..end]);
        at = end;
    }
    let between = head_end.map_or(&[][..], |h| &page[h + b"</head>".len()..body]);
    if objects.is_empty() && between.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    head.extend_from_slice(&page[at..head_stop]);
    let mut out = head;
    if head_end.is_some() {
        out.extend_from_slice(b"</head>");
    }
    out.extend_from_slice(&page[body..body_end]);
    out.extend_from_slice(&objects);
    out.extend_from_slice(between);
    out.extend_from_slice(&page[body_end..]);
    Some(out)
}

/// A page script's global `var name`: Internet Explorer made it a script variable over the
/// window's `name`, Wine's script engine assigns the window's (a string): the High Score page's
/// `name[i]` were all undefined. Renamed `nAme`.
fn rename_name(page: &[u8]) -> Option<Vec<u8>> {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    // the identifier `name` at `i`: not part of a longer word nor a property (`x.name`), and what
    // follows it
    let identifier = |i: usize| -> Option<&[u8]> {
        let before_ok = i == 0 || !(is_word(page[i - 1]) || page[i - 1] == b'.');
        let after = &page[i + 4..];
        (page[i..].starts_with(b"name") && before_ok && after.first().is_none_or(|&b| !is_word(b)))
            .then(|| &after[after.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(after.len())..])
    };
    let declared = (4..page.len().saturating_sub(4)).any(|i| {
        identifier(i).is_some() && page[..i].trim_ascii_end().ends_with(b"var") && page[i - 1].is_ascii_whitespace()
    });
    if !declared {
        return None;
    }
    let mut out = page.to_vec();
    for i in 0..page.len().saturating_sub(4) {
        // an indexing, or an assignment (not an HTML attribute `name="..."` nor a comparison)
        let renamed = match identifier(i) {
            Some([b'[', ..]) => true,
            Some([b'=', next, ..]) => !matches!(next, b'"' | b'\'' | b'='),
            _ => false,
        };
        if renamed {
            out[i + 1] = b'A';
        }
    }
    Some(out)
}

/// Page texts changed:
/// * Beach Head 2000's operator menu frames its navigation fills
///   (`frames.menu.navigate("machine.html")`): empty, Wine resolves that page against their
///   `about:blank` (Internet Explorer against the frame set's page) and shows "machine.html".
///   Given their first page as their source.
/// * Beach Head 2002's operator menu frames: its heading frame is also named "nav", the pages'
///   `window.parent.frames.nav` (the navigation frame, their scripts' shell) is the first of the
///   two for Wine's browser. Renamed "heading". Its help frame is empty as Beach Head 2000's (given
///   the stand-up cabinet's help page), whose page there, `helpSX.html`, is missing from the game:
///   its `helpPup.html`.
/// * Beach Head 2002's black screen page, which starts the game from its update timer then goes
///   to the high score page: Internet Explorer stopped its timers there, Wine's run once more
///   before the next page (the game started again). It starts the game once.
/// * Beach Head 2002's and Desert War's difficulty page, which picks its row from the
///   trackball's position since the page opened (a band of 30 per row, Easy first): the virtual
///   stick's steps (the shell's [`crate::turret`]) overshoot it. A step moves one row, kept on
///   the rows.
const TEXT_FIXES: &[(&[u8], &[u8])] = &[
    (b"<FRAME\t\t\t\t\t\tNAME=\"menu\"", b"<FRAME SRC=\"machine.html\" NAME=\"menu\""),
    (b"<FRAME\t\t\t\t\t\tNAME=\"help\"", b"<FRAME SRC=\"help.html\" NAME=\"help\""),
    (b"<FRAME SRC=\"heading.html\" NAME=\"nav\" ID=\"nav\"", b"<FRAME SRC=\"heading.html\" NAME=\"heading\" ID=\"heading\""),
    (b"<FRAME            NAME=\"help\"", b"<FRAME SRC=\"helpPup.html\" NAME=\"help\""),
    (b"help.navigate(\"helpSX.html\")", b"help.navigate(\"helpPup.html\")"),
    (b"var larsdelayed= 0;", b"var larsdelayed= 0, walStarted = 0;"),
    (b"\tif(larsdelayed==1)\r\n", b"\tif(larsdelayed==1 && !walStarted++)\r\n"),
    (
        b"function customPostUpdate()\r\n{\r\n  var item;\r\n  var itemu;\r\n\tif((menuMouseY > 0) && (menuMouseY<30))",
        b"var walY = 0;\r\nfunction customPostUpdate()\r\n{\r\n  var item;\r\n  var itemu;\r\n\
          \tif(menuMouseY != walY) {\r\n\
          \t\tNewMenu = Math.max(0, Math.min(MaxDiff - 1, NewMenu + (menuMouseY > walY ? 1 : -1)));\r\n\
          \t\tmenuMouseY = walY = NewMenu * 30 + 15;\r\n\
          \t}\r\n\
          \tif((menuMouseY > 0) && (menuMouseY<30))",
    ),
];

fn replace_text(page: &[u8]) -> Option<Vec<u8>> {
    let mut out = page.to_vec();
    let mut changed = false;
    for (from, to) in TEXT_FIXES {
        if let Some(at) = out.windows(from.len()).position(|w| w == *from) {
            out.splice(at..at + from.len(), to.iter().copied());
            changed = true;
        }
    }
    changed.then_some(out)
}

/// The attract pages' `delete movieElem;` (a global `var`, once their movie stopped): Internet
/// Explorer kept the variable, Wine's script engine removes it, the pages' update timer then
/// fails on it ("undefined") and never leaves the page (a coin dropped in the attract movie).
/// Made a plain statement.
fn keep_movie(page: &[u8]) -> Option<Vec<u8>> {
    const FROM: &[u8] = b"delete movieElem;";
    const TO: &[u8] = b"movieElem;";
    let mut out = Vec::with_capacity(page.len());
    let mut rest = page;
    while let Some(at) = rest.windows(FROM.len()).position(|w| w == FROM) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(TO);
        rest = &rest[at + FROM.len()..];
    }
    (!out.is_empty()).then(|| [out.as_slice(), rest].concat())
}

/// The page fixed for Wine's browser: [`fix_body`], [`rename_name`], [`replace_text`],
/// [`keep_movie`].
fn fix_page(page: &[u8]) -> Option<Vec<u8>> {
    let mut fixed: Option<Vec<u8>> = None;
    for fix in [fix_body, rename_name, replace_text, keep_movie] {
        if let Some(f) = fix(fixed.as_deref().unwrap_or(page)) {
            fixed = Some(f);
        }
    }
    fixed
}

/// urlmon's `CreateFileW`: a shell page [`fix_page`] changes, or script [`keep_movie`] changes, is
/// served from a fixed copy.
unsafe extern "system" fn create_file(name: *const u16, access: u32, share: u32, sa: usize, disposition: u32, flags: u32, template: usize) -> usize {
    let orig: CreateFileW = unsafe { std::mem::transmute(ORIG_CREATE_FILE.load(Ordering::Relaxed)) };
    let path = wide_str(name);
    let lower = path.to_ascii_lowercase();
    let fix: Option<fn(&[u8]) -> Option<Vec<u8>>> = if lower.ends_with(".html") || lower.ends_with(".htm") {
        Some(fix_page)
    } else if lower.ends_with(".js") {
        Some(keep_movie)
    } else {
        None
    };
    if let Some(fix) = fix
        && let Ok(page) = std::fs::read(&path)
        && let Some(fixed) = fix(&page)
    {
        // a copy per read (the browser reads a page twice, the first copy still open), deleted
        // once read
        static COPIES: AtomicUsize = AtomicUsize::new(0);
        let file = path.rsplit(['\\', '/']).next().unwrap_or("page.html");
        let n = COPIES.fetch_add(1, Ordering::Relaxed);
        let copy = std::env::temp_dir().join(format!("wal-shell-{}-{n}-{file}", std::process::id()));
        if std::fs::write(&copy, fixed).is_ok() {
            log!("beachhead: {path}: fixed");
            let w = wide(&copy.to_string_lossy());
            const FILE_SHARE_DELETE: u32 = 4;
            const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
            return unsafe {
                orig(w.as_ptr(), access, share | FILE_SHARE_DELETE, sa, disposition, flags | FILE_FLAG_DELETE_ON_CLOSE, template)
            };
        }
    }
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

fn c_str(p: *const u8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

/// The payload's own path.
fn payload_path() -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(PAYLOAD.load(Ordering::Relaxed) as HINSTANCE, buf.as_mut_ptr(), 260) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The game's command line through `wal-loader`, from the shell's `CreateProcessA(app, cmd)`:
/// `None` for another program.
fn loader_command(app: &str, cmd: &str, payload: &str) -> Option<String> {
    let exe = if app.is_empty() { cmd.split_whitespace().next().unwrap_or("").trim_matches('"') } else { app };
    let lower = exe.to_ascii_lowercase();
    if !["beach16.exe", "bh2.exe", "desertwar.exe"].iter().any(|game| lower.ends_with(game)) {
        return None;
    }
    // the arguments: the command line without the program when it starts with it
    let args = cmd.trim_start();
    let args = match args.strip_prefix('"') {
        Some(quoted) if quoted.to_ascii_lowercase().starts_with(&exe.to_ascii_lowercase()) => quoted[exe.len()..].trim_start_matches('"'),
        _ if args.to_ascii_lowercase().starts_with(&exe.to_ascii_lowercase()) => &args[exe.len()..],
        _ => args,
    };
    let dir = payload.rsplit_once('\\').map_or("", |(d, _)| d);
    Some(format!("\"{dir}\\wal-loader.exe\" \"{payload}\" \"{}\" {}", exe.replace('/', "\\"), args.trim()))
}

/// The shell control's `CreateProcessA`: the game started through `wal-loader`, in its own folder.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u8,
    cmd: *mut u8,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u8,
    si: usize,
    pi: usize,
) -> i32 {
    type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, usize, usize, i32, u32, usize, *const u8, usize, usize) -> i32;
    let orig: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s, dir_s) = (c_str(app), c_str(cmd), c_str(dir));
    log!("beachhead: the shell starts {app_s:?} {cmd_s:?} in {dir_s:?}");
    let Some(command) = loader_command(&app_s, &cmd_s, &payload_path()) else {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
    };
    let exe = if app_s.is_empty() { cmd_s.split_whitespace().next().unwrap_or("").trim_matches('"').to_string() } else { app_s };
    let folder = if dir_s.is_empty() { exe.replace('/', "\\").rsplit_once('\\').map_or(String::new(), |(d, _)| d.to_string()) } else { dir_s };
    log!("beachhead: {command} (in {folder})");
    let mut command = std::ffi::CString::new(command).unwrap_or_default().into_bytes_with_nul();
    let folder = std::ffi::CString::new(folder).unwrap_or_default();
    let ok = unsafe { orig(std::ptr::null(), command.as_mut_ptr(), pa, ta, inherit, flags, env, folder.as_ptr().cast(), si, pi) };
    if ok == 0 {
        log!("beachhead: the game did not start (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
    }
    ok
}

/// The multigame shell control's license check, `int check(const char *game)` (`MgWtf`, the
/// script's, returns it; `ExecDir` runs the game only when it is 1, else disables it): 1,
/// `MC_MATCHED` (the game's code found and matching the dongle).
unsafe extern "C" fn license(game: *const u8) -> i32 {
    static LOGGED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let game = c_str(game);
    if let Ok(mut logged) = LOGGED.lock()
        && !logged.contains(&game)
    {
        log!("beachhead: license of {game:?} valid");
        logged.push(game);
    }
    1
}

/// The address an incremental linking thunk (`jmp rel32`) at `at` jumps to, `at` when not one.
fn follow_jump(at: usize) -> usize {
    if unsafe { *(at as *const u8) } == 0xE9 { at + 5 + unsafe { ((at + 1) as *const i32).read_unaligned() } as usize } else { at }
}

/// The license check of the shell control `module`: the function its `MgWtf` dispatch method
/// (found in its MFC dispatch map: name, DISPID_UNKNOWN, parameters, return type, function)
/// calls first, through its thunk. `None` without one (Beach Head 2000's control).
fn license_thunk(module: usize) -> Option<usize> {
    let size = unsafe {
        let nt = module + ((module + 0x3c) as *const u32).read_unaligned() as usize;
        ((nt + 0x50) as *const u32).read_unaligned() as usize
    };
    let image = unsafe { std::slice::from_raw_parts(module as *const u8, size) };
    let name = image.windows(6).position(|w| w == b"MgWtf\0")?;
    let word = |i: usize| u32::from_le_bytes([image[i], image[i + 1], image[i + 2], image[i + 3]]) as usize;
    let entry = (0..size.saturating_sub(20)).step_by(4).find(|&i| word(i) == module + name && word(i + 4) == u32::MAX as usize)?;
    let method = follow_jump(word(entry + 16));
    // its first call to a thunk (the prologue only fills its frame)
    (method..method + 0x60).find_map(|at| {
        if unsafe { *(at as *const u8) } != 0xE8 {
            return None;
        }
        let target = at + 5 + unsafe { ((at + 1) as *const i32).read_unaligned() } as usize;
        (target > module && target < module + size && unsafe { *(target as *const u8) } == 0xE9).then_some(target)
    })
}

/// The thunk at `at` made a jump to `function`.
fn patch_jump(at: usize, function: usize) {
    unsafe {
        let mut old = 0;
        VirtualProtect(at as _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        ((at + 1) as *mut i32).write_unaligned(function.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as _, 5, old, &mut old);
    }
}

/// `OleInitialize`: the frontend's OLE, then the pages' controls and their reads, the shell
/// control's controls and game start.
unsafe extern "system" fn ole_initialize(reserved: usize) -> i32 {
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(ORIG_OLE_INITIALIZE.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(reserved) };
    crate::beachhead_save::init();
    crate::amovie::register();
    let urlmon = unsafe { LoadLibraryW(wide("urlmon.dll").as_ptr()) } as usize;
    if urlmon != 0
        && let Some(o) = unsafe { iat::hook_module(urlmon, "kernel32.dll", "CreateFileW", create_file as *const () as usize) }
    {
        ORIG_CREATE_FILE.store(o, Ordering::Relaxed);
    }
    // the control the pages create later (the same module)
    let control = unsafe { LoadLibraryW(wide("C:\\Orion\\ShellControl.ocx").as_ptr()) } as usize;
    if control == 0 {
        log!("beachhead: no C:\\Orion\\ShellControl.ocx");
        return hr;
    }
    crate::beachhead_io::hook(control);
    if let Some(thunk) = license_thunk(control) {
        patch_jump(thunk, license as *const () as usize);
        log!("beachhead: multigame shell licenses (check at {thunk:#x})");
    }
    if let Some(o) = unsafe { iat::hook_module(control, "kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
        ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
    }
    hr
}

pub(crate) fn init(module: HINSTANCE) {
    PAYLOAD.store(module as usize, Ordering::Relaxed);
    match crate::frontend_fit::exe_name().as_str() {
        "univershell.exe" => {
            if let Some(o) = unsafe { iat::hook("ole32.dll", "OleInitialize", ole_initialize as *const () as usize) } {
                ORIG_OLE_INITIALIZE.store(o, Ordering::Relaxed);
            }
            crate::beachhead_io::filter_messages();
            crate::beachhead_io::watch_coins();
            log!("beachhead: frontend");
        }
        "beach16.exe" | "bh2.exe" | "desertwar.exe" => {
            let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
            crate::beachhead_io::hook(exe);
            crate::beachhead_io::filter_messages();
            log!("beachhead: game");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_name_renamed() {
        let page = b"<script>var\tname = new Array();\nname[0] = x.name; f(name[1]);</script><input name=\"a\">";
        let fixed = String::from_utf8(rename_name(page).unwrap()).unwrap();
        assert_eq!(fixed, "<script>var\tnAme = new Array();\nnAme[0] = x.name; f(nAme[1]);</script><input name=\"a\">");
        assert!(rename_name(b"<input name=\"a\"> var x = y.name;").is_none());
    }

    #[test]
    fn game_through_loader() {
        let payload = "C:\\Orion\\USBIOExtreme.dll";
        let expected = "\"C:\\Orion\\wal-loader.exe\" \"C:\\Orion\\USBIOExtreme.dll\" \"C:\\Orion\\BH2K\\Beach16.exe\" console conval 0";
        assert_eq!(loader_command("C:/Orion/BH2K/Beach16.exe", "console conval 0", payload).unwrap(), expected);
        assert_eq!(loader_command("", "\"C:/Orion/BH2K/Beach16.exe\" console conval 0", payload).unwrap(), expected);
        assert_eq!(loader_command("C:/Orion/BH2K/Beach16.exe", "C:/Orion/BH2K/Beach16.exe console conval 0", payload).unwrap(), expected);
        assert_eq!(
            loader_command("C:/Orion/bh2002/BH2.exe", "-console -freeplay", payload).unwrap(),
            "\"C:\\Orion\\wal-loader.exe\" \"C:\\Orion\\USBIOExtreme.dll\" \"C:\\Orion\\bh2002\\BH2.exe\" -console -freeplay"
        );
        assert!(loader_command("C:/Windows/notepad.exe", "", payload).is_none());
    }

    #[test]
    fn body_start_fixed() {
        let fix_page = fix_body;
        let page = b"<html><head><object id=a><param></object>\n<script>x</script></head>\n<style>s</style>\n-->\n<body onload='f()'>text</body>";
        let fixed = String::from_utf8(fix_page(page).unwrap()).unwrap();
        assert_eq!(
            fixed,
            "<html><head>\n<script>x</script></head><body onload='f()'><object id=a><param></object>\n<style>s</style>\n-->\ntext</body>"
        );
        assert!(fix_page(b"<html><head><title>t</title></head>\n<body><object></object></body>").is_none());
    }

    #[test]
    fn movie_kept() {
        let fixed = keep_movie(b"delete movieElem;\r\n\tif(x)\r\n\t\tdelete movieElem;").unwrap();
        assert_eq!(fixed, b"movieElem;\r\n\tif(x)\r\n\t\tmovieElem;");
        assert!(keep_movie(b"movieElem.Stop();").is_none());
    }
}
