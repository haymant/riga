#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

# Load local settings (provider keys, RIGA_TOKEN, overrides). Writes and shell
# are gated by the approval flow, not by environment variables.
if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

export RIGA_SERVER_ADDRESS="${RIGA_SERVER_ADDRESS:-127.0.0.1:8787}"
exec cargo run -p riga-server
