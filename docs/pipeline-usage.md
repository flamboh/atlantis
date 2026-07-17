# Pipeline usage

The pipeline lives in `tools/netflow-db/pipeline.py`.

It processes explicit CSV inputs or prepared nfcapd trees into the canonical
SQLite stats tables.

## Dataset run

Use `datasets.json` for dataset roots and output paths, then run a bounded
nfcapd tree ingest:

```bash
python tools/netflow-db/pipeline.py \
  --dataset uoregon \
  --start-date 2025-02-11 \
  --end-date 2025-02-12
```

Useful flags:

- `--database-path`: override the SQLite output path
- `--start-time` / `--end-time`: limit a half-open local time window. These must
  align to aggregate bucket boundaries so coarse rows stay complete.
- `--maad-bin`: path to the MAAD helper binary
- `--max-workers`: worker process count
- `--force`: rewrite selected nfcapd buckets even when marked processed

## Config run

For CSV imports or mixed inputs, pass a pipeline config:

```bash
python tools/netflow-db/pipeline.py \
  --config scripts/local/ugr16-csv.pipeline.json \
  --database-path data/ugr16/netflow.sqlite
```

Local helper:

```bash
scripts/local/build_ugr16_netflow.sh --config scripts/local/ugr16-csv.pipeline.json
```

### CSV duration mapping

The optional logical `columns.duration` mapping is measured in seconds. Decimal
seconds are converted exactly to integer milliseconds, so values may have at
most millisecond precision. A mapped, nonblank duration is authoritative for
the duration value, while mapped endpoints must still satisfy
`time_end >= time_start`. When duration is absent, ingestion derives it from
mapped `time_start` and `time_end` timestamps when both are available.

## Compile helpers

The canonical MAAD helper is built with:

```bash
scripts/build_maad_fast.sh
```

`nfdump` must be on `PATH` for nfcapd inputs. Use
`scripts/run-with-nix-if-available.sh` when the local environment needs Nix
tooling.

## Sanity check

If you edited backend Python, run:

```bash
cd tools/netflow-db
python -m py_compile *.py
```
