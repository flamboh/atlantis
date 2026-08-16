# Setup

## Prerequisites

- Bun 1.2+
- Rust 1.97.1 through `rustup` (the repository pins the toolchain)
- Autotools, flex/bison, a C compiler, `pkg-config`, Python 3, and `tar` for the
  nfdump fork build
- SSH access to the research host when using live ONRG data

`nix-shell` supplies Bun, rustup, the nfdump fork's build toolchain, and
Playwright's browser dependencies. The pinned Atlantis fork must still be
built below.

## Install

```bash
git clone https://github.com/flamboh/atlantis.git
cd atlantis
bun install
git submodule update --init --recursive
./vendor/scripts/compile-nfdump.sh
cargo build --locked --release --package atlantis-netflow-db
```

The `vendor/nfdump` submodule is pinned by this repository and hosted at
[`flamboh/nfdump`](https://github.com/flamboh/nfdump). The fork is based on
upstream nfdump v1.7.6. The build runs its Atlantis wire conformance test, then
stages the private helper at `target/nfdump/libexec/nfdump`; `target/` is
disposable and git-ignored.

To update the fork, rebase its `atlantis-binary-v1` branch onto a reviewed
upstream nfdump tag, resolve the small output-adapter patch there, and run the
fork's serial test suite. Push the updated fork branch, advance this repository's
submodule pointer, then run `./vendor/scripts/compile-nfdump.sh` and the checks
below. Treat a protocol or normalization change as a versioned wire-contract
change and update the Rust decoder and provenance revision in the same change.

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
  --start-date 2025-02-11 \
  --nfdump target/nfdump/libexec/nfdump
```

This discovers native captures, invokes the pinned fork's private
`atlantis-flow-stream-v1` output, decodes it directly into canonical buckets in
Rust, computes MAAD in process, and publishes canonical SQLite rows. See
[pipeline-usage.md](pipeline-usage.md) for bounded runs, input contracts,
exports, and verification.

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
