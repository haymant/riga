//! Runtime search path for the dynamically linked CUDA build.
//!
//! When built with `riga-server/cuda`, llama.cpp/ggml and the CUDA runtime are
//! shared libraries rather than being linked into the binary. `riga-server`'s
//! build script stages the llama.cpp `.so` files next to this executable, so
//! `$ORIGIN` resolves them; `/usr/lib/riga` is where the Linux `.deb` installs
//! them. `--disable-new-dtags` emits `DT_RPATH` (inherited by the loaded
//! libraries) instead of `DT_RUNPATH`, so `libllama.so` finds its sibling
//! `libggml.so`. The CPU build has no such libraries and ignores the entries.
//!
//! `rustc-link-arg` is not inherited from dependencies, so each runnable crate
//! emits its own.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/riga");
}
