//! `WAL_NAMCOES3_GCB_TRACE=1`: every call of Mobile Suit Gundam U.C. Card Builder into its
//! cabinet libraries logged (arguments, result, caller), for bring-up: the imports of
//! `bnAS6DB.dll` (coin selector), `libUIMT.dll` (touch panel), `nbamsavdat.dll` (backup data)
//! and the `API_*` functions of `Y3CodeReaderNE.dll` (card reader) the game resolves with
//! `GetProcAddress`. Each wrapper forwards eight argument slots (the Windows x64 convention: four
//! registers, then the caller's stack; the callee reads only its own).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

type Raw = unsafe extern "system" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64;

macro_rules! traced {
    ($( $id:ident = $name:literal ),* $(,)?) => {
        $(
            #[allow(non_snake_case)]
            mod $id {
                use super::*;
                pub(super) static REAL: AtomicUsize = AtomicUsize::new(0);
                pub(super) unsafe extern "system" fn wrap(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64, h: u64) -> u64 {
                    let real: Raw = unsafe { std::mem::transmute(REAL.load(Ordering::Relaxed)) };
                    let r = unsafe { real(a, b, c, d, e, f, g, h) };
                    super::logged($name, [a, b, c, d, e, f], r);
                    r
                }
            }
        )*
        const TRACED: &[(&str, &AtomicUsize, Raw)] = &[ $( ($name, &$id::REAL, $id::wrap as Raw) ),* ];
    };
}

traced! {
    as6_finalize = "bnAS6DBFinalize",
    as6_api = "bnAS6DBGetAPIVersion",
    as6_error = "bnAS6DBGetErrorInfo",
    as6_input = "bnAS6DBGetInputInfo",
    as6_init = "bnAS6DBInitialize",
    as6_lockout = "bnAS6DBReqControlFlagsLockout",
    as6_update = "bnAS6DBUpdate",
    ui_info = "UI_GetDeviceInformation",
    ui_restart = "UI_RestartDevice",
    sav_new = "nbamSavdatNew",
    sav_write = "nbamSavdatWrite",
    sav_api = "nbamSavdatGetAPIVersion",
    sav_read = "nbamSavdatRead",
    sav_delete = "nbamSavdatDelete",
    y3_block_read = "API_BlockRead",
    y3_block_write = "API_BlockWrite",
    y3_checksum = "API_CalcCheckSum",
    y3_calibration = "API_Calibration",
    y3_clear_error = "API_ClearError",
    y3_close = "API_Close",
    y3_connect = "API_Connect",
    y3_dll_version = "API_DLLVersion",
    y3_debug_reset = "API_DebugReset",
    y3_firmware_update = "API_FirmwareUpdate",
    y3_block_read_result = "API_GetBlockReadResult",
    y3_board_type = "API_GetBoardType",
    y3_calibration_result = "API_GetCalibrationResult",
    y3_card_data_size = "API_GetCardDataSize",
    y3_card_info = "API_GetCardInfo",
    y3_checksum_result = "API_GetCheckSumResult",
    y3_counter = "API_GetCounter",
    y3_debug_param = "API_GetDebugParam",
    y3_error_message = "API_GetErrorMessage",
    y3_firm_date = "API_GetFirmDate",
    y3_firm_name = "API_GetFirmName",
    y3_firm_version = "API_GetFirmVersion",
    y3_last_error = "API_GetLastError",
    y3_mem_counter = "API_GetMemCounter",
    y3_mem_status = "API_GetMemStatus",
    y3_parameter = "API_GetParameter",
    y3_proc_time = "API_GetProcTime",
    y3_status = "API_GetStatus",
    y3_sys_control = "API_GetSysControl",
    y3_target_code = "API_GetTargetCode",
    y3_reset = "API_Reset",
    y3_set_command = "API_SetCommand",
    y3_set_device = "API_SetDevice",
    y3_set_parameter = "API_SetParameter",
    y3_set_sys_control = "API_SetSysControl",
    y3_start = "API_Start",
    y3_stop = "API_Stop",
    y3_system_command = "API_SystemCommand",
}

/// Repeated calls (the polls) are logged the first times only.
fn logged(name: &str, args: [u64; 6], r: u64) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static COUNT: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
    let n = {
        let mut guard = COUNT.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        let n = map.entry(name.to_string()).or_insert(0);
        *n += 1;
        *n
    };
    if n <= 20 || n.is_power_of_two() {
        let a: Vec<String> = args.iter().map(|v| format!("{v:#x}")).collect();
        log!("gcb trace: {name}({}) -> {r:#x} [#{n}]", a.join(", "));
    }
}

fn find(name: &str) -> Option<&'static (&'static str, &'static AtomicUsize, Raw)> {
    TRACED.iter().find(|(n, ..)| *n == name)
}

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_proc_address(module: usize, name: *const u8) -> usize {
    let real: unsafe extern "system" fn(usize, *const u8) -> usize = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { real(module, name) };
    if f == 0 || (name as usize) < 0x10000 {
        return f;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
    if text.starts_with("API_") {
        if let Some((n, slot, wrap)) = find(&text) {
            slot.store(f, Ordering::Relaxed);
            log!("gcb trace: {n} resolved");
            return *wrap as usize;
        }
    }
    f
}

pub(crate) fn init(base: usize) {
    for dll in ["bnAS6DB.dll", "libUIMT.dll", "nbamsavdat.dll"] {
        for (name, slot, wrap) in TRACED {
            if let Some(orig) = unsafe { iat::hook_module(base, dll, name, *wrap as usize) } {
                slot.store(orig, Ordering::Relaxed);
            }
        }
    }
    match unsafe { iat::hook_module(base, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("gcb trace: GetProcAddress not hooked"),
    }
    log!("gcb trace: on");
}
