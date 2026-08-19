# Web setup

These instructions guide installing the project tools and running the dashboard.

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

The pipeline also needs the nfdump fork build tools: autoconf, automake, libtool, flex, bison, make, a C compiler, `pkg-config`, Python 3, and `tar`. [Pipeline setup](setup-pipeline.md) uses them to build the fork.

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

To change the code and run the full project checks, read [Development](../code/development.md).

## Run the dashboard

The dashboard reads a SQLite database that the pipeline creates from your NetFlow data.

First, [configure a dataset](datasets.md). Then, [set up the data pipeline](setup-pipeline.md) to create the database.

1. If `.env` does not exist, copy the environment template.

   ```bash
   cp .env.example .env
   ```

   This step is necessary. Without `ATLANTIS_DB_DRIVER=sqlite` from the template, the local server tries to read a Cloudflare D1 database and fails.

2. In `.env`, set `DEFAULT_DATASET` to your dataset ID. This step is optional. Without it, the dashboard uses the first discovered dataset.

3. Start the dashboard.

   ```bash
   bun run dev:web
   ```

4. Open `http://localhost:5173`.

The dashboard shows one card for each dataset. Select a card to see the charts.

The dashboard automatically discovers each database at `data/<dataset-id>/netflow.sqlite`. Before the pipeline creates a database, the dashboard shows a "No datasets found" message with setup guidance.

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
