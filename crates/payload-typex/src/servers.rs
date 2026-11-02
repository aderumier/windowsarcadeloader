//! Servers the cabinet starts beside the game (`WAL_TYPEX_SERVERS`): Usagi Online's launcher
//! (game.exe) starts the game with its local message relay and lobby daemons, which the game
//! connects to before anything else (its network check screen never ends without them).
//!
//! `WAL_TYPEX_SERVERS=<program> <args>;...`: programs of the game directory (paths relative to
//! it), each started by the game's payload in its own folder, without a window, in a job that
//! ends them with the game. They run without the payload: a D: drive they need is mapped by
//! the profile (`drives`).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use wal_payload_common::log;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain([0]).collect()
}

pub(crate) fn init() {
    let Ok(list) = std::env::var("WAL_TYPEX_SERVERS") else { return };
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    // the job handle stays open until the game exits: the servers die with it
    let job = unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32);
        job
    };
    for entry in list.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let (program, args) = entry.split_once(' ').unwrap_or((entry, ""));
        let path = dir.join(program.replace('/', "\\"));
        let Some(cwd) = path.parent() else { continue };
        let mut line = wide(format!("\"{}\" {}", path.display(), args.trim()).as_ref());
        let cwd = wide(cwd.as_os_str());
        unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(std::ptr::null(), line.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_NO_WINDOW, std::ptr::null(), cwd.as_ptr(), &si, &mut pi) == 0 {
                log!("servers: cannot start {entry}");
                continue;
            }
            AssignProcessToJobObject(job, pi.hProcess);
        }
        log!("servers: {entry} started");
    }
}
