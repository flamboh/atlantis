# Development

This procedure prepares a checkout for code changes.

## Install the project

1. Install all the tools in [Requirements](../user/requirements.md), including the development section.

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

## Choose the database driver

The dashboard has two database drivers. Each build and each development server includes only one of them.

| `ATLANTIS_DB_DRIVER` | Driver                                                                                                           | Default for  |
| -------------------- | ---------------------------------------------------------------------------------------------------------------- | ------------ |
| `sqlite`             | `src/lib/server/db/sqlite.ts` reads `data/<dataset-id>/netflow.sqlite`, `LOCAL_DATA_DIR`, or `LOCAL_SQLITE_PATH` | `vite dev`   |
| `d1`                 | `src/lib/server/db/d1.ts` reads the `DB` binding from `cloudflare:workers`                                       | `vite build` |

Server code imports the driver as `#db`. The `imports` field in `apps/web/package.json` maps `#db` to the D1 driver under the `atlantis-d1` export condition, and to the SQLite driver otherwise. `apps/web/vite.config.ts` adds that condition to the server environments and keeps `cloudflare:workers` external when the driver is `d1`. Both drivers implement `DatabaseDriver` in `src/lib/server/db/driver.ts`.

Set `ATLANTIS_DB_DRIVER` in the shell. The value in `.env` does not select the driver, so a deploy build cannot pick up a local SQLite setting.

```bash
bun run dev:web                             # SQLite
bun run build:web                           # D1 server bundle, no adapter
ATLANTIS_DB_DRIVER=sqlite bun run build:web # Node server in apps/web/build
```

A D1 build has no SvelteKit adapter. `bun run build:web` checks that the D1 bundle compiles. The Cloudflare worker is built by Alchemy during a deploy, which injects its own adapter into the `sveltekit()` call. [Operations](../user/operations.md#deploy-the-dashboard) describes the deploy.

A SQLite build uses `@sveltejs/adapter-node` and writes a Node server to `apps/web/build`. Start it with `node build` from `apps/web`. The campus deployment runs this build in a container. [Operations](../user/operations.md#deploy-the-campus-dashboard) describes it.

The build leaves `paths.origin` unset. SvelteKit 3 replaced adapter-node's runtime `ORIGIN` variable with this build-time option, and a fixed origin would break SSH port forwarding, where each user picks a local port. Adapter-node then builds the request URL from the `Host` header and the `https` protocol, so a request to `http://localhost:8080` has the origin `https://localhost:8080`. The dashboard has no form actions, remote functions, or mutating endpoints, so SvelteKit's CSRF origin check never runs. Before you add a `POST` form, set `PROTOCOL_HEADER` or `paths.origin` so that the origin check sees the browser's real origin.

The D1 driver runs only in a deployed worker. `vite dev` and `vite preview` reject `ATLANTIS_DB_DRIVER=d1`. `alchemy dev` does not help here, because it runs SvelteKit's server code in Node and exposes bindings only on `platform.env`. It does not provide the `cloudflare:workers` module. To test D1 behavior, deploy a personal stage:

```bash
bun run deploy:cloudflare --stage <your-name>
bun run destroy:cloudflare --stage <your-name>
```

## Configure the dashboard

SvelteKit options are in the `sveltekit()` call in `apps/web/vite.config.ts`. The project has no `svelte.config.js`.

The dashboard loads `.env` from the repository root. `apps/web/src/env.ts` declares the runtime variables, and server code imports them from `$app/env/private`. Add a variable to `src/env.ts` before you use it.

Import library modules through `#lib` with the file extension, for example `#lib/utils.ts` or `#lib/components/charts/ChartCard.svelte`.

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

The suite builds a SQLite bundle, serves it with `vite preview`, and runs against a seeded fixture database.

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

3. Review the generated `migration.sql` in the new `apps/web/drizzle/<timestamp>_<name>/` directory.

4. Run the schema and route tests. `tests/lib/server/migrations.test.ts` applies every migration to an empty SQLite database.

5. Deploy a personal stage to apply the migration to a D1 database. The next production deploy applies it to production.

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
