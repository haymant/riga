#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

# Load local capability flags. README documents RIGA_ENABLE_WRITES and
# RIGA_ENABLE_SHELL in .env.local, but nothing read that file, so the flags had
# no effect and writes/shell were always disabled under `npm run dev`.
if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

export RIGA_SERVER_ADDRESS="${RIGA_SERVER_ADDRESS:-127.0.0.1:8787}"
exec cargo run -p riga-server
