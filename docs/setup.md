# Setup

## Prerequisites

- Bun 1.2+
- Python 3.x
- `nfdump` available on your PATH (or via Nix — see `shell.nix`)
- a C++17 compiler for the exact nfcapd reducer
- SSH access to the research host if using live ONRG data

### For MAAD-Backed Address Structure Stats

Only needed if you want MAAD-backed address structure stats:

- a C++ compiler for `tools/netflow-db/maad_fast.cpp`
- or `nix-shell`, then use `scripts/build_maad_fast.sh`

Build the required nfcapd reducer before processing native captures:

```bash
./scripts/build_nfdump_reducer.sh
```

## Install

```bash
git clone https://github.com/flamboh/atlantis.git
cd atlantis
bun install
```

## Configure Datasets

Copy the example config and edit paths for your machine:

```bash
cp datasets.json.example datasets.json
```

`root_path` should point at the directory containing router/source subdirectories.

See [datasets-json.md](datasets-json.md) for more.

## Environment

Start from the template:

```bash
cp .env.example .env
```

At minimum, make sure `DEFAULT_DATASET` matches one of the dataset ids in
`datasets.json`.

Optional overrides: `DATASETS_CONFIG_PATH`, `MAX_WORKERS`, `AGGREGATE_MAAD_MAX_WORKERS`.

## Populate the Database

```bash
python tools/netflow-db/pipeline.py --dataset uoregon --start-date 2025-02-11
```

This discovers and processes NetFlow files into SQLite.
See [pipeline-usage.md](pipeline-usage.md) for flags and scheduling patterns.

## D1 Migrations

The web app's D1 schema is tracked in `apps/web/drizzle`. Generate migrations
from the Drizzle schema:

```bash
bun run --cwd apps/web db:generate
```

Wrangler is configured to read the same directory:

```bash
bun run --cwd apps/web d1:migrations:list
bun run --cwd apps/web d1:migrations:apply:local
```

Current stack policy is greenfield: before a shared D1 database has applied
these files, rebaseline `apps/web/drizzle` when the schema changes. After a D1
database has applied a migration, add a new migration instead of editing the
applied file.

## Optional: Compile MAAD Helper

If you need MAAD-backed address structure stats for real captures, build the
fast MAAD helper:

```bash
git submodule update --init --recursive
./scripts/build_maad_fast.sh
```

This helper is not required for the no-data setup verification flow above.

## Run the App

```bash
bun run dev
```

The web app starts at http://localhost:5173.

### SSH Tunnel (remote host)

```bash
ssh -L 5173:localhost:5173 user@pinot
```

Then open http://localhost:5173 locally.
