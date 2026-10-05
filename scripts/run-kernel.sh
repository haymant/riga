#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"
export RIGA_SERVER_ADDRESS="${RIGA_SERVER_ADDRESS:-127.0.0.1:8787}"
exec cargo run -p riga-server
