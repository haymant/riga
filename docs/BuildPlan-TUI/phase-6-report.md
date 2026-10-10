

## Progress and CUDA follow-up

Long-running local-model actions now run behind the TUI event loop instead of being awaited in the key/render loop. Loading and unloading show an animated Ratatui gauge, downloads show a percentage Gauge sourced from the shared local-model progress stream, and completion/failure clears or updates the operation state. Starting a chat run is also dispatched in the background; the footer shows `Starting...`, `Thinking...`, or `Waiting for approval...` while events arrive. The UI continues polling keys and redrawing during these operations.

The CUDA wrapper now passes the generated compatibility include directory through both `CUDAFLAGS` and `CMAKE_CUDA_FLAGS`, because `llama-cpp-sys-2`'s CMake compiler-identification step does not reliably inherit the former alone. It also invalidates a cached CMake tree when the cache predates the shim or is not configured with CUDA. This addresses the reported CUDA 12.6/glibc conflict for `cospi`, `sinpi`, `rsqrt`, `cospif`, `sinpif`, and `rsqrtf`; the wrapper remains the canonical CUDA CLI entry point.

The focused CLI suite now passes with **26 tests, 0 failures**, including the local-model progress gauge. Workspace clippy and tests also pass.
