#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
cargo build --locked --release --package atlantis-netflow-db --bin netflow-db
printf '%s\n' 'The reducer is available as: scripts/netflow-db.sh reduce'
