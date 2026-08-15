#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
cargo build --locked --release --package atlantis-netflow-db --bin netflow-db
printf '%s\n' 'MAAD is available in process and as: scripts/netflow-db.sh maad'
