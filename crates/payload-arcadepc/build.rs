fn main() {
    // xinput1_3.dll's export names and ordinals (see xinput1_3.def); the x86 build is also
    // Frenzy Express's xio.dll (xio.def).
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let def = dir.join("xinput1_3.def");
    let xio = dir.join("xio.def");
    println!("cargo:rerun-if-changed={}", def.display());
    println!("cargo:rerun-if-changed={}", xio.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86") {
        let both = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("wal_arcadepc32.def");
        let text = std::fs::read_to_string(&def).unwrap() + &std::fs::read_to_string(&xio).unwrap();
        std::fs::write(&both, text).unwrap();
        println!("cargo:rustc-cdylib-link-arg={}", both.display());
    } else {
        println!("cargo:rustc-cdylib-link-arg={}", def.display());
    }
}
