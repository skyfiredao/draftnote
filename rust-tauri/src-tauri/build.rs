fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if cfg!(feature = "desktop") || target_os == "ios" {
        tauri_build::build();
    }
}
