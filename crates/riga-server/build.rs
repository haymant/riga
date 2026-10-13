//! Stage llama.cpp's shared libraries next to the runnable binaries and for the
//! Linux `.deb`.
//!
//! With the `cuda` feature, `riga-server` turns on `llama-cpp-2/dynamic-link`,
//! which builds llama.cpp/ggml as shared libraries and links the CUDA runtime
//! (cuBLAS/cuBLASLt/cudart) dynamically. The binary crates emit a `DT_RPATH` of
//! `$ORIGIN:/usr/lib/riga`, so the libraries must sit next to the executable
//! (and in its `deps/` sibling, where test binaries run).
//!
//! `llama-cpp-sys-2` already tries to stage them, but it hard-links the CMake
//! *symlinks*, which leaves dangling links if a build is interrupted or pruned.
//! This script instead copies real sonames (the loader's lookup name), which is
//! deterministic. A stable `riga-cuda-libs/` directory is also populated for the
//! `.deb`, which maps it to `/usr/lib/riga`; it is always created (with a
//! placeholder) so the unconditional Tauri config also resolves on CPU builds.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    emit_rpath();

    // <target>/<profile>/build/riga-server-<hash>/out -> <target>/<profile>
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let Some(profile_dir) = out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
    else {
        return;
    };

    // Always stage a directory for the .deb mapping, even when there is nothing
    // to put in it, so `bundle.linux.deb.files` never points at a missing path.
    // Clear it first so switching a shared target directory between CPU and CUDA
    // builds cannot leak libraries into the wrong `.deb`.
    let staging = profile_dir.join("riga-cuda-libs");
    clear_staging(&staging);

    // The shared CUDA libraries only exist when the target is Linux and the CUDA
    // backend was built (the only configuration that enables `dynamic-link`).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || std::env::var_os("CARGO_FEATURE_CUDA").is_none()
    {
        stage_placeholder(&staging);
        return;
    }

    let libs = collect_sonames(&profile_dir.join("build"));
    if libs.is_empty() {
        println!(
            "cargo:warning=cuda feature is on but no llama.cpp shared libraries were found; \
             expected llama-cpp-2/dynamic-link to build them"
        );
        stage_placeholder(&staging);
        return;
    }

    let destinations = [
        profile_dir.clone(),
        profile_dir.join("deps"),
        // `llama-cpp-sys-2` copies only the unversioned `libX.so` symlink here,
        // which dangles unless the versioned soname also lands in the directory;
        // otherwise its next build script run panics on `hard_link` (EEXIST).
        profile_dir.join("examples"),
        staging,
    ];
    for lib in &libs {
        for dest in &destinations {
            place_real(lib, dest);
        }
    }
}

/// Make dynamically linked llama.cpp/CUDA libraries resolvable at runtime.
///
/// `$ORIGIN` finds the `.so` files staged next to the binary; `/usr/lib/riga` is
/// where the Linux `.deb` installs them. An entry that does not exist on a CPU
/// build is harmless.
///
/// `--disable-new-dtags` makes the linker emit `DT_RPATH` instead of
/// `DT_RUNPATH`. Unlike `RUNPATH`, `RPATH` is inherited by the libraries we
/// load, so `libllama.so` resolves its sibling `libggml.so` from the same
/// directory even though the installed llama.cpp libraries carry no rpath.
fn emit_rpath() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/riga");
}

/// Place `lib` under `dest_dir` as a real file (hard link when possible).
///
/// Any pre-existing entry is removed first so a dangling symlink left by
/// `llama-cpp-sys-2` cannot shadow the library or make a later hard link fail.
fn place_real(lib: &Path, dest_dir: &Path) {
    let Some(name) = lib.file_name() else {
        return;
    };
    let _ = std::fs::create_dir_all(dest_dir);
    let dest = dest_dir.join(name);
    let _ = std::fs::remove_file(&dest);
    let real = std::fs::canonicalize(lib).unwrap_or_else(|_| lib.to_path_buf());
    if std::fs::hard_link(&real, &dest).is_err() {
        let _ = std::fs::copy(lib, &dest);
    }
}

/// Write an explanatory placeholder so the staging directory is never empty.
fn stage_placeholder(staging: &Path) {
    let _ = std::fs::write(
        staging.join("README.txt"),
        "llama.cpp shared libraries are staged here when the CUDA backend is built \
         with dynamic linking; Tauri installs this directory to /usr/lib/riga.\n",
    );
}

/// Create the staging directory empty of staged libraries and placeholders.
fn clear_staging(staging: &Path) {
    let _ = std::fs::create_dir_all(staging);
    let Ok(entries) = std::fs::read_dir(staging) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.contains(".so") || name == "README.txt" {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Collect the shared-object sonames from llama.cpp's CMake install tree.
///
/// Only the loader's lookup name (`libX.so.<major>`) is needed at runtime, so
/// the versioned full name and unversioned `libX.so` symlink are skipped. The
/// `out/lib` install tree is used rather than `out/build/bin`, whose libraries
/// carry an absolute rpath to the build directory.
fn collect_sonames(build_dir: &Path) -> Vec<PathBuf> {
    let mut libs = Vec::new();
    let Ok(entries) = std::fs::read_dir(build_dir) else {
        return libs;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("llama-cpp-sys-2-") {
            continue;
        }
        walk(&entry.path().join("out").join("lib"), &mut libs);
    }
    libs
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && is_soname(name)
        {
            out.push(path);
        }
    }
}

/// `libfoo.so.1` (a runtime soname), but not `libfoo.so` or `libfoo.so.1.2.3`.
fn is_soname(name: &str) -> bool {
    let Some((_, version)) = name.strip_prefix("lib").and_then(|n| n.split_once(".so.")) else {
        return false;
    };
    !version.is_empty() && !version.contains('.')
}
