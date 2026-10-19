//! Crash reporter: logs the first access violations of the game process with registers and
//! a symbolized stack scan (`module+offset` of every stack value pointing into a module).
//! Wine's own traces only give the faulting address; this shows which game code led there.

use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};

use crate::log;

/// `len` bytes at `addr` can be read. The handler must never fault itself: a fault while
/// reporting left Music GunGun! 2 hanging (stack scan past the top of a new thread's stack).
/// `IsBadReadPtr` raises a first-chance access violation on unmapped addresses (a VEH-visible
/// AV under wine 11.18, one per call, exhausting the report quota): a `VirtualQuery` page
/// walk instead.
fn readable(addr: usize, len: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
    };
    addr >= 0x10000 && {
        let end = addr.saturating_add(len);
        let mut page = addr & !0xFFF;
        while page < end {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            if unsafe { VirtualQuery(page as *const _, &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
                return false;
            }
            if info.State != MEM_COMMIT || info.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0 {
                return false;
            }
            page = (info.BaseAddress as usize).wrapping_add(info.RegionSize as usize).max(page + 1);
        }
        true
    }
}

const EXCEPTION_ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;
/// MSVC C++ `throw` (`'msc'`).
const EXCEPTION_MSVC_CPP: i32 = 0xE06D_7363_u32 as i32;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const MAX_REPORTS: u32 = 8;

// Separate quotas: a burst of C++ exceptions (the game's fatal-error throw path dispatches
// several before the dialog) must not suppress the access-violation dump, which carries the
// real crash stack (the report quota was otherwise exhausted before the fatal AV).
static AV_REPORTS: AtomicU32 = AtomicU32::new(0);
static CPP_REPORTS: AtomicU32 = AtomicU32::new(0);

pub fn init() {
    unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
}

/// `module+offset` for an address inside a loaded module.
pub fn symbolize(addr: usize) -> Option<String> {
    let mut module: HMODULE = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, addr as *const u16, &mut module) } == 0 || module.is_null() {
        return None;
    }
    let mut name = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&name[..n]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    Some(format!("{file}+{:#x}", addr - module as usize))
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let record = unsafe { &*(*info).ExceptionRecord };
    let code = record.ExceptionCode;
    let reports = if code == EXCEPTION_ACCESS_VIOLATION {
        &AV_REPORTS
    } else if code == EXCEPTION_MSVC_CPP {
        &CPP_REPORTS
    } else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    if reports.fetch_add(1, Ordering::Relaxed) >= MAX_REPORTS {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let addr = record.ExceptionAddress as usize;
    log!(
        "crash: [tid {:x}] {}",
        unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
        if code == EXCEPTION_MSVC_CPP { "C++ exception" } else { "access violation" }
    );
    if code == EXCEPTION_MSVC_CPP {
        log_cpp_exception(record);
    } else {
        log_access_violation(record, addr);
    }
    log_context(info, code == EXCEPTION_ACCESS_VIOLATION);
    EXCEPTION_CONTINUE_SEARCH
}

/// C++ exception thrown by the game (an uncaught one ends in "Runtime Error! ... terminate it in
/// an unusual way"): the thrown type (32-bit ThrowInfo: absolute pointers) and, for
/// `std::exception`, its message (`{vfptr, const char* what, ...}`).
fn log_cpp_exception(record: &windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_RECORD) {
    let (object, throw_info) = (record.ExceptionInformation[1], record.ExceptionInformation[2]);
    #[allow(unused_mut)]
    let mut name = String::new();
    #[cfg(target_arch = "x86")]
    unsafe {
        let at = |a: usize| if readable(a, 4) { Some(std::ptr::read_volatile(a as *const usize)) } else { None };
        // ThrowInfo +0xC -> CatchableTypeArray {n, types[]}; CatchableType +4 -> TypeDescriptor, name at +8
        if let Some(types) = at(throw_info + 0xC)
            && let Some(first) = at(types + 4)
            && let Some(desc) = at(first + 4)
            && readable(desc + 8, 64)
        {
            name = (0..64).map(|i| std::ptr::read_volatile((desc + 8 + i) as *const u8)).take_while(|b| *b != 0).map(|b| b as char).collect();
        }
        if let Some(what) = at(object + 4)
            && readable(what, 1)
        {
            let text: String = (0..160)
                .map(|i| if readable(what + i, 1) { std::ptr::read_volatile((what + i) as *const u8) } else { 0 })
                .take_while(|b| *b != 0)
                .map(|b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
                .collect();
            log!("crash: C++ exception what: {text}");
        }
    }
    log!("crash: C++ exception {name} (object {object:#x}, throw info {throw_info:#x}) at {:#x}", record.ExceptionAddress as usize);
}

fn log_access_violation(record: &windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_RECORD, addr: usize) {
    let (op, target) = (record.ExceptionInformation[0], record.ExceptionInformation[1]);
    log!(
        "crash: access violation ({} {target:#x}) at {addr:#x} {}",
        if op == 1 { "writing" } else { "reading" },
        symbolize(addr).unwrap_or_default()
    );
    // write overrun: show what was being written just below the faulting address
    if op == 1 && target > 0x10000 && readable(target - 160, 160) {
        let start = target - 160;
        let bytes: Vec<u8> = (start..target).map(|a| unsafe { std::ptr::read_volatile(a as *const u8) }).collect();
        let text: String = bytes.iter().map(|b| if (0x20..0x7f).contains(b) { *b as char } else { '.' }).collect();
        log!("crash: bytes before {target:#x}: {text}");
    }
}

fn log_context(info: *mut EXCEPTION_POINTERS, raw_stack: bool) {
    let _ = (info, raw_stack);
    #[cfg(target_arch = "x86")]
    unsafe {
        let c = &*(*info).ContextRecord;
        log!(
            "crash: eip={:08x} eax={:08x} ebx={:08x} ecx={:08x} edx={:08x} esi={:08x} edi={:08x} ebp={:08x} esp={:08x}",
            c.Eip, c.Eax, c.Ebx, c.Ecx, c.Edx, c.Esi, c.Edi, c.Ebp, c.Esp
        );
        // For an access violation the first stack word is the return address of the faulting
        // call: dump the raw stack top even for words that do not resolve to a module.
        if raw_stack {
            let esp = c.Esp as usize;
            if readable(esp, 128) {
                let words: Vec<String> = (0..32)
                    .map(|i| format!("{:08x}", std::ptr::read_volatile((esp + i * 4) as *const usize)))
                    .collect();
                log!("crash:   raw stack: {}", words.join(" "));
            }
        }
        // EBP frame chain: [ebp] = caller ebp, [ebp+4] = return address
        let mut ebp = c.Ebp as usize;
        for depth in 0..16 {
            if ebp < 0x10000 || ebp % 4 != 0 || ebp < c.Esp as usize || ebp > c.Esp as usize + 0x100000 || !readable(ebp, 8) {
                break;
            }
            let frame = ebp as *const usize;
            let ret = std::ptr::read_volatile(frame.add(1));
            log!("crash:   frame {depth}: return {ret:#010x} {}", symbolize(ret).unwrap_or_default());
            ebp = std::ptr::read_volatile(frame);
        }
        // stack scan: return addresses into modules
        let esp = c.Esp as usize as *const usize;
        let mut found = 0;
        for i in 0..4096 {
            if !readable(esp.add(i) as usize, 4) {
                break;
            }
            let v = std::ptr::read_volatile(esp.add(i));
            if v < 0x10000 {
                continue;
            }
            if let Some(sym) = symbolize(v) {
                log!("crash:   [esp+{:#05x}] {v:#010x} {sym}", i * 4);
                found += 1;
                if found >= 48 {
                    break;
                }
            }
        }
    }
}

/// Hang reporter: `heartbeat()` is called by the game's main loop (a per-frame API call);
/// when it stops for 5 s, the thread that called it last is suspended and its instruction
/// pointer and the module addresses on its stack are logged (`hang:` lines), once per stall,
/// at most 3 times, then 20 samples of its instruction pointer (a loop that is not a wait).
/// The game window "not responding" without a crash: what it waits on.
pub fn heartbeat() {
    use std::sync::atomic::AtomicU64;
    use std::sync::Once;
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    static LAST: AtomicU64 = AtomicU64::new(0);
    static THREAD: AtomicU32 = AtomicU32::new(0);
    static START: Once = Once::new();
    LAST.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
    THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    START.call_once(|| {
        std::thread::spawn(|| {
            let mut reported = false;
            let mut dumps = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let idle = unsafe { GetTickCount64() }.saturating_sub(LAST.load(Ordering::Relaxed));
                if idle < 5000 {
                    reported = false;
                } else if !reported && dumps < 3 {
                    reported = true;
                    dumps += 1;
                    log!("hang: main loop stopped for {idle} ms");
                    let main = THREAD.load(Ordering::Relaxed);
                    dump_thread(main, 48);
                    // what the main thread may wait on: the game's other threads
                    for id in other_threads(main) {
                        dump_thread(id, 16);
                    }
                    // where a busy main thread loops: its instruction pointer sampled
                    let samples: Vec<String> = (0..20)
                        .filter_map(|_| {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            thread_ip(main)
                        })
                        .map(|ip| format!("{ip:#x}"))
                        .collect();
                    log!("hang: main thread samples {}", samples.join(" "));
                }
            }
        });
    });
}

/// The process's threads except `main` and the calling one.
fn other_threads(main: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};

    let mut ids = Vec::new();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return ids;
    }
    let (pid, me) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid && entry.th32ThreadID != main && entry.th32ThreadID != me {
            ids.push(entry.th32ThreadID);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    ids
}

/// A thread's instruction pointer, read while it is suspended.
fn thread_ip(id: u32) -> Option<usize> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
    };

    let thread = unsafe { OpenThread(THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME, 0, id) };
    if thread.is_null() {
        return None;
    }
    let mut context: CONTEXT = unsafe { std::mem::zeroed() };
    #[cfg(target_arch = "x86_64")]
    {
        context.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_AMD64;
    }
    #[cfg(target_arch = "x86")]
    {
        context.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_X86;
    }
    unsafe { SuspendThread(thread) };
    let ok = unsafe { GetThreadContext(thread, &mut context) } != 0;
    unsafe {
        ResumeThread(thread);
        CloseHandle(thread);
    }
    #[cfg(target_arch = "x86_64")]
    let ip = context.Rip as usize;
    #[cfg(target_arch = "x86")]
    let ip = context.Eip as usize;
    ok.then_some(ip)
}

fn dump_thread(id: u32, max: usize) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
    };

    let thread = unsafe { OpenThread(THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME, 0, id) };
    if thread.is_null() {
        log!("hang: cannot open thread {id}");
        return;
    }
    let mut context: CONTEXT = unsafe { std::mem::zeroed() };
    #[cfg(target_arch = "x86_64")]
    {
        context.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_AMD64;
    }
    #[cfg(target_arch = "x86")]
    {
        context.ContextFlags = windows_sys::Win32::System::Diagnostics::Debug::CONTEXT_FULL_X86;
    }
    unsafe { SuspendThread(thread) };
    let ok = unsafe { GetThreadContext(thread, &mut context) } != 0;
    #[cfg(target_arch = "x86_64")]
    let (ip, sp) = (context.Rip as usize, context.Rsp as usize);
    #[cfg(target_arch = "x86")]
    let (ip, sp) = (context.Eip as usize, context.Esp as usize);
    // read the stack while the thread is stopped
    let word = std::mem::size_of::<usize>();
    let mut found = Vec::new();
    if ok {
        for i in 0..8192 {
            let at = sp + i * word;
            if !readable(at, word) {
                break;
            }
            let v = unsafe { std::ptr::read_volatile(at as *const usize) };
            if v >= 0x10000
                && let Some(sym) = symbolize(v)
            {
                found.push((i * word, v, sym));
                if found.len() >= max {
                    break;
                }
            }
        }
    }
    // x64: the exact call stack from the unwind tables (the stack scan above also shows stale
    // return addresses)
    #[cfg(target_arch = "x86_64")]
    let frames = if ok { unwind(context) } else { Vec::new() };
    unsafe {
        ResumeThread(thread);
        CloseHandle(thread);
    }
    #[cfg(target_arch = "x86_64")]
    for (depth, ip) in frames.iter().enumerate() {
        log!("hang:   thread {id} frame {depth}: {ip:#x} {}", symbolize(*ip).unwrap_or_default());
    }
    if !ok {
        log!("hang: no context for thread {id}");
        return;
    }
    log!("hang: thread {id} at {ip:#x} {} (sp {sp:#x})", symbolize(ip).unwrap_or_default());
    for (off, v, sym) in found {
        log!("hang:   [sp+{off:#06x}] {v:#x} {sym}");
    }
}

/// Call stack of a suspended thread's context (x64 unwind data, leaf frames popped).
#[cfg(target_arch = "x86_64")]
fn unwind(mut context: windows_sys::Win32::System::Diagnostics::Debug::CONTEXT) -> Vec<usize> {
    use windows_sys::Win32::System::Diagnostics::Debug::{RtlLookupFunctionEntry, RtlVirtualUnwind, UNW_FLAG_NHANDLER};

    let mut frames = Vec::new();
    for _ in 0..32 {
        let ip = context.Rip;
        if ip < 0x10000 {
            break;
        }
        frames.push(ip as usize);
        let mut base = 0u64;
        let entry = unsafe { RtlLookupFunctionEntry(ip, &mut base, std::ptr::null_mut()) };
        if entry.is_null() {
            // leaf function: the return address is on top of the stack
            let sp = context.Rsp as usize;
            if !readable(sp, 8) {
                break;
            }
            context.Rip = unsafe { std::ptr::read_volatile(sp as *const u64) };
            context.Rsp += 8;
        } else {
            let mut handler_data = std::ptr::null_mut();
            let mut establisher = 0u64;
            unsafe {
                RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, ip, entry, &mut context, &mut handler_data, &mut establisher, std::ptr::null_mut());
            }
        }
        if context.Rip == ip as u64 {
            break;
        }
    }
    frames
}
