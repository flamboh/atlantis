#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CANDIDATE_PATH="$ROOT_DIR/data/ugr16/netflow.detached.build.sqlite"
TARGET_PATH="$ROOT_DIR/data/ugr16/netflow.sqlite"
PROMOTE=0
WEB_BASE_URL="${WEB_BASE_URL:-}"
SKIP_WEB=0

usage() {
  cat <<'USAGE'
Usage: scripts/local/finalize_ugr16_netflow.sh [--candidate PATH] [--target PATH] [--promote] [--web-base-url URL] [--skip-web]

Verify a complete UGR'16 pipeline candidate. With --promote, atomically
replace the web-facing netflow.sqlite after verification passes.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --candidate)
      CANDIDATE_PATH="$2"
      shift 2
      ;;
    --target)
      TARGET_PATH="$2"
      shift 2
      ;;
    --promote)
      PROMOTE=1
      shift
      ;;
    --web-base-url)
      WEB_BASE_URL="$2"
      shift 2
      ;;
    --skip-web)
      SKIP_WEB=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cd "$ROOT_DIR"

verify_db() {
  local db_path="$1"
  ./scripts/run-with-nix-if-available.sh uv run python \
    tools/netflow-db/verify_web_compatible.py \
    "$db_path" \
    --source-id ugr16 \
    --require-data \
    --require-maad-data \
    --require-processed \
    --require-no-raw-ip
}

verify_db "$CANDIDATE_PATH"

if [[ "$PROMOTE" -ne 1 ]]; then
  echo "candidate verified: $CANDIDATE_PATH"
  echo "rerun with --promote to replace $TARGET_PATH"
  exit 0
fi

mkdir -p "$(dirname "$TARGET_PATH")"
if [[ -e "$TARGET_PATH" ]]; then
  backup_path="$TARGET_PATH.backup.$(date -u +%Y%m%dT%H%M%SZ)"
  mv "$TARGET_PATH" "$backup_path"
  echo "backed up existing target: $backup_path"
fi

mv "$CANDIDATE_PATH" "$TARGET_PATH"
verify_db "$TARGET_PATH"
if [[ "$SKIP_WEB" -ne 1 ]]; then
  web_args=(--db-path "$TARGET_PATH")
  if [[ -n "$WEB_BASE_URL" ]]; then
    web_args+=(--base-url "$WEB_BASE_URL")
  fi
  python scripts/local/verify_ugr16_web_routes.py "${web_args[@]}"
fi
echo "promoted UGR16 database: $TARGET_PATH"
