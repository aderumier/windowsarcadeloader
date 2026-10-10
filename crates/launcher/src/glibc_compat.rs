//! libm functions the GUI's float math (egui) calls, that glibc 2.43+ versions anew: built here
//! (the libm crate), the launcher then runs with an older glibc (Batocera 43: 2.40).

#![allow(clippy::missing_safety_doc)]

#[unsafe(no_mangle)]
pub extern "C" fn acosf(x: f32) -> f32 {
    libm::acosf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn asinf(x: f32) -> f32 {
    libm::asinf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn atan2f(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[unsafe(no_mangle)]
pub extern "C" fn coshf(x: f32) -> f32 {
    libm::coshf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn sinhf(x: f32) -> f32 {
    libm::sinhf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn acoshf(x: f32) -> f32 {
    libm::acoshf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn cosh(x: f64) -> f64 {
    libm::cosh(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn sinh(x: f64) -> f64 {
    libm::sinh(x)
}
