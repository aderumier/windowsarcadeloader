fn main() {
    // Export Inp32 / Out32 under the ordinals of inpout32.dll (see inpout32.def).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("inpout32.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
