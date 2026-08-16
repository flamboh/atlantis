# Pipeline usage

The complete data pipeline is the Rust `atlantis-netflow-db` crate. Use
`scripts/netflow-db.sh`; it runs `target/release/netflow-db` when available and
otherwise builds the pinned Cargo workspace.

Native `nfcapd` ingestion requires the pinned Atlantis nfdump fork. Build it
once with `./vendor/scripts/compile-nfdump.sh`, then pass the staged executable
explicitly with `--nfdump target/nfdump/libexec/nfdump`.

## Dataset run

```bash
./scripts/netflow-db.sh pipeline \
  --dataset uoregon \
  --start-date 2025-02-11 \
  --end-date 2025-02-12 \
  --nfdump target/nfdump/libexec/nfdump
```

Useful flags include `--database-path`, `--ip-prefix`, `--src-visibility`,
`--dst-visibility`, `--start-time`, `--end-time`, `--nfdump`, `--force`, and
`--no-maad`. `--nfdump` names the executable, not a format selector; use the
staged fork path for native input. A selected population is a distinct
database product and should use an explicit output path.

## Config run

```bash
./scripts/netflow-db.sh pipeline \
  --config scripts/local/ugr16-csv.pipeline.json \
  --database-path data/ugr16/netflow.sqlite
```

Pipeline JSON accepts explicit `csv` and `nfcapd` inputs plus `csv_tree` and
`nfcapd_tree` discovery specs. CSV mappings preserve exact millisecond
timestamps, missing measurements, named protocols, source IDs, archive member
filters, and ordered or bounded-unsorted input.

For a configuration file that contains native input, set its top-level
`"nfdump"` value to `"target/nfdump/libexec/nfdump"`, or provide the same path
with the command's `--nfdump` option when the configuration does not set it.

Selection criteria are combined with AND. The IP prefix matches either
endpoint, while source and destination visibility filters are independent.
Coverage is observed before selection, so selected-out buckets remain dense
zero buckets.

## Contracts and retries

The native adapter invokes the pinned fork with `-o atlantis`. Its stdout is the
private `atlantis-flow-stream-v1` binary contract, which Rust decodes directly
into canonical scopes before MAAD runs. There is no standalone `reduce`
command. Every input is bound to its SHA-256 content fingerprint and canonical
decoder fingerprint; ordinary replacement or in-place modification is rejected
instead of mixing products. Synthetic gaps verify continued file absence during
publication.

Each SQLite database is bound once to its schema, normalized flow selection,
pipeline timezone, decoder contract, and MAAD setting. Schema, source-layout,
or selection changes require a fresh output database.

## Window exports

```bash
./scripts/netflow-db.sh extract-window \
  --source-db data/uoregon/netflow.sqlite \
  --output-dir data/uoregon/extracts/2025-06 \
  --start 2025-06-01 \
  --end 2025-07-01 \
  --output sqlite \
  --output parquet
```

Exports use a consistent source snapshot, apply a half-open time window, and
publish a manifest plus SQLite and/or Zstd-compressed Parquet artifacts.

## Verification and promotion

```bash
./scripts/netflow-db.sh verify data/uoregon/netflow.sqlite \
  --require-data --require-processed --require-no-raw-ip

./scripts/netflow-db.sh compare \
  data/uoregon/candidate.sqlite \
  data/uoregon/historical.sqlite \
  --start 2025-11-01 \
  --end 2025-12-01

./scripts/netflow-db.sh sqlite-maintenance \
  data/uoregon/candidate.sqlite \
  data/uoregon/netflow.sqlite \
  --backup-existing data/uoregon/previous.sqlite
```

Verification exercises the same schema and representative query shapes used by
the web application. Comparison streams the shared historical schema in key
order, compares scalar values exactly, compares MAAD JSON semantically with a
configurable absolute tolerance, and reports candidate-only tables and rollup
coverage separately. Missing historical rollup buckets and dense zero scopes
are accepted as restored coverage; unexpected nonzero scopes are not. A missing
reference row or a shared-value mismatch returns a nonzero exit status.
`--start` and `--end` are half-open local date/time boundaries (or Unix
timestamps); the default timezone is `America/Los_Angeles` and the default MAAD
tolerance is `1e-10`.

Promotion creates a checked SQLite backup and atomically replaces the target
without exposing stale WAL sidecars.

## Development

```bash
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```
