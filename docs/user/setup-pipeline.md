# Pipeline setup

The pipeline is the Rust `atlantis-netflow-db` crate. It converts nfcapd or CSV input into a compatible SQLite database. You can run it with Docker or build it natively.

`scripts/netflow-db-docker.sh` runs the container path. `scripts/netflow-db.sh` runs the native path with `cargo run --locked --release`, which compiles it when necessary. Set `NETFLOW_DB_BIN` to make the native wrapper run a prebuilt binary instead.

Use a new output database when you change selection rules or result semantics. The pipeline rejects incompatible reuse.

## Run the pipeline with Docker

Install the [Docker pipeline requirements](requirements.md#docker-pipeline), then complete the [dataset configuration](datasets.md). Pass each capture root before the pipeline arguments:

```bash
./scripts/netflow-db-docker.sh \
  --capture-root /absolute/path/to/captures \
  pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD>
```

The wrapper builds the image when it is missing; pass `--build` to rebuild it after a source update. Each `--capture-root` mounts read-only at the same absolute path inside the container, so `root_path` in `datasets.json` needs no change. Output stays under the repository's `data/` directory, owned by you. The image carries the nfdump fork on `PATH`, so Docker commands do not need `--nfdump`.

On macOS, bind mounts over large capture trees are slower than native filesystem access.

## Build the pipeline natively

The native path uses `scripts/netflow-db.sh` and needs the [native pipeline requirements](requirements.md#native-pipeline).

### Build the nfdump fork

nfcapd input needs the pinned ATLANTIS nfdump fork. A system nfdump installation does not work: the pipeline uses an output mode that only the fork has. CSV input does not need nfdump.

The build needs the [native nfdump fork tools](requirements.md#native-nfdump-fork). The build script checks for them and names any tool that is missing.

1. Initialize the Git submodules.

   ```bash
   git submodule update --init --recursive
   ```

2. Build the fork.

   ```bash
   ./vendor/scripts/compile-nfdump.sh
   ```

The build stages the executable at `target/nfdump/libexec/nfdump`. The `target` directory is disposable and git-ignored. Pass this path to each pipeline command with `--nfdump`; the pipeline does not find it automatically.

## Process a dataset natively

First, complete the [dataset configuration](datasets.md).

Run a bounded import while you test the configuration:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
  --nfdump target/nfdump/libexec/nfdump
```

The start date and end date are inclusive; use the same date for both to process a single day. If you omit the end date, the pipeline processes each day through the latest available day.

Dataset mode calculates MAAD statistics by default. MAAD statistics describe the multifractal structure of the observed IPv4 address sets, and they power the address-structure charts. Use `--no-maad` to skip them.

If a command fails, read [Troubleshooting](troubleshooting.md).

## Select flows

Selection conditions use AND logic. The IP prefix can match the source endpoint or the destination endpoint.

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
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

For native nfcapd input, set the top-level `"nfdump"` value to `"target/nfdump/libexec/nfdump"`. You can also pass the same path with `--nfdump` when the configuration does not set it.

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

Docker users run the same check with `./scripts/netflow-db-docker.sh`; verification reads only `data/`.

For pipeline identity and export rules, read the [pipeline contract](../code/pipeline-contract.md).
