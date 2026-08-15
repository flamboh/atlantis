# Setup

## Prerequisites

- Bun 1.2+
- Rust 1.97.1 through `rustup` (the repository pins the toolchain)
- `nfdump` on `PATH` for native capture ingestion
- SSH access to the research host when using live ONRG data

`nix-shell` supplies Bun, rustup, nfdump, and Playwright's browser dependencies.
The reducer and MAAD implementation are part of the Rust crate; no C++ compiler
or separately built helper is required.

## Install

```bash
git clone https://github.com/flamboh/atlantis.git
cd atlantis
bun install
cargo build --locked --release --package atlantis-netflow-db
```

The build uses [rust-toolchain.toml](../rust-toolchain.toml), so `rustup`
installs the exact toolchain automatically. `scripts/netflow-db.sh` uses the
release binary when present and otherwise builds it through Cargo.

## Configure datasets

```bash
cp datasets.json.example datasets.json
cp .env.example .env
```

Set each dataset's `root_path` and `db_path`; see
[datasets-json.md](datasets-json.md). `DATASETS_CONFIG_PATH` can point the CLI
and web application at another registry.

## Populate the database

```bash
./scripts/netflow-db.sh pipeline \
  --dataset uoregon \
  --start-date 2025-02-11
```

This discovers native captures, streams `nfdump`'s fixed CSV contract, computes
MAAD in process, and publishes canonical SQLite rows. See
[pipeline-usage.md](pipeline-usage.md) for bounded runs, CSV mapping, exports,
and verification.

## Run the app

```bash
bun run dev
```

The web app starts at http://localhost:5173.

## Checks

```bash
bun format
bun lint
bun typecheck
bun run test
```

## D1 migrations

The web app's D1 schema is tracked in `apps/web/drizzle`:

```bash
bun run --cwd apps/web db:generate
bun run --cwd apps/web d1:migrations:list
bun run --cwd apps/web d1:migrations:apply:local
```

The observation-metrics schema requires a fresh database product rather than an
in-place migration from an older pipeline schema.
