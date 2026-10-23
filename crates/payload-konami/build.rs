fn main() {
    // Export the ACIO functions under their MSVC names too (see libacio.def): the 32-bit games'
    // libacio.dll (the 64-bit build has none).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("libacio.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86")
    {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
