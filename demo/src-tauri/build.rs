fn main() {
    // With the `cuda` feature, llama.cpp/ggml and the CUDA runtime are shared
    // libraries. `riga-server`'s build script stages the llama.cpp `.so` files
    // next to this executable (`$ORIGIN`) and Tauri installs them to
    // `/usr/lib/riga` in the `.deb`. `--disable-new-dtags` makes the rpath
    // `DT_RPATH`, which the loaded libraries inherit. Harmless no-ops for CPU
    // and Metal builds.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/riga");
    }
    tauri_build::build()
}
