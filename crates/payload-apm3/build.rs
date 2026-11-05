fn main() {
    // Pin the export ordinals of the original apm_x86.dll (see apm_x86.def).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("apm_x86.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
        // the decorated .def names resolve Rust's undecorated exports of the same functions
        println!("cargo:rustc-cdylib-link-arg=-Wl,--enable-stdcall-fixup");
    }
}
