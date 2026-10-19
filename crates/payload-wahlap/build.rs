fn main() {
    // Export the UsbIO functions under their MSVC names (see usb_io.def).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("usb_io.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
