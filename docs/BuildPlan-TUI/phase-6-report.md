# TUI Phase 6 Report

Status: **PASS**

TUI-6 adds protocol-backed local-model controls and session-scoped attachment upload. It also makes provider/model selection discoverable from the transcript and README.

## User-facing controls

- `Ctrl+,` opens Provider settings; the third field is the model identifier. Tab/Shift+Tab moves between fields and `Ctrl+S` saves.
- `Ctrl+L` opens Local models and attachments.
- In the local-model panel, Enter/d downloads a catalog model or loads an installed model, x cancels a download, u unloads the current model, and a starts attachment-path entry.
- Loading an installed model selects its model id, switches the provider kind to local, and persists that provider selection through the existing secure IPC configuration operation.
- The panel shows CPU/CUDA accelerator label, loaded model, installed models, catalog metadata, download status, and inline errors.
- Attachments are read from the selected local path, uploaded through `IpcService::upload_attachment`, stored in the selected session worktree, and inserted into the draft as a workspace attachment path.

## Implementation

Local-model overview, actions, progress events, and attachment upload were added to the generic `RigaTransport` seam. The TUI consumes the existing IPC service; no TUI-only endpoint or second local-model runtime was added. Download progress and completion/failure events are consumed from the shared local-model broadcast stream, while the overview is refreshed after actions.

The help panel and persistent transcript footer now explicitly advertise settings/model (`Ctrl+,`) and local models (`Ctrl+L`). README instructions explain remote model editing, local GGUF loading/downloading, CPU/CUDA startup, and attachment controls.

## Files changed

- `README.md`
- `crates/riga-cli/src/app.rs`
- `crates/riga-cli/src/main.rs`
- `crates/riga-cli/src/model.rs`
- `crates/riga-cli/src/transport.rs`
- `crates/riga-cli/src/ui.rs`
- `crates/riga-server/src/lib.rs`
- `docs/BuildPlan-TUI/BuildPlan-TUI.md`
- `docs/BuildPlan-TUI/phase-6-report.md`

## Validation

```text
cargo fmt --all -- --check
cargo test -p riga-cli
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The focused CLI suite passes with **21 tests, 0 failures**, including local-model selection. The full workspace gate is the final merge check for this phase.

## Known limitations

The local-model panel does not expose manual GGUF path installation because the shared service intentionally supports only curated downloads and loading installed paths. Attachment upload is path-based rather than a terminal file picker; this is explicit and works without mouse support. WebSocket transport remains outside the CLI adapter seam.

## Next phase

TUI-7 is allowed: hardening, terminal snapshots, protocol conformance, release documentation, and full validation coverage.
