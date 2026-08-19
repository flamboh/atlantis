# Pipeline setup

The pipeline is the Rust `atlantis-netflow-db` crate. It converts nfcapd or CSV input into a compatible SQLite database.

`scripts/netflow-db.sh` is the pipeline entry point. It runs the crate with `cargo run --locked --release`, which compiles it when necessary. The first run compiles the Rust dependencies and takes several minutes. Set `NETFLOW_DB_BIN` to run a prebuilt binary instead.

Use a new output database when you change selection rules or result semantics. The pipeline rejects incompatible reuse.

## Build the nfdump fork

nfcapd input needs the pinned ATLANTIS nfdump fork. A system nfdump installation does not work: the pipeline uses an output mode that only the fork has. CSV input does not need nfdump.

1. Initialize the Git submodules.

   ```bash
   git submodule update --init --recursive
   ```

2. Build the fork.

   ```bash
   ./vendor/scripts/compile-nfdump.sh
   ```

The build stages the executable at `target/nfdump/libexec/nfdump`. The `target` directory is disposable and git-ignored. Pass this path to each pipeline command with `--nfdump`; the pipeline does not find it automatically.

## Process a dataset

First, complete the [dataset configuration](datasets.md).

Run a bounded import while you test the configuration:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date 2025-02-01 \
  --end-date 2025-02-01 \
  --nfdump target/nfdump/libexec/nfdump
```

The start date and end date are inclusive, so this command processes one day. If you omit the end date, the pipeline processes each day through the latest available day.

Dataset mode calculates MAAD statistics by default. MAAD statistics describe the multifractal structure of the observed IPv4 address sets, and they power the address-structure charts. Use `--no-maad` to skip them.

If a command fails, read [Troubleshooting](troubleshooting.md).

## Select flows

Selection conditions use AND logic. The IP prefix can match the source endpoint or the destination endpoint.

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date 2025-02-01 \
  --end-date 2025-02-01 \
  --database-path data/example-public/netflow.sqlite \
  --ip-prefix 192.0.2.0/24 \
  --src-visibility literal \
  --nfdump target/nfdump/libexec/nfdump
```

A selected population is a different database product. Thus, selection options require an explicit `--database-path`.

Available selection options are:

- `--ip-prefix`
- `--src-visibility literal|anonymized`
- `--dst-visibility literal|anonymized`

## Use a pipeline configuration

Configuration mode supports CSV input, nfcapd input, and mixed input. Explicit `csv` and `nfcapd` inputs and `csv_tree` and `nfcapd_tree` discovery inputs go in the top-level `inputs` list.

```bash
./scripts/netflow-db.sh pipeline \
  --config /path/to/pipeline.json \
  --database-path data/example/netflow.sqlite
```

Put flow selection in the top-level `selection` object:

```json
{
  "selection": {
    "ip_prefix": "192.0.2.0/24",
    "src_visibility": "literal",
    "dst_visibility": "anonymized"
  },
  "inputs": []
}
```

For nfcapd input, set the top-level `"nfdump"` value to `"target/nfdump/libexec/nfdump"`. You can also pass the same path with `--nfdump` when the configuration does not set it.

## Common options

| Option            | Purpose                                    |
| ----------------- | ------------------------------------------ |
| `--database-path` | Changes the SQLite output path.            |
| `--datasets`      | Reads a different dataset registry file.   |
| `--start-time`    | Sets the start of a half-open time window. |
| `--end-time`      | Sets the end of a half-open time window.   |
| `--nfdump`        | Names the nfdump executable.               |
| `--force`         | Rewrites selected nfcapd buckets.          |
| `--no-maad`       | Skips the MAAD statistics.                 |

Time limits must align with local-day boundaries.

## Verify the output

Run the compatibility check after the pipeline finishes:

```bash
./scripts/netflow-db.sh verify data/example/netflow.sqlite \
  --dataset-id example \
  --require-data \
  --require-maad-data \
  --require-processed \
  --require-rollup-parity \
  --require-no-raw-ip
```

The command prints an `OK` line when the database is compatible. A failed requirement returns a nonzero exit status.

For pipeline identity and export rules, read the [pipeline contract](../code/pipeline-contract.md).
