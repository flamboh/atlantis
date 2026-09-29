# ATLANTIS

ATLANTIS turns NetFlow (nfcapd) captures into a SQLite database with a Rust pipeline and visualizes it in a SvelteKit dashboard.

## Quick start

You need Git, Bun, Node.js, and Docker ([requirements](docs/user/requirements.md)), plus nfcapd captures on disk.

1. Install the project.

   ```bash
   git clone https://github.com/flamboh/atlantis.git
   cd atlantis
   bun install
   cp .env.example .env
   cp datasets.json.example datasets.json
   ```

2. In `datasets.json`, set `root_path` to your capture directory and list one source per collector directory ([dataset configuration](docs/user/datasets.md)). In `.env`, set `DEFAULT_DATASET` to your `dataset_id`.

3. Build a database from one day of captures. The first run builds the pipeline image and takes several minutes.

   ```bash
   ./scripts/netflow-db-docker.sh \
     --capture-root /path/to/captures \
     pipeline \
     --dataset example \
     --start-date <YYYY-MM-DD> \
     --end-date <YYYY-MM-DD>
   ```

4. Start the dashboard and open `http://localhost:5173`.

   ```bash
   bun run dev:web
   ```

If a step fails, read [Troubleshooting](docs/user/troubleshooting.md).

## Documentation

- [Install and use ATLANTIS](docs/user/README.md)
- [Develop ATLANTIS](docs/code/README.md)
- [Domain context and invariants](CONTEXT.md)

## Acknowledgment

Developed by Oliver Boorstein under support by NSF Research Experiences for Undergraduates with the Oregon Networking Research Group.

Advised by Chris Misa and Reza Rejaie.
