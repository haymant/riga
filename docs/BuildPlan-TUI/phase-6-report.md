

## Manual-test regression pass

A follow-up manual-test pass fixed the issues found in the first TUI-6 build:

- The composer now positions the terminal cursor at the UTF-8-safe insertion point, including multiline drafts.
- Start-run and resume errors stay in the TUI as an `ERROR:` footer toaster instead of propagating out of the event loop and terminating the application.
- Shift+Enter is handled before submit and inserts a newline; Enter remains submit.
- Plain `r` and `t` are composer input. Reasoning/tools toggles use Ctrl+R/Ctrl+T.
- llama.cpp backend logs are voided while the server owns the terminal, preventing tensor repack diagnostics from corrupting the alternate screen.
- Ctrl+H history also handles terminals that encode Ctrl+H as Backspace when the composer is empty. F2 and Ctrl+S provide settings fallbacks for terminals that do not transmit Ctrl-comma reliably.
- Escape closes panels, but from the transcript it opens a y/n quit confirmation rather than exiting immediately.
- CUDA CLI builds have a wrapper that validates the required toolchain and repairs stale CMake output with a missing Makefile/build.ninja.

Automated coverage now includes cursor/toaster rendering, plain composer `r`/`t`, Shift+Enter, Ctrl+R/Ctrl+T, Escape confirmation, F2 settings, local-model selection, and a selected-downloaded-model flow that submits two chat threads in the same session. The focused CLI suite passes with **25 tests, 0 failures**.
