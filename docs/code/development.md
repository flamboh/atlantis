# Development

This procedure prepares a checkout for code changes.

## Install the project

1. Install the required tools from the [web setup](../user/setup-web.md#required-tools).

2. Install the JavaScript dependencies.

   ```bash
   bun install --frozen-lockfile
   ```

3. For native ingestion work, build the pinned nfdump fork.

   ```bash
   git submodule update --init --recursive
   ./vendor/scripts/compile-nfdump.sh
   ```

Cargo builds the pipeline itself. `scripts/netflow-db.sh` and the Rust checks build it on demand.

## Start development servers

Start only the dashboard:

```bash
bun run dev:web
```

Start only the landing site:

```bash
bun run dev:landing
```

Start both applications:

```bash
bun run dev
```

The root `dev` command starts both applications. It does not start the pipeline.

## Run required checks

Run these commands before you complete a change:

```bash
bun run format
bun run lint
bun run typecheck
```

The root commands check the dashboard and the Rust workspace. `format` runs Prettier and `cargo fmt`. `lint` runs ESLint and Clippy. `typecheck` runs `svelte-check` and `cargo check`.

Run the landing checks separately:

```bash
bun run --cwd apps/landing lint
bun run --cwd apps/landing format:check
bun run build:landing
```

Run focused tests for the changed code:

```bash
bun run test:web
bun run test:db
```

Run the Playwright suite when a browser flow changes:

```bash
bun run test:e2e
```

Always use `bun run test`. Do not use `bun test` in this repository.

## Build applications

```bash
bun run build:web
bun run build:landing
```

The root `build` command builds only the landing site. Use the explicit commands for complete verification.

## Change the D1 schema

The Drizzle schema is in `apps/web/src/lib/server/db/schema.ts`.

1. Change the schema and the compatible local pipeline schema.

2. Generate a migration.

   ```bash
   bun run --cwd apps/web db:generate
   ```

3. Review the generated SQL in `apps/web/drizzle`.

4. Apply the migration to a local D1 database.

   ```bash
   bun run --cwd apps/web d1:migrations:apply:local
   ```

5. Run the schema and route tests.

Before shared use, you can replace an unapplied greenfield baseline. After shared use, always add a new migration.

Do not apply the observation-metrics baseline to a database from the prior baseline. Create a new database for this product.

## Rust checks

Run the full pipeline tests:

```bash
bun run test:db
```

This runs `cargo test --workspace --all-features --locked`. The root `lint` and `format:check` commands run the other Rust checks:

```bash
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all --check
```

rustup installs the pinned toolchain from `rust-toolchain.toml` automatically.

For the MAAD comparison against the pinned Haskell oracle, read [MAAD conformance](maad-conformance.md).
