fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
    // Wire: the framework archive compiled into the binary (src/wire_framework.rs).
    // Empty when WIRE_FRAMEWORK_ARCHIVE is unset, so an ordinary build still works.
    println!("cargo:rerun-if-env-changed=WIRE_FRAMEWORK_ARCHIVE");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dest = out.join("wire-framework.tar.gz");
    match std::env::var("WIRE_FRAMEWORK_ARCHIVE") {
        Ok(path) if !path.is_empty() => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::copy(&path, &dest).expect("copying WIRE_FRAMEWORK_ARCHIVE");
        }
        _ => {
            std::fs::write(&dest, b"").expect("writing an empty framework archive");
        }
    }
}
