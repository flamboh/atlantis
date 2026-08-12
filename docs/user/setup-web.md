# Web setup

These instructions guide installing the project tools.

## Required tools

Install these tools:

| Tool    | Version                   | Source of truth       |
| ------- | ------------------------- | --------------------- |
| Bun     | 1.2.16                    | `package.json`        |
| Node.js | 22.16.0                   | `.node-version`       |
| rustup  | Current stable version    | rustup installation   |
| Rust    | 1.97.1                    | `rust-toolchain.toml` |
| Git     | Current supported version | Git installation      |

rustup reads `rust-toolchain.toml` and installs the pinned Rust toolchain automatically.

The pipeline has more requirements. Install them only when you build a database from native nfcapd input:

- The `vendor/nfdump` Git submodule
- The nfdump fork build tools: autoconf, automake, libtool, flex, bison, make, a C compiler, `pkg-config`, Python 3, and `tar`
- NetFlow or CSV input data

The repository has a `shell.nix` file. It supplies Bun, rustup, the nfdump build tools, and the Playwright browser dependencies.

## Install the dependencies

1. Clone the repository.

   ```bash
   git clone https://github.com/flamboh/atlantis.git
   cd atlantis
   ```

2. Install the JavaScript dependencies.

   ```bash
   bun install --frozen-lockfile
   ```

## Progress check before data processing

These commands do not read your NetFlow data.

```bash
bun run format:check
bun run lint
bun run typecheck
bun run --cwd apps/landing lint
bun run build:web
bun run build:landing
```

The root `format:check`, `lint`, and `typecheck` commands also check the Rust workspace. The first run compiles the Rust dependencies and takes several minutes.

These checks confirm that the source code and dependencies are valid. They should all be green at this point.

## Run the dashboard

The dashboard reads a SQLite database that the pipeline creates from your NetFlow data.

First, [configure a dataset](datasets.md). Then, [set up the data pipeline](setup-pipeline.md) to create the database.

1. If `.env` does not exist, copy the environment template.

   ```bash
   cp .env.example .env
   ```

2. Set these values in `.env`.

   ```dotenv
   ATLANTIS_DB_DRIVER=sqlite
   DEFAULT_DATASET=your-dataset-id
   ```

3. Start the dashboard.

   ```bash
   bun run dev:web
   ```

4. Open `http://localhost:5173`.

The dashboard starts on the dataset from `DEFAULT_DATASET`. When that ID is absent, the dashboard uses the first discovered dataset.

Put each database at `data/<dataset-id>/netflow.sqlite`. They are automatically discovered.

## Run both sites

This command starts the dashboard and the landing site:

```bash
bun run dev
```

The dashboard uses port 5173. The landing site uses port 4321.

## Use a remote development host

Create an SSH tunnel for the dashboard:

```bash
ssh -L 5173:localhost:5173 user@remote-host
```

Then, open `http://localhost:5173` on your local computer.
