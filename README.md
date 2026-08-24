# ATLANTIS

ATLANTIS is a Network telemetry visualization platform for analyzing large-scale NetFlow data. SvelteKit frontend, SQLite backend, Rust ingestion pipeline.

This is done by reading nfcapd captures from a NetFlow collector. The pipeline converts the captures into a SQLite database, and the dashboard visualizes that database.

## Quick start

First, install the [required tools](docs/user/requirements.md) — Bun and Node.js run the dashboard, and Docker runs the pipeline. You also need nfcapd capture files on disk.

1. Clone the project and install the dashboard dependencies.

   ```bash
   git clone https://github.com/flamboh/atlantis.git
   cd atlantis
   bun install
   ```

2. Describe your capture dataset. Copy the templates, then edit them:

   ```bash
   cp .env.example .env
   cp datasets.json.example datasets.json
   ```

   In `datasets.json`, set `root_path` to your capture directory and name one source for each collector directory under it ([dataset configuration](docs/user/datasets.md) explains the fields and the expected layout). In `.env`, set `DEFAULT_DATASET` to your `dataset_id`.

3. Build a database from one day of captures. Use your `dataset_id`, the same absolute path you set as `root_path`, and a date that has captures. The first run builds the pipeline image and takes several minutes; run it in a persistent shell like tmux.

   ```bash
   ./scripts/netflow-db-docker.sh \
     --capture-root /absolute/path/to/captures \
     pipeline \
     --dataset example \
     --start-date <YYYY-MM-DD> \
     --end-date <YYYY-MM-DD>
   ```

   To build the pipeline toolchain yourself instead of using Docker, follow the [native setup](docs/user/setup-pipeline.md#native-setup).

4. Start the dashboard, then open `http://localhost:5173`.

   ```bash
   bun run dev:web
   ```

If a step fails, read [Troubleshooting](docs/user/troubleshooting.md).

## Documentation

- [Install and use ATLANTIS](docs/user/README.md)
- [Develop ATLANTIS](docs/code/README.md)
- [Domain context and invariants](CONTEXT.md)

For a new setup, use these procedures:

- [Set up the web applications](docs/user/setup-web.md)
- [Configure the datasets](docs/user/datasets.md)
- [Set up the data pipeline](docs/user/setup-pipeline.md)

## Acknowledgment

Developed by Oliver Boorstein under support by NSF Research Experiences for Undergraduates with the Oregon Networking Research Group.

Advised by Chris Misa and Reza Rejaie.
