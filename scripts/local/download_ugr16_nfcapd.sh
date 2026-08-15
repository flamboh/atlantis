#!/usr/bin/env bash
set -euo pipefail

BASE_URL="https://nesg.ugr.es/nesg-ugr16/"
DEST_DIR="${RAW_DATASETS_PATH:-/research/obo/raw_datasets}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
URL_LIST_NAME="ugr16-nfcapd-urls.txt"
URLS_ONLY=0
DOWNLOADER=""

usage() {
  cat <<'EOF'
Usage: download_ugr16_nfcapd.sh [options]

Download only the UGR16 weekly nfcapd archives from the NESG dataset site.

Options:
  --dest DIR          Destination directory for archives and URL manifest.
                      Default: $RAW_DATASETS_PATH or /research/obo/raw_datasets
  --urls-only         Only generate the URL manifest; do not download files.
  --downloader NAME   Force a downloader: aria2c, wget, or curl.
  -h, --help          Show this help text.

Examples:
  scripts/local/download_ugr16_nfcapd.sh
  scripts/local/download_ugr16_nfcapd.sh --urls-only
  scripts/local/download_ugr16_nfcapd.sh --dest /research/obo/raw_datasets
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dest)
      DEST_DIR="$2"
      shift 2
      ;;
    --urls-only)
      URLS_ONLY=1
      shift
      ;;
    --downloader)
      DOWNLOADER="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

mkdir -p "$DEST_DIR"
URL_LIST_PATH="$DEST_DIR/$URL_LIST_NAME"
TMP_URL_LIST="$(mktemp)"
trap 'rm -f "$TMP_URL_LIST"' EXIT

"$SCRIPT_DIR/../netflow-db.sh" scrape-ugr16 \
  --base-url "$BASE_URL" \
  --kind nfcapd \
  > "$TMP_URL_LIST"


sort -u "$TMP_URL_LIST" > "$URL_LIST_PATH"
URL_COUNT="$(wc -l < "$URL_LIST_PATH" | tr -d ' ')"

if [[ "$URL_COUNT" -eq 0 ]]; then
  echo "No nfcapd archive URLs were found." >&2
  exit 1
fi

echo "Wrote $URL_COUNT archive URLs to $URL_LIST_PATH"
echo "Destination directory: $DEST_DIR"

if [[ "$URLS_ONLY" -eq 1 ]]; then
  exit 0
fi

if [[ -z "$DOWNLOADER" ]]; then
  if command -v aria2c >/dev/null 2>&1; then
    DOWNLOADER="aria2c"
  elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
  elif command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
  else
    echo "No supported downloader found. Install aria2c, wget, or curl." >&2
    exit 1
  fi
fi

case "$DOWNLOADER" in
  aria2c)
    exec aria2c -c -j 2 -x 4 -s 4 -i "$URL_LIST_PATH" -d "$DEST_DIR"
    ;;
  wget)
    exec wget \
      -c \
      --tries=0 \
      --retry-connrefused \
      --waitretry=30 \
      --read-timeout=60 \
      --timeout=60 \
      -i "$URL_LIST_PATH" \
      -P "$DEST_DIR"
    ;;
  curl)
    while IFS= read -r url; do
      filename="${url##*/}"
      echo "Downloading $filename"
      curl -fL --retry 10 --retry-delay 30 -C - -o "$DEST_DIR/$filename" "$url"
    done < "$URL_LIST_PATH"
    ;;
  *)
    echo "Unsupported downloader: $DOWNLOADER" >&2
    exit 1
    ;;
esac
