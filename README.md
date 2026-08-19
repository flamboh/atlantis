# ATLANTIS

Network telemetry visualization platform for analyzing large-scale NetFlow data. SvelteKit frontend, SQLite backend, Rust ingestion pipeline.

ATLANTIS reads nfcapd captures from a NetFlow collector. The pipeline converts the captures into a SQLite database, and the dashboard visualizes that database.

## Quick start

First, install the [required tools](docs/user/requirements.md) — both Bun and Node.js are necessary, and nfcapd processing needs the nfdump build tools. You also need nfcapd capture files on disk.

1. Clone the project and build the tools.

   ```bash
   git clone https://github.com/flamboh/atlantis.git
   cd atlantis
   bun install

   # A system nfdump installation does not work.
   git submodule update --init --recursive
   ./vendor/scripts/compile-nfdump.sh
   ```

2. Describe your captures. Copy the templates, then edit them:

   ```bash
   cp .env.example .env
   cp datasets.json.example datasets.json
   ```

   In `datasets.json`, set `root_path` to your capture directory and name one source for each collector directory under it ([dataset configuration](docs/user/datasets.md) explains the fields and the expected layout). In `.env`, set `DEFAULT_DATASET` to your `dataset_id`.

3. Build a database from one day of captures. Use your `dataset_id` and a date that has captures. The first run compiles the Rust pipeline and takes several minutes; run it in a persistent shell like tmux.

   ```bash
   ./scripts/netflow-db.sh pipeline \
     --dataset example \
     --start-date 2025-02-01 \
     --end-date 2025-02-01 \
     --nfdump target/nfdump/libexec/nfdump
   ```

4. Start the dashboard, then open `http://localhost:5173`.

   ```bash
   bun run dev:web
   ```

If a step fails, read [Troubleshooting](docs/user/troubleshooting.md).

## Documentation

- [Install and use ATLANTIS](docs/user/README.md)
- [Develop ATLANTIS](docs/code/README.md)

For a new setup, use these procedures:

- [Set up the web applications](docs/user/setup-web.md)
- [Configure the datasets](docs/user/datasets.md)
- [Set up the data pipeline](docs/user/setup-pipeline.md)

## Acknowledgment

Developed by Oliver Boorstein under support by NSF Research Experiences for Undergraduates with the Oregon Networking Research Group.

Advised by Chris Misa and Reza Rejaie.
