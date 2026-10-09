# RIGA Coding-Agent Handoff

> **Purpose:** Give a coding agent an executable entry point for implementing RIGA phase by phase.

## Read these documents in order

1. **`PLAN-RIG-CODING-APP.md`**
   - Product scope and naming
   - assistant-ui component requirements
   - Rig-versus-RIGA responsibility boundary
   - Tauri trust boundaries
   - sandbox terminology and mandatory security requirements
   - Manus automation contract
   - testing and OpenCode Go secret policy

2. **`RIGA-BUILD-PLAN.md`**
   - Repository and package layout
   - Rust kernel architecture
   - Rig compatibility strategy
   - wire protocol
   - assistant-ui implementation plan
   - Tauri v2 integration
   - process supervisor and isolation profiles
   - HTTP/OpenAPI/Create.xyz adapter
   - testing, coverage, fuzzing, and release workflows
   - Section 12: mandatory phased roadmap and exit gates

3. **`RIGA-CODING-AGENT-HANDOFF.md`**
   - This document; use it as the execution checklist and phase-report contract.

## Implementation rule

Implement **one phase at a time** from Section 12 of `RIGA-BUILD-PLAN.md`:

0. Repository, naming, and dependency lock
1. Rig compatibility layer and fake vertical slice
2. Domain state, persistence, and replay
3. Tauri v2 adapter and transport proof
4. assistant-ui foundation and runtime parity
5. Coding tools, policy, and restricted execution
6. OS sandbox and disposable worker
7. HTTP, OpenAPI, and Create.xyz adapter
8. OpenCode Go live compatibility
9. Hardening, coverage, packaging, and release

Do not begin the next phase until the current phase’s exit gate passes.

## Mandatory phase report

At the end of every phase, create or update:

```text
Phase: N
Status: PASS | BLOCKED
Implemented:
Tests and commands:
Artifacts:
Known limitations:
Next phase allowed: YES | NO
```

A phase is **BLOCKED** when any required test, security check, API verification, or platform requirement is missing. Do not convert a skipped test or unavailable isolation backend into a pass.

## Automated validation contract

Normal validation must be unattended and non-interactive:

```bash
npm run check
npm run test:e2e
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The implementation must provide:

- deterministic fake model and fake tools;
- portable restricted worker and deterministic fake worker;
- Vitest and Testing Library tests;
- headless Playwright E2E tests against a fake kernel;
- Tauri command/channel contract tests;
- transport parity tests;
- bounded timeouts and process cleanup;
- machine-readable test and phase results.

The core UI acceptance gate must not depend on a person clicking through a desktop window. Headless Playwright, Rust/Tauri contract tests, and deterministic kernel tests are authoritative.

## Secret policy

The only expected user-secret surface is the opt-in OpenAI-compatible live integration test:

```bash
export OPENCODE_API_KEY='provided-out-of-band'
export RIGA_LIVE_LLM=1
cargo test -p riga-kernel --test opencode_go_live -- --nocapture
```

Never place the key in source files, `.env` files committed to Git, command-line arguments, chat messages, issue comments, logs, screenshots, fixtures, coverage reports, or artifacts.

Normal tests must pass without network access, an OpenCode account, or `OPENCODE_API_KEY`.

## Manus environment constraints

A Manus sandbox may not expose Docker, a VM, privileged namespaces, a display server, CUDA, or a target platform’s native sandbox facility. The implementation must feature-detect these capabilities.

- Use portable restricted/fake workers for ordinary deterministic tests.
- Run real OS-sandbox conformance tests when the backend is available.
- Report `OS_BACKEND_UNAVAILABLE` when it is not available.
- Never claim OS isolation was validated by fake-worker tests.
- Keep container/microVM, signed installer, CUDA, and protected live-provider workflows automated in GitHub Actions.

## Do not reinvent Rig

Use the pinned Rig release for:

- provider clients and completion models;
- normalized messages;
- agent/tool execution loop;
- streaming;
- hooks and flow actions;
- memory contracts;
- structured output;
- mock models and scripted turns.

RIGA owns product orchestration, workspace/security policy, durable sessions, approvals, event journaling, transports, UI integration, and recovery. Do not create a second provider abstraction, agent loop, generic memory layer, or generic streaming framework without an ADR explaining why Rig cannot provide it.

## Completion definition

The implementation is ready for release only when:

- all phase gates pass;
- deterministic CI passes from a clean checkout;
- the headless Playwright flow covers streaming, tools, approvals, diffs, artifacts, cancellation, reconnect, replay, and accessibility;
- Tauri commands and channels pass contract tests;
- privileged tools are policy-checked and audited;
- restricted, OS-sandboxed, and disposable execution modes are clearly reported;
- OpenCode Go live tests pass only when explicitly enabled with a protected secret;
- coverage, fuzzing, stress, dependency, secret, and mutation gates pass or have documented release exceptions;
- aligned kernel, UI, server, and desktop artifacts are reproducibly generated.
