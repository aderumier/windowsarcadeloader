fn main() {
    // the amVideo library's export ordinals (see amvideo.def)
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("amvideo.def");
    println!("cargo:rerun-if-changed={}", def.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
