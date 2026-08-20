fn main() {
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-l:libpulse-simple.so.0");
        println!("cargo:rustc-link-arg=-l:libpulse.so.0");
    }
}
