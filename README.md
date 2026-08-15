# ATLANTIS

Network telemetry visualization platform for analyzing large-scale NetFlow data. SvelteKit frontend, SQLite backend, Rust ingestion pipeline.

## Quick Start

```bash
bun install
cp .env.example .env
cp datasets.json.example datasets.json   # configure your dataset paths
./scripts/netflow-db.sh pipeline --dataset uoregon --start-date 2025-02-11
bun run dev                              # start the web app
```

## Documentation

- [Setup](docs/setup.md) — prerequisites, configuration, running the app
- [Querying](docs/querying.md) — direct database access and example queries
- [Project Structure](docs/structure.md) — packages, stack, and dev commands
- [Pipeline Usage](docs/pipeline-usage.md) — ingestion pipeline flags and scheduling
- [datasets.json](docs/datasets-json.md) — dataset configuration reference

# Acknowledgement

Developed by Oliver Boorstein under support by NSF Research Experiences for Undergraduates with the Oregon Networking Research Group.

Advised by Chris Misa and Reza Rejaie.
