from pathlib import Path

kernel_manifest = Path("crates/riga-kernel/Cargo.toml").read_text()
for forbidden in ("tauri", "axum", "actix", "browser", "react"):
    if forbidden in kernel_manifest.lower():
        raise SystemExit(f"forbidden kernel dependency marker: {forbidden}")
print("riga-kernel dependency boundary passed")
