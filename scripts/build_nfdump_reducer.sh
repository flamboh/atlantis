#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_PATH="$ROOT_DIR/tools/netflow-db/nfdump_reducer.cpp"
OUTPUT_PATH="$ROOT_DIR/tools/netflow-db/nfdump_reducer"
CXX="${CXX:-g++}"
FLAGS=(-O3 -std=c++17 -Wall -Wextra -Wpedantic -Werror)
BUILD_ID_PATH="$OUTPUT_PATH.build-id"
EXPECTED_VERSION="nfdump_reducer 1 nfdump-csv-15-v1 canonical-scopes-v1"
BUILD_KEY="$(
  {
    printf '%s\n' "$CXX" "${FLAGS[*]}"
    "$CXX" --version
    sha256sum "$SOURCE_PATH"
  } | sha256sum | cut -d' ' -f1
)"

if [[ -x "$OUTPUT_PATH" && -f "$BUILD_ID_PATH" && "$(cat "$BUILD_ID_PATH")" == "$BUILD_KEY" ]]; then
  exit 0
fi

TEMP_OUTPUT="$(mktemp "$OUTPUT_PATH.tmp.XXXXXX")"
TEMP_BUILD_ID="$(mktemp "$BUILD_ID_PATH.tmp.XXXXXX")"
trap 'rm -f "$TEMP_OUTPUT" "$TEMP_BUILD_ID"' EXIT

"$CXX" "${FLAGS[@]}" -o "$TEMP_OUTPUT" "$SOURCE_PATH"
chmod +x "$TEMP_OUTPUT"
if [[ "$("$TEMP_OUTPUT" --version)" != "$EXPECTED_VERSION" ]]; then
  echo "built nfdump reducer reported an unexpected contract" >&2
  exit 1
fi
printf '%s\n' "$BUILD_KEY" >"$TEMP_BUILD_ID"
mv -f "$TEMP_OUTPUT" "$OUTPUT_PATH"
mv -f "$TEMP_BUILD_ID" "$BUILD_ID_PATH"
