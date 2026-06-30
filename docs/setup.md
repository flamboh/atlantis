# Setup

## Prerequisites

- Bun 1.2+
- Python 3.x
- `nfdump` available on your PATH (or via Nix — see `shell.nix`)
- SSH access to the research host if using live ONRG data

### For Multifractal Analysis

Only needed if you want the vendor analyzer binaries:

- `ghc` plus the Haskell packages listed in `vendor/maad/shell.nix` for `vendor/maad`
- `zig` for `vendor/burstify`
- `pkg-config` and `libpcap` if you also want the full burstify toolchain, not just `StructureFunction`
- or `nix-shell`, then use the compile scripts under `vendor/scripts/`

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

Optional overrides: `DATASETS_CONFIG_PATH`, `NETFLOW_DATA_PATH`, `DATABASE_PATH`.

## Populate the Database

```bash
python tools/netflow-db/pipeline.py
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

## Optional: Compile Vendor Analyzers

If you need `spectrum_stats` / `structure_stats` for real captures, initialize
the analyzer submodules and compile the vendor binaries:

```bash
git submodule update --init --recursive
./vendor/scripts/compile-maad.sh
./vendor/scripts/compile-burstify.sh
# or: ./vendor/scripts/compile-all.sh
```

`compile-burstify.sh` builds `vendor/burstify/zig-out/bin/StructureFunction`,
which `structure_stats` uses. If `libpcap` is also available, it additionally
builds the rest of burstify.

`compile-maad.sh` builds `vendor/maad/Spectrum` and related Haskell executables
used by `spectrum_stats`.

Those binaries are not required for the no-data setup verification flow above.

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
