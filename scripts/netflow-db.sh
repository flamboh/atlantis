#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -n "${NETFLOW_DB_BIN:-}" ]]; then
  exec "$NETFLOW_DB_BIN" "$@"
fi

cd "$ROOT_DIR"
exec "$ROOT_DIR/scripts/run-with-nix-if-available.sh" \
  cargo run --locked --release --package atlantis-netflow-db -- "$@"
