//! The I36I board (Winning Eleven 2012): the cabinet's PlayStation 3 controller port, a USB hub
//! the board switches between itself and the PC. No pad is plugged in: its status buffer stays
//! empty, the USB device descriptor read through the board is a DualShock 3's (as spice2x
//! answers it). The PS2 pad (PJEC) and the arcade stick (PJEI) are the controllers played.

/// The status buffer (48 bytes, all zero: no pad).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_get_control_status_buffer(out: *mut u8) -> bool {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 48) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_update_control_status_buffer() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_create_get_status_thread() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_destroy_get_status_thread() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_ps3_controller_pwr_on() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_ps3_controller_pwr_off() -> bool {
    true
}

/// The pad's USB link to the board.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_usb_controller_bus_IO() -> bool {
    true
}

/// The pad's USB link to the PC.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_usb_controller_bus_PC() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_req_get_usb_desc(_kind: i32) -> bool {
    true
}

/// A DualShock 3's USB device descriptor.
const DS3_DESCRIPTOR: [u8; 18] = [
    0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x4c, 0x05, 0x68, 0x02, 0x00, 0x01, 0x01, 0x02, 0x00, 0x01,
];

/// The descriptor read: `state` 3 (done), the bytes in `data` (at most `size`).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36i_req_get_usb_desc_isfinished(
    _kind: i32, state: *mut u32, _a3: i32, out_size: *mut u32, data: *mut u8, size: u32,
) -> bool {
    let n = DS3_DESCRIPTOR.len().min(size as usize);
    unsafe {
        if !data.is_null() {
            std::ptr::copy_nonoverlapping(DS3_DESCRIPTOR.as_ptr(), data, n);
        }
        if !out_size.is_null() {
            *out_size = n as u32;
        }
        if !state.is_null() {
            *state = 3;
        }
    }
    true
}
