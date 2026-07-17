#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATABASE_PATH="$ROOT_DIR/data/ugr16/netflow.sqlite"
LOG_PATH="$ROOT_DIR/data/ugr16/netflow.build.log"
PID_PATH="$ROOT_DIR/data/ugr16/netflow.build.pid"
CONFIG_PATH="$ROOT_DIR/scripts/local/ugr16-csv.pipeline.json"
DETACH=0

usage() {
  cat <<'USAGE'
Usage: scripts/local/build_ugr16_netflow.sh [--detach] [--config PATH] [--database PATH] [--log PATH] [--pid PATH]

Build the UGR'16 pipeline SQLite database from a local CSV config.
Create scripts/local/ugr16-csv.pipeline.json, or pass --config PATH.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --detach)
      DETACH=1
      shift
      ;;
    --database)
      DATABASE_PATH="$2"
      shift 2
      ;;
    --config)
      CONFIG_PATH="$2"
      shift 2
      ;;
    --log)
      LOG_PATH="$2"
      shift 2
      ;;
    --pid)
      PID_PATH="$2"
      shift 2
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

mkdir -p "$(dirname "$DATABASE_PATH")" "$(dirname "$LOG_PATH")" "$(dirname "$PID_PATH")"

run_build() {
  cd "$ROOT_DIR"
  if [[ ! -f "$CONFIG_PATH" ]]; then
    echo "missing local pipeline config: $CONFIG_PATH" >&2
    echo "create scripts/local/ugr16-csv.pipeline.json or pass --config PATH" >&2
    exit 1
  fi
  ./scripts/build_maad_fast.sh
  ./scripts/build_nfdump_reducer.sh
  exec ./scripts/run-with-nix-if-available.sh uv run python -u \
    tools/netflow-db/pipeline.py \
    --config "$CONFIG_PATH" \
    --database-path "$DATABASE_PATH"
}

if [[ "$DETACH" -eq 1 ]]; then
  if [[ -s "$PID_PATH" ]] && kill -0 "$(cat "$PID_PATH")" 2>/dev/null; then
    echo "build already running: pid $(cat "$PID_PATH")" >&2
    exit 1
  fi
  setsid "$0" --config "$CONFIG_PATH" --database "$DATABASE_PATH" --log "$LOG_PATH" --pid "$PID_PATH" >"$LOG_PATH" 2>&1 < /dev/null &
  echo "$!" >"$PID_PATH"
  echo "started UGR16 build pid $(cat "$PID_PATH")"
  echo "database: $DATABASE_PATH"
  echo "log: $LOG_PATH"
  exit 0
fi

run_build
