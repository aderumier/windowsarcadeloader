fn main() {
    // The games import the HASP HL API by ordinal (see hasp.def).
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hasp.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
