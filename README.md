# ATLANTIS

Network telemetry visualization platform for analyzing large-scale NetFlow data. SvelteKit frontend, SQLite backend, Rust ingestion pipeline.

ATLANTIS requires NetFlow data. The pipeline converts this data into the database that the dashboard reads.

## Quick start

```bash
bun install
cp .env.example .env
cp datasets.json.example datasets.json   # configure your dataset paths
./scripts/netflow-db.sh pipeline --dataset uoregon --start-date 2025-02-11
bun run dev                              # start the web app
```

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
