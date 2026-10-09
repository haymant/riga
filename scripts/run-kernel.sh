#!/usr/bin/env bash
set -euo pipefail

# Operate from the repository root so `cargo run` resolves the workspace and
# `.env.local` is found regardless of the caller's working directory (for
# example `npm run dev` from `demo/`).
cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ -f "$HOME/.cargo/env" ]]; then
  # rustup installs this file; distro-provided cargo does not need it.
  source "$HOME/.cargo/env"
fi

# Load local settings (provider keys, RIGA_TOKEN, overrides). Writes and shell
# are gated by the approval flow, not by environment variables.
if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

export RIGA_SERVER_ADDRESS="${RIGA_SERVER_ADDRESS:-127.0.0.1:8787}"

# Fail fast if the address is already taken. Without this, a second
# `npm run dev` (or `tauri:dev`) spends the whole kernel build and then the
# process panics on `bind`, which reads as a crash and buries the real cause.
host="${RIGA_SERVER_ADDRESS%:*}"
port="${RIGA_SERVER_ADDRESS##*:}"
if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then
  exec 3>&- 3<&-
  echo "RIGA kernel: ${RIGA_SERVER_ADDRESS} is already in use." >&2
  echo "Another kernel is running. Stop it, or set RIGA_SERVER_ADDRESS to a free address." >&2
  exit 1
fi

exec cargo run -p riga-server
