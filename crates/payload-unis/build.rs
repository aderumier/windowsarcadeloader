fn main() {
    // xinput1_3.dll's export names and ordinals (see xinput1_3.def).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("xinput1_3.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
