# Pipeline setup

The pipeline turns nfcapd captures (or CSV) into the SQLite database that the dashboard reads.

## Choose a path

| Path   | Entry point                    | Host requirements                                                                                                  |
| ------ | ------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Docker | `scripts/netflow-db-docker.sh` | [Git and Docker](requirements.md#docker-pipeline)                                                                  |
| Native | `scripts/netflow-db.sh`        | [Rust toolchain](requirements.md#native-pipeline) and the [nfdump build tools](requirements.md#native-nfdump-fork) |

The examples below use `./scripts/netflow-db.sh`. For Docker, use `./scripts/netflow-db-docker.sh --capture-root /path/to/captures` instead. For native runs that read captures, add `--nfdump target/nfdump/libexec/nfdump`.

### Docker setup

Nothing to install beyond Docker. Put the container options before the pipeline command:

```bash
./scripts/netflow-db-docker.sh --capture-root /path/to/captures pipeline ...
```

The first run builds the image and takes several minutes. Later runs rebuild only when the pipeline sources change. The capture root mounts read-only at the same path, and output goes to the repository's `data/` directory.

### Native setup

Build the pinned nfdump fork. A system nfdump does not work.

```bash
git submodule update --init --recursive
./vendor/scripts/compile-nfdump.sh
```

The fork is staged at `target/nfdump/libexec/nfdump`. The first pipeline command compiles the Rust crate, which takes several minutes.

## Build a database

First, [configure the dataset](datasets.md). Then process a few days:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD>
```

- Both dates are inclusive. Omit `--end-date` to process through the latest capture.
- The output goes to `data/<dataset-id>/netflow.sqlite`.
- Rerunning the same command skips days that are already done.
- Add `--no-maad` to skip the address-structure (MAAD) statistics and finish faster.

The pipeline refuses to write into a database built with different settings. Pass a new `--database-path` or delete the old database.

## Verify the output

```bash
./scripts/netflow-db.sh verify data/example/netflow.sqlite \
  --dataset-id example \
  --require-data \
  --require-maad-data \
  --require-processed \
  --require-rollup-parity \
  --require-no-raw-ip
```

Success prints an `OK` line. Drop `--require-maad-data` for a `--no-maad` database.

## Select flows

A filtered database is a separate product, so it needs its own output path:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
  --database-path data/example-internal/netflow.sqlite \
  --ip-prefix 192.0.2.0/24 \
  --src-locality internal
```

Conditions combine with AND. `--ip-prefix` matches either endpoint. The locality flags need [locality rules](datasets.md#classify-internal-and-external-endpoints).

`--daily-active-sources` keeps only traffic from sources that were active over each whole local day. It needs `--ip-prefix` with an IPv4 `/16` and locality rules, and it cannot be combined with the locality flags. The [pipeline contract](../code/pipeline-contract.md#flow-selection) defines "active".

To keep a selection permanent, put it in the dataset's `selection` field with its own `db_path` (see [coordinated subsets](datasets.md#define-coordinated-subsets)).

## Build several subsets in one pass

Repeat `--dataset` to build several `daily_active_sources` subsets of the same capture tree while reading the captures once:

```bash
./scripts/netflow-db.sh pipeline \
  --dataset dataset-a \
  --dataset dataset-b \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD>
```

Each entry needs its own `selection` and `db_path` in `datasets.json`, and all entries must share the same `root_path` and sources. Do not add `--database-path`, `--config`, or selection flags.

## Build from CSV or a config file

CSV input, and any mix of CSV and nfcapd input, uses a pipeline configuration file:

```bash
./scripts/netflow-db.sh pipeline \
  --config /path/to/pipeline.json \
  --database-path data/example/netflow.sqlite
```

```json
{
  "locality": [{ "type": "prefixes", "prefixes": ["192.0.2.0/24"] }],
  "selection": { "ip_prefix": "192.0.2.0/24" },
  "inputs": [
    {
      "input_kind": "nfcapd_tree",
      "root_path": "/path/to/captures",
      "source_ids": ["router-a"],
      "start_date": "<YYYY-MM-DD>",
      "end_date": "<YYYY-MM-DD>"
    }
  ]
}
```

Input kinds are `csv`, `nfcapd`, `csv_tree`, and `nfcapd_tree`. Relative paths resolve from the configuration file's directory. For native nfcapd input, set `"nfdump": "target/nfdump/libexec/nfdump"` or pass `--nfdump`.

## Run a long range on several hosts

`scripts/netflow-db-cluster.sh` splits a date range across hosts over plain SSH, builds each piece, copies the results back, merges them, and verifies the merged database.

Each host needs:

- SSH key access from your machine.
- The captures at the same absolute path.
- Local disk for its share of the output.

Run from the machine that should hold the result:

```bash
./scripts/netflow-db-cluster.sh \
  --hosts host-a:2,host-b,host-c \
  --dataset example \
  --start-date <YYYY-MM-DD> \
  --end-date <YYYY-MM-DD> \
  --output data/example/netflow.sqlite \
  --deploy-bin target/release/netflow-db \
  --deploy-nfdump target/nfdump/libexec/nfdump \
  --deploy-datasets /path/to/datasets.json
```

- `host:2` runs two shards on that host at once.
- The `--deploy-*` options copy the binary, nfdump, and registry to `$HOME/atlantis-cluster` on every host. Change it with `--remote-dir`. It must be the same path on every host.
- Pipeline flags after `--` apply to every shard.
- Success ends with the `verify` output for the merged database.

If a shard fails, the merge fails, or you press Ctrl-C, run the same command again. It resumes where it stopped, and shards on the hosts keep running after Ctrl-C.

Common problems:

- A rerun with different `--hosts`, dates, or dataset is refused. Delete `<output>.shards/layout` to start over.
- Do not redeploy nfdump while shards are running. The shards stop, and a different nfdump build is incompatible with the finished shards.

When you are finished, delete `work/` under the remote directory on each host.

To merge shards by hand, build each one with identical flags and an explicit `--end-date` over its own day range, then run:

```bash
./scripts/netflow-db.sh merge-shards --output data/example/netflow.sqlite shard-*.sqlite
```

The output must not exist. Add `--consume` to delete each shard as it is merged when disk is tight. If a consuming merge fails, the error prints the command that resumes it.
