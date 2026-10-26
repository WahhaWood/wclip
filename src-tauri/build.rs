fn main() {
    // Start without `vulkan-1.dll`; `src/vulkan_loader.rs` handles its absence (ADR 0004).
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    tauri_build::build()
}
