//! 32-bit Namco ES3 support, installed as the game's Banapass reader (`bngrw.dll`).
//! Mario Kart GP DX reads its wheel and buttons through DirectInput, so the same module also
//! supplies the launcher's virtual wheel and reports the game's memory based force feedback.

#![allow(non_snake_case)]

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_protocol::{button, output};

mod amcus;
mod jvs;
mod ipc;
mod netlog;
mod profile;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc, VirtualProtect};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

static CARD_CONSUMED: AtomicBool = AtomicBool::new(false);
static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

type AttachCallback = unsafe extern "C" fn(i32, i32, *mut isize) -> isize;
type ResetCallback = unsafe extern "C" fn(i32, i32, *mut isize) -> isize;
type TouchCallback = unsafe extern "C" fn(i32, i32, *mut c_void, *mut c_void);

fn game_base() -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MkdxVersion { V11022, V11816 }

impl MkdxVersion {
    fn name(self) -> &'static str {
        match self { Self::V11022 => "v1.10.22", Self::V11816 => "v1.18.16" }
    }
}

/// PE build identity is checked before any version-specific code or data address is used.
pub(crate) fn game_version() -> Option<MkdxVersion> {
    static VERSION: std::sync::OnceLock<Option<MkdxVersion>> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        let base = game_base();
        if safe_read::<u16>(base)? != 0x5a4d { return None; }
        let pe = safe_read::<u32>(base + 0x3c)? as usize;
        if pe > 0x1000 || safe_read::<u32>(base + pe)? != 0x4550 { return None; }
        match (safe_read::<u32>(base + pe + 8)?, safe_read::<u32>(base + pe + 24 + 56)?) {
            (0x55a38b64, 0xaab000) => Some(MkdxVersion::V11022),
            (0x5fb222aa, 0xb37000) => Some(MkdxVersion::V11816),
            _ => None,
        }
    })
}

fn thread_input() {
    wal_payload_common::start("namcoes3-mariokart");
    wal_payload_common::log!("namcoes3: executable build {:?}", game_version());
    unsafe { patch_mkdx_dongle(); }
    unsafe { patch_mkdx_content_router(); }
    unsafe { patch_mkdx_net_check(); }
    unsafe { patch_mkdx_str_check(); }
    unsafe { patch_mkdx_auto_accel(); }
    unsafe { hook_game_log(); }
    wal_payload_common::dinput::init();
    jvs::init(&[game_base()]);
    unsafe {
        if let Some(original) = wal_payload_common::iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) {
            CO_CREATE_INSTANCE.store(original, Ordering::Relaxed);
            wal_payload_common::log!("namcoes3: hooked game IPC COM activation");
        }
        netlog::init();
        // The built-in unlocked-profile card server, on the redirected card-server address.
        if std::env::var("WAL_NAMCOES3_PROFILE_SERVER").is_ok_and(|v| v.trim() == "1") {
            if let Some((host, port)) = netlog::card_server() {
                profile::start(format!("{host}:{port}"));
            }
        }
        hook_error_registrar();
    }
    std::thread::spawn(force_feedback_loop);
    // TEMP debug: online/offline flags.
    std::thread::spawn(|| {
        let base = game_base();
        let at = |n: usize| base + n - 0x400000;
        let mut last = String::new();
        loop {
            let net = safe_read::<u32>(at(0xec2a0c)).unwrap_or(0) as usize;
            let auth = safe_read::<u32>(at(0xec2a04)).and_then(|p| safe_read::<u32>(p as usize)).unwrap_or(0) as usize;
            let l = format!("flags: net={net:#x} db0={:?} auth={auth:#x} cdc={:?} ce0={:?} ce4={:?} ce8={:?}",
                if net != 0 { safe_read::<u8>(net + 0xdb0) } else { None },
                if auth != 0 { safe_read::<u8>(auth + 0xcdc) } else { None },
                if auth != 0 { safe_read::<u8>(auth + 0xce0) } else { None },
                if auth != 0 { safe_read::<u32>(auth + 0xce4) } else { None },
                if auth != 0 { safe_read::<u32>(auth + 0xce8) } else { None });
            if l != last { wal_payload_common::log!("{l}"); last = l; }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
}

/// MKDX checks for a physical Namco Content Router during boot and
/// raises E05-01 when discovery and link checks fail. UltraLoader's
/// disassembly and registrar trace identify the error branches. Redirect the
/// discovery branches to the game's success path, then skip the two separately
/// traced link-up raises. This only skips the cabinet router presence check;
/// it does not implement online services.
unsafe fn patch_mkdx_content_router() {
    let Some(version) = game_version() else { return; };
    let base = game_base();
    if base == 0 { return; }
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11022 => &[
        // 0x006E0F61: je 0x006E10B7 (null network object -> E05-01)
        //             -> je 0x006E10D7 (router found success path)
        (0x2e0f61, &[0x0f, 0x84, 0x50, 0x01, 0x00, 0x00],
            &[0x0f, 0x84, 0x70, 0x01, 0x00, 0x00], "null-object"),
        // 0x006E0FC3: jge 0x006E10AD (10 retries -> E05-01)
        //             -> jge 0x006E10D7 (router found success path)
        (0x2e0fc3, &[0x0f, 0x8d, 0xe4, 0x00, 0x00, 0x00],
            &[0x0f, 0x8d, 0x0e, 0x01, 0x00, 0x00], "retry-limit"),
        // Two additional router link-up raises discovered by UltraLoader's
        // registrar trace (both push error id 0x10, also rendered E05-01).
        // Keep the comparison and jump target, but make the jump unconditional.
        (0x2e1200, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-a"),
        (0x2e1861, &[0x0f, 0x85, 0x16, 0x01, 0x00, 0x00],
            &[0xe9, 0x17, 0x01, 0x00, 0x00, 0x90], "router-link-b"),
        ],
        MkdxVersion::V11816 => &[
            // 0x71BEAC: null router object -> success at 0x71C02E.
            (0x31beac, &[0x0f, 0x84, 0x5c, 0x01, 0x00, 0x00],
                &[0x0f, 0x84, 0x7c, 0x01, 0x00, 0x00], "null-object"),
            // 0x71BF2A: retry limit -> the same success path.
            (0x31bf2a, &[0x0f, 0x8d, 0xd4, 0x00, 0x00, 0x00],
                &[0x0f, 0x8d, 0xfe, 0x00, 0x00, 0x00], "retry-limit"),
            // The newer build has three link-up raises of error id 0x10
            // (E05-01), rather than the two in v1.10.22. Skip each raise.
            (0x31c160, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-a"),
            (0x31cc50, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-b"),
            (0x31cde2, &[0x75, 0x0f], &[0xeb, 0x0f], "router-link-c"),
            // The wired Batocera network can make the router object appear,
            // then fail its identity/certification checks. Those paths raise
            // E05-02 through the same registrar. The router service has eight
            // direct registrar calls; replace each call with `xor eax,eax;`
            // `add esp,4`, the registrar's false return and stdcall argument
            // cleanup. These call sites are all inside the router subsystem,
            // so other cabinet/device errors still reach the normal handler.
            (0x31c018, &[0xe8, 0xa3, 0xd5, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-null"),
            (0x31c16c, &[0xe8, 0x4f, 0xd4, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-link-a"),
            (0x31cb58, &[0xe8, 0x63, 0xca, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-begin"),
            (0x31cc5c, &[0xe8, 0x5f, 0xc9, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-match"),
            (0x31ccfa, &[0xe8, 0xc1, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-missing"),
            (0x31cd80, &[0xe8, 0x3b, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-version"),
            (0x31cda1, &[0xe8, 0x1a, 0xc8, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-state"),
            (0x31cdee, &[0xe8, 0xcd, 0xc7, 0xee, 0xff], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "router-error-check-final"),
        ],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: Content Router {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: Content Router {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: skipped {} offline Content Router check ({name})", version.name());
    }
}

/// The boot self-test pings the cabinet game server with ICMP (`IcmpSendEcho`) and
/// raises E22-12 (code 0x10) through the shared error registrar when no reply arrives.
/// Offline the server is unreachable, so the raise fires and the game halts on the error
/// screen after the steering check. Suppress it exactly like the Content Router raises:
/// replace the registrar `call` with the registrar's false return plus its stdcall
/// argument cleanup. The preceding `push 0x10` and the following `mov edi,[esp+0x14]`
/// restore are left untouched, so the stack and control flow are unchanged.
unsafe fn patch_mkdx_net_check() {
    let Some(version) = game_version() else { return; };
    let base = game_base();
    if base == 0 { return; }
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11816 => &[
            // 0x6047EC: game-server ping failed -> error registrar call (push 0x10 first).
            (0x2047ec, &[0xe8, 0xcf, 0x4d, 0x00, 0x00], &[0x31, 0xc0, 0x83, 0xc4, 0x04], "ping-e2212"),
        ],
        MkdxVersion::V11022 => &[],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: net-check {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: net-check {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: skipped {} game-server ping check ({name})", version.name());
    }
}

/// The race code (nominal 0x663400) turns on an auto-accelerate assist ("auto accel is on!")
/// while the accelerator is released: a kart flag (`[car+0x9a]`) makes it write full
/// throttle into the kart's input, whatever the pedal says. `WAL_NAMCOES3_AUTO_ACCEL=0` (the
/// default) turns the flag's `je` into a `jmp`, so the forced-throttle branch never runs and
/// the pedal value is used as is.
unsafe fn patch_mkdx_auto_accel() {
    if std::env::var("WAL_NAMCOES3_AUTO_ACCEL").is_ok_and(|v| v.trim() != "0") { return; }
    if game_version() != Some(MkdxVersion::V11816) { return; }
    let base = game_base();
    if base == 0 { return; }
    // cmp byte [esi+0x9a], 0 ; je +0x226  ->  ... ; nop ; jmp +0x226 (patched below)
    let expected: &[u8] = &[0x80, 0xbe, 0x9a, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x84, 0x26, 0x02, 0x00, 0x00];
    let address = (base + 0x263898) as *mut u8;
    if unsafe { std::slice::from_raw_parts(address, expected.len()) } != expected {
        wal_payload_common::log!("namcoes3: auto-accel signature mismatch; skipped");
        return;
    }
    let jump = unsafe { address.add(7) };
    let mut old = 0;
    if unsafe { VirtualProtect(jump.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old) } == 0 { return; }
    unsafe {
        *jump = 0x90;
        *jump.add(1) = 0xe9;
        VirtualProtect(jump.cast(), 2, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), jump.cast(), 2);
    }
    wal_payload_common::log!("namcoes3: disabled v1.18.16 auto-accelerate");
}

/// Debug (`WAL_NAMCOES3_GAME_LOG=1`): the game's own log lines (Shift-JIS, formatted by its
/// logger at nominal 0x830d70) are appended raw to `mkdx-game.log` in the run directory. The
/// logger's sink (0x868d60, stdcall level/text/length) is disabled in the release build; it
/// jumps here instead.
unsafe fn hook_game_log() {
    if !std::env::var("WAL_NAMCOES3_GAME_LOG").is_ok_and(|v| v.trim() == "1") { return; }
    if game_version() != Some(MkdxVersion::V11816) { return; }
    let base = game_base();
    // The sink's entry (push ebp; mov ebp,esp; push -1) becomes a jump to ours: same
    // arguments, same `ret 0xc`, the original is never needed.
    let entry = (base + 0x468d60) as *mut u8;
    if unsafe { std::slice::from_raw_parts(entry, 5) } != [0x55, 0x8b, 0xec, 0x6a, 0xff] {
        wal_payload_common::log!("namcoes3: game log signature mismatch; skipped");
        return;
    }
    let rel = (game_log_sink as usize as i32).wrapping_sub(entry as i32 + 5).to_le_bytes();
    let mut old = 0;
    if unsafe { VirtualProtect(entry.cast(), 5, PAGE_EXECUTE_READWRITE, &mut old) } == 0 { return; }
    unsafe {
        *entry = 0xe9;
        std::ptr::copy_nonoverlapping(rel.as_ptr(), entry.add(1), 4);
        VirtualProtect(entry.cast(), 5, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), entry.cast(), 5);
    }
    wal_payload_common::log!("namcoes3: game log -> mkdx-game.log");
}

unsafe extern "system" fn game_log_sink(_level: u32, text: *const u8, len: u32) -> u32 {
    use std::io::Write;
    if text.is_null() { return 0; }
    let bytes = unsafe { std::slice::from_raw_parts(text, len.min(0x3ff) as usize) };
    static FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
    let mut file = FILE.lock().unwrap();
    if file.is_none() {
        *file = std::fs::OpenOptions::new().create(true).append(true).open("mkdx-game.log").ok();
    }
    if let Some(f) = file.as_mut() {
        let _ = f.write_all(bytes);
        if !bytes.ends_with(b"\n") { let _ = f.write_all(b"\n"); }
    }
    0
}

/// The boot steering (STR/force-feedback) self-test polls the board until its
/// done-flag is set, then reports success; after ~4800 polls (~120 s) with the
/// flag still clear it raises E22-12 (code 0x61) and the game halts on the error
/// screen. The check's success path (nominal 0x602e47) re-reads the device each
/// poll and branches on the done-flag byte at [device+0x49]. Replace that flag
/// check with a flag *set*: `mov byte [eax+0x49], 1` followed by NOPs, so the
/// self-test reports complete on the first poll and the game boots straight to
/// the title with FFB enabled. `eax` holds the STR device at this point (loaded
/// at 0x602e47), and the surrounding null/device guards are left untouched.
unsafe fn patch_mkdx_str_check() {
    let Some(version) = game_version() else { return; };
    let base = game_base();
    if base == 0 { return; }
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11816 => &[
            // 0x602E55: cmp byte [eax+0x49],0 / je 0x602EF7  ->  mov [eax+0x49],1 + nop*6
            (0x202e55,
             &[0x80, 0x78, 0x49, 0x00, 0x0f, 0x84, 0x98, 0x00, 0x00, 0x00],
             &[0xc6, 0x40, 0x49, 0x01, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
             "str-steering-skip"),
        ],
        MkdxVersion::V11022 => &[],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: STR-check {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: STR-check {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: skipped {} boot steering self-test ({name})", version.name());
    }
}

/// Diagnostic: inline-hook the shared error registrar (nominal 0x6095c0) so every error
/// the boot self-test raises is logged with its code and caller. This tells us which
/// check still fails when the router/ping raises are already suppressed.
unsafe fn hook_error_registrar() {
    if game_version() != Some(MkdxVersion::V11816) { return; }
    let base = game_base();
    if base == 0 { return; }
    let registrar = base + 0x2095c0; // nominal 0x6095c0
    let resume = base + 0x2095c6;    // nominal 0x6095c6 (mov eax,[0xec2a04])
    let hook = err_registrar_hook as usize;
    let trampoline = unsafe {
        VirtualAlloc(std::ptr::null_mut(), 0x100, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
    } as usize;
    if trampoline == 0 {
        wal_payload_common::log!("namcoes3: ERR-REG hook VirtualAlloc failed");
        return;
    }
    let mut t: Vec<u8> = Vec::new();
    t.push(0x55); // push ebp
    t.push(0x8b); t.push(0xec); // mov ebp, esp
    t.push(0x55); // push ebp (arg)
    t.push(0xe8); t.extend_from_slice(&((hook as i32) - ((trampoline + 9) as i32)).to_le_bytes()); // call hook
    t.push(0x83); t.push(0xe4); t.push(0xf8); // and esp, 0xfffffff8
    t.push(0xe9); t.extend_from_slice(&((resume as i32) - ((trampoline + 17) as i32)).to_le_bytes()); // jmp resume
    unsafe {
        std::ptr::copy_nonoverlapping(t.as_ptr(), trampoline as *mut u8, t.len());
    }
    // jmp trampoline over the registrar's first 5 bytes
    let rel = ((trampoline as i32) - ((registrar + 5) as i32)).to_le_bytes();
    let mut old = 0;
    if unsafe { VirtualProtect(registrar as *mut _, 5, PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        wal_payload_common::log!("namcoes3: ERR-REG hook VirtualProtect failed");
        return;
    }
    unsafe {
        std::ptr::write(registrar as *mut u8, 0xe9);
        std::ptr::copy_nonoverlapping(rel.as_ptr(), (registrar + 1) as *mut u8, 4);
        VirtualProtect(registrar as *mut _, 5, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), registrar as *mut _, 5);
    }
    wal_payload_common::log!("namcoes3: hooked error registrar (diagnostic)");
}

/// stdcall, one arg = the registrar's EBP frame. Logs the error code ([ebp+8]) and the
/// caller's return address ([ebp+4]).
unsafe extern "system" fn err_registrar_hook(frame: usize) {
    if frame < 0x10000 { return; }
    let code = unsafe { *(frame as *const u32).add(2) };
    let retaddr = unsafe { *(frame as *const u32).add(1) } as usize;
    // Rebase the caller onto the nominal 0x400000 image for readability.
    let base = game_base();
    let nominal = if base != 0 { retaddr - base + 0x400000 } else { retaddr };
    wal_payload_common::log!("namcoes3: ERR-REG code {code:#04x} from nominal {nominal:#010x}");
}

/// Version-specific dongle compatibility patches, with PE identity and original bytes
/// checked before modification. v1.18.16's persistence helper takes one stack argument;
/// its replacement must retain `ret 4`, unlike v1.10.22's `ret`.
unsafe fn patch_mkdx_dongle() {
    let base = game_base();
    if base == 0 { return; }
    let Some(version) = game_version() else {
        wal_payload_common::log!("namcoes3: unknown executable; dongle patches skipped");
        return;
    };
    let patches: &[(usize, &[u8], &[u8], &str)] = match version {
        MkdxVersion::V11022 => &[
        (0x1efcc0, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "poll"),
        (0x1ef669,
            &[0xe8, 0x42, 0x07, 0x00, 0x00, 0x8a, 0x97, 0xe0, 0x0c, 0x00, 0x00],
            &[0x90, 0x90, 0x90, 0x90, 0x90, 0xb2, 0x01, 0x90, 0x90, 0x90, 0x90], "init"),
        (0x1ef7d0, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "update"),
        ],
        MkdxVersion::V11816 => &[
            (0x208130, &[0x55, 0x8b, 0xec, 0x6a, 0xff], &[0xb0, 0x01, 0xc2, 0x04, 0x00], "poll"),
            // Skip loading the saved dongle data and clear its offline-grace flag (copied to
            // [mgr+0xcdc]): with it set the game runs in offline mode and refuses Banapass
            // cards; the in-process AMCUS auth reports the cabinet online instead.
            (0x207ae9,
                &[0xe8, 0x32, 0x07, 0x00, 0x00, 0x8a, 0x97, 0xe0, 0x0c, 0x00, 0x00],
                &[0x90, 0x90, 0x90, 0x90, 0x90, 0xb2, 0x00, 0x90, 0x90, 0x90, 0x90], "init"),
            (0x207c50, &[0x55, 0x8b, 0xec], &[0xb0, 0x01, 0xc3], "update"),
        ],
    };
    for (rva, expected, replacement, name) in patches {
        let address = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(address, expected.len()) } != *expected {
            wal_payload_common::log!("namcoes3: dongle {name} signature mismatch; skipped");
            continue;
        }
        let mut old = 0;
        if unsafe { VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
            wal_payload_common::log!("namcoes3: dongle {name} VirtualProtect failed");
            continue;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
            VirtualProtect(address.cast(), replacement.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
        }
        wal_payload_common::log!("namcoes3: applied {} dongle {name} patch", version.name());
    }
}

type CoCreateInstance = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
const IPC_CLSID: [u8; 16] = [0x2a,0x4b,0xc4,0xcc,0x9a,0x70,0xa8,0x33,0xa1,0xb0,0x5d,0xbe,0x25,0xb1,0x9c,0x2c];

unsafe extern "system" fn co_create_instance(
    clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void,
) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == IPC_CLSID {
        if !out.is_null() { unsafe { *out = (&ipc::OBJECT as *const ipc::Object).cast_mut().cast(); } }
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| wal_payload_common::log!("namcoes3: supplied offline IPC COM adapter"));
        return 0;
    }
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == amcus::CLSID {
        if !out.is_null() { unsafe { *out = (&amcus::OBJECT as *const amcus::Object).cast_mut().cast(); } }
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| wal_payload_common::log!("namcoes3: supplied in-process AMCUS auth (IAuth)"));
        return 0;
    }
    let original = CO_CREATE_INSTANCE.load(Ordering::Relaxed);
    if original == 0 { return 0x80004005u32 as i32; }
    let real: CoCreateInstance = unsafe { std::mem::transmute(original) };
    unsafe { real(clsid, outer, context, iid, out) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        thread_input();
        wal_payload_common::log!("namcoes3: 32-bit Banapass and DirectInput adapter loaded");
    }
    TRUE
}

fn safe_read<T: Copy + Default>(address: usize) -> Option<T> {
    if address < 0x10000 { return None; }
    let mut value = T::default();
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(), address as *const c_void, (&mut value as *mut T).cast(),
            std::mem::size_of::<T>(), std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(value)
}

/// Game state read for the "custom" force feedback (the FFB Arcade Plugin's
/// MarioKartGPDX1.18Custom.cpp): instead of passing through the game's own small FFB value,
/// it synthesizes effects from what the kart is doing (surface, drift, boost, hits, ...).
/// The offsets are the 1.18 ones and are checked against v1.18.16.
#[derive(Clone, Copy, Default, PartialEq)]
struct CustomState {
    shake: u32,      // 0xAC72F4: 0x400004 while the screen is shaking (big hit)
    kart: u32,       // 0xAC2A30: local kart object
    surface: u8,     // [kart+0x66C]: 3 dirt, 4 grass, 7 water, 8 bridge, 9 carpet, 10 bump, 11 rough, 12 tile, 14 sand
    on_ground: u8,   // [kart+0x69C]: 1 when on the ground
    in_race: u8,     // pointer chain: 1 only during a race
    speed: f32,      // [kart+0x580]
    coins: u8,       // 0xAB9110: coin count (changes on pickup)
    weapon: u8,      // 0xAC5C0D: weapon (changes on pickup / use)
    wheel: u8,       // 0xAC6EAD: steering 0..=255, 128 straight
    boost: u8,       // [0xAAD0BC+0x3A4]: 1 while boosting
    drift: u8,       // [kart+0x6B8]: 1 while drifting
    game_state: u8,  // [0xAAEE58+0x44]
}

fn read_custom_state(base: usize) -> CustomState {
    let kart = safe_read::<u32>(base + 0xAC2A30).unwrap_or(0) as usize;
    let in_race = safe_read::<u32>(base + 0xAAFAB8)
        .and_then(|p| safe_read::<u32>(p as usize + 0x44))
        .and_then(|p| safe_read::<u32>(p as usize + 0x70))
        .and_then(|p| safe_read::<u32>(p as usize + 0x108))
        .and_then(|p| safe_read::<u32>(p as usize + 0x98))
        .and_then(|p| safe_read::<u8>(p as usize + 0x4C4))
        .unwrap_or(0);
    CustomState {
        shake: safe_read::<u32>(base + 0xAC72F4).unwrap_or(0),
        kart: kart as u32,
        surface: safe_read::<u8>(kart + 0x66C).unwrap_or(0),
        on_ground: safe_read::<u8>(kart + 0x69C).unwrap_or(0),
        in_race,
        speed: safe_read::<f32>(kart + 0x580).unwrap_or(0.0),
        coins: safe_read::<u8>(base + 0xAB9110).unwrap_or(0),
        weapon: safe_read::<u8>(base + 0xAC5C0D).unwrap_or(0),
        wheel: safe_read::<u8>(base + 0xAC6EAD).unwrap_or(0),
        boost: safe_read::<u32>(base + 0xAAD0BC).and_then(|p| safe_read::<u8>(p as usize + 0x3A4)).unwrap_or(0),
        drift: safe_read::<u8>(kart + 0x6B8).unwrap_or(0),
        game_state: safe_read::<u32>(base + 0xAAEE58).and_then(|p| safe_read::<u8>(p as usize + 0x44)).unwrap_or(0),
    }
}

/// A 0..=100 percentage from an env var (0 disables the effect), with a default.
fn ffb_percent(var: &str, default: i32) -> i32 {
    std::env::var(var).ok().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(default).clamp(0, 100)
}

fn force_feedback_loop() {
    let base = game_base();
    // These force-feedback offsets belong to v1.18.16, not v1.10.22.
    if game_version() != Some(MkdxVersion::V11816) { return; }
    let spring = std::env::var("WAL_NAMCOES3_FFB_SPRING").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(0, 10000);
    // `real` (default) passes through the game's own small FFB value. `custom` synthesizes
    // effects from game state (surface, drift, boost, hits, ...) like the FFB Arcade Plugin's
    // MarioKartGPDX1.18Custom.cpp — it feels much richer.
    let custom = std::env::var("WAL_NAMCOES3_FFB_MODE").ok().is_some_and(|v| v.trim().eq_ignore_ascii_case("custom"));
    let trace = std::env::var("WAL_NAMCOES3_FFB_TRACE").is_ok_and(|v| v == "1");
    if custom {
        wal_payload_common::log!(
            "namcoes3: FFB mode custom (steer={} shake={} boost={} drift={} terrain={} pickup={})",
            ffb_percent("WAL_NAMCOES3_FFB_STEER", 25), ffb_percent("WAL_NAMCOES3_FFB_SHAKE", 80),
            ffb_percent("WAL_NAMCOES3_FFB_BOOST", 60), ffb_percent("WAL_NAMCOES3_FFB_DRIFT", 50),
            ffb_percent("WAL_NAMCOES3_FFB_TERRAIN", 45), ffb_percent("WAL_NAMCOES3_FFB_PICKUP", 50),
        );
    } else {
        wal_payload_common::log!("namcoes3: FFB mode real (game pass-through)");
    }
    // Custom effect strengths as 0..=1.
    let steer   = ffb_percent("WAL_NAMCOES3_FFB_STEER", 25) as f32 / 100.0;
    let shake   = ffb_percent("WAL_NAMCOES3_FFB_SHAKE", 80) as f32 / 100.0;
    let boost   = ffb_percent("WAL_NAMCOES3_FFB_BOOST", 60) as f32 / 100.0;
    let drift   = ffb_percent("WAL_NAMCOES3_FFB_DRIFT", 50) as f32 / 100.0;
    let terrain = ffb_percent("WAL_NAMCOES3_FFB_TERRAIN", 45) as f32 / 100.0;
    let pickup  = ffb_percent("WAL_NAMCOES3_FFB_PICKUP", 50) as f32 / 100.0;

    let mut last = [i32::MIN; 2];                 // [constant, vibration]
    let mut last_sig = CustomState::default();    // for the trace
    // Edge-triggered ("transient") effect expiry times.
    let mut coin_until = std::time::Instant::now();
    let mut weapon_until = std::time::Instant::now();
    let mut land_until = std::time::Instant::now();
    let mut prev: Option<CustomState> = None;

    loop {
        let st = read_custom_state(base);
        if trace {
            // Only log when a discrete field changes (speed/wheel churn every frame); the line
            // still shows the current speed/wheel for inspection.
            let sig = CustomState { shake: st.shake, surface: st.surface, on_ground: st.on_ground,
                in_race: st.in_race, coins: st.coins, weapon: st.weapon, boost: st.boost,
                drift: st.drift, game_state: st.game_state, kart: 0, speed: 0.0, wheel: 0 };
            if sig != last_sig {
                wal_payload_common::log!(
                    "ffb-trace: shake={:#010x} kart={:#x} surface={} ground={} race={} speed={:.2} coins={} weapon={} wheel={} boost={} drift={} gs={}",
                    st.shake, st.kart, st.surface, st.on_ground, st.in_race, st.speed,
                    st.coins, st.weapon, st.wheel, st.boost, st.drift, st.game_state,
                );
                last_sig = sig;
            }
        }
        let (constant, vibration) = if custom {
            // Edge detection for transient effects.
            if let Some(p) = prev {
                let now = std::time::Instant::now();
                if st.coins != p.coins { coin_until = now + Duration::from_millis(200); }
                if st.weapon != p.weapon { weapon_until = now + Duration::from_millis(300); }
                if p.on_ground == 0 && st.on_ground == 1 { land_until = now + Duration::from_millis(120); }
            }
            prev = Some(st);
            let now = std::time::Instant::now();
            let (mut con, mut vib) = (0.0f32, 0.0f32);
            if st.in_race == 1 {
                // Constant force in the steering direction, proportional to how far off-center.
                if st.wheel < 128 { con -= steer * (128 - st.wheel as i32) as f32 / 128.0; }
                else if st.wheel > 128 { con += steer * (st.wheel as i32 - 128) as f32 / 128.0; }
                // Big shake: wall / kart / item hits.
                if st.shake == 0x400004 { vib += shake; }
                // Nitro boost.
                if st.boost == 1 { vib += boost; }
                // Drift.
                if st.drift == 1 { vib += drift; }
                // Surface rumble while on the ground and moving.
                if st.on_ground == 1 && st.speed > 0.1 {
                    match st.surface {
                        3 | 4 | 7 | 8 | 9 | 11 | 12 | 14 => vib += terrain,
                        10 => vib += terrain * 0.6,   // speed bump: shorter, lighter
                        _ => {}
                    }
                }
                // Transient pickups / landing.
                if now < coin_until { vib += pickup; }
                if now < weapon_until { vib += pickup; }
                if now < land_until { vib += pickup * 1.2; }
            }
            let constant = (con.clamp(-1.0, 1.0) * output::FFB_MAX as f32) as i32;
            let vibration = (vib.clamp(0.0, 1.0) * output::FFB_MAX as f32) as i32;
            (constant, vibration)
        } else {
            let force = safe_read::<u32>(base + 0xAAEEA0)
                .and_then(|ptr| safe_read::<u32>(ptr as usize + 8))
                .and_then(|ptr| safe_read::<f32>(ptr as usize + 0xBC8))
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                .clamp(-1.0, 1.0);
            ((force * output::FFB_MAX as f32) as i32, (force.abs() * output::FFB_MAX as f32) as i32)
        };
        let values = [constant, vibration];
        for (i, id) in [output::FFB_CONSTANT, output::FFB_VIBRATION].into_iter().enumerate() {
            if last[i] != values[i] {
                wal_payload_common::send_output(0, id, values[i]);
                last[i] = values[i];
            }
        }
        wal_payload_common::send_output(0, output::FFB_SPRING, spring);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn card_data() -> [u8; 168] {
    let mut data = [0u8; 168];
    data[0..8].copy_from_slice(&[1, 1, 0, 0, 4, 0, 0, 0]);
    data[44..76].copy_from_slice(b"7F5C9744F111111143262C3300040610");
    data[80..100].copy_from_slice(b"30764352518498791337");
    if let Ok(id) = std::env::var("WAL_NAMCOES3_CARD_ID") {
        let id = id.as_bytes();
        data[44..76].fill(0);
        let len = id.len().min(31);
        data[44..44 + len].copy_from_slice(&id[..len]);
    }
    if let Ok(access) = std::env::var("WAL_NAMCOES3_ACCESS_CODE") {
        let access = access.as_bytes();
        data[80..100].fill(0);
        let len = access.len().min(19);
        data[80..80 + len].copy_from_slice(&access[..len]);
    }
    data
}

/// The game's pending card-tap request (callback, context): a new request replaces it, a
/// cancel clears it.
static TOUCH_REQUEST: std::sync::Mutex<Option<(TouchCallback, usize)>> = std::sync::Mutex::new(None);

/// v1.18.16's game state byte (`[0xAAEE58]+0x44`, nominal) on the "Card Validation" screen
/// (2 attract, 59 character select, 15 race).
const STATE_CARD_SCREEN: u8 = 9;

/// The game state, v1.18.16 only.
fn card_screen() -> bool {
    game_version() == Some(MkdxVersion::V11816)
        && safe_read::<u32>(game_base() + 0xAAEE58)
            .and_then(|p| safe_read::<u8>(p as usize + 0x44))
            == Some(STATE_CARD_SCREEN)
}

/// Completes the pending card-tap request with the card, if there is one.
fn touch_card() -> bool {
    // the lock is released before the callback: the game re-arms the wait from it
    let request = TOUCH_REQUEST.lock().unwrap().take();
    let Some((callback, context)) = request else { return false };
    let mut data = card_data();
    unsafe { callback(0, 0, data.as_mut_ptr().cast(), context as *mut c_void) };
    true
}

/// Game asks asynchronously for a card tap. The launcher's `card` control inserts a card;
/// the pending request is completed once for each insertion. The game re-arms the wait many
/// times a second (30/s in attract, 60/s after a card is read): one watcher thread serves
/// every request (a thread per request piled up hundreds of 2 MB stacks until the 32-bit
/// address space ran out).
///
/// `WAL_NAMCOES3_AUTO_CARD=1`: the card is inserted by itself once the game has shown its
/// card screen for a second, once per visit of the screen.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn BngRwReqWaitTouch(
    _reader: u32, _timeout: i32, _flags: u32, callback: Option<TouchCallback>, context: *mut c_void,
) -> i32 {
    let Some(callback) = callback else { return -1 };
    *TOUCH_REQUEST.lock().unwrap() = Some((callback, context as usize));
    static WATCHER: std::sync::Once = std::sync::Once::new();
    WATCHER.call_once(|| {
        let auto = std::env::var("WAL_NAMCOES3_AUTO_CARD").is_ok_and(|v| v.trim() == "1");
        std::thread::spawn(move || {
            // when the card screen showed up, and whether its card was inserted
            let mut screen: Option<(std::time::Instant, bool)> = None;
            loop {
                if auto && card_screen() {
                    screen.get_or_insert((std::time::Instant::now(), false));
                } else {
                    screen = None;
                }
                let inserted = wal_payload_common::input(0).pressed(button::CARD);
                if !inserted {
                    CARD_CONSUMED.store(false, Ordering::Release);
                } else if !CARD_CONSUMED.load(Ordering::Acquire) && touch_card() {
                    // a card inserted between two requests completes the next one
                    CARD_CONSUMED.store(true, Ordering::Release);
                    // the player's own card: no automatic one on this screen
                    if let Some((_, done)) = screen.as_mut() { *done = true; }
                }
                if let Some((since, done)) = screen.as_mut() {
                    if !*done && since.elapsed() >= Duration::from_secs(1) && touch_card() {
                        *done = true;
                        wal_payload_common::log!("namcoes3: card inserted automatically");
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    });
    -1
}

macro_rules! export {
    ($rust:ident, $name:literal, ($($arg:ident : $ty:ty),*) -> $ret:ty, $body:expr) => {
        #[unsafe(export_name = $name)]
        pub extern "system" fn $rust($($arg: $ty),*) -> $ret { $body }
    };
}


export!(bng_init, "BngRwInit", () -> i32, 0);
export!(bng_fin, "BngRwFin", () -> i32, 0);
export!(bng_version, "BngRwGetVersion", () -> u32, 0x0001_0000);
export!(bng_reset, "BngRwDevReset", (_reader: u32, callback: Option<ResetCallback>, context: *mut isize) -> i32, {
    if let Some(callback) = callback { let context = context as usize; std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); unsafe { callback(0, 0, context as *mut isize); } }); }
    1
});
export!(bng_attach, "BngRwAttach", (_reader: u32, _name: *const c_char, _a: i32, _b: i32, callback: Option<AttachCallback>, context: *mut isize) -> u64, {
    if let Some(callback) = callback { let context = context as usize; std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(100)); unsafe { callback(0, 0, context as *mut isize); } }); }
    1
});
export!(bng_station, "BngRwGetStationID", (_out: *mut u32) -> i32, 1);
export!(bng_fw, "BngRwGetFwVersion", (_out: *mut u32) -> i32, 1);
export!(bng_retry, "BngRwGetTotalRetryCount", (_out: *mut u32) -> i32, 0);
export!(bng_cmd, "BngRwIsCmdExec", (_cmd: u32) -> i32, 0);
export!(bng_mifare, "BngRwExReadMifareAllBlock", (_a: u32, _b: *mut c_void) -> i32, 1);
export!(bng_action, "BngRwReqAction", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_auth, "BngRwReqAiccAuth", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32) -> i32, 1);
export!(bng_beep, "BngRwReqBeep", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_cancel, "BngRwReqCancel", (_a: u32) -> i32, {
    *TOUCH_REQUEST.lock().unwrap() = None;
    1
});
export!(bng_cleanup, "BngRwReqFwCleanup", (_a: u32, _b: u32, _c: u32) -> i32, 1);
export!(bng_fw_update, "BngRwReqFwVersionup", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_latch, "BngRwReqLatchID", (_a: u32, _b: u32, _c: u32) -> i32, 1);
export!(bng_led, "BngRwReqLed", (_a: u32, _b: u32, _c: u32, _d: u32) -> i32, 1);
export!(bng_mail, "BngRwReqSendMailTo", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32, _i: u32, _j: u32) -> i32, 1);
export!(bng_url, "BngRwReqSendUrlTo", (_a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32) -> i32, 1);
export!(bng_power, "BngRwSetLedPower", (_a: u32, _b: u32, _c: u32) -> i32, 1);
