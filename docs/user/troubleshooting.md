# Troubleshooting

This document lists the common first-setup failures.

## `bun install` warns `'better-sqlite3' is not yet supported in Bun`

Node.js was not on `PATH` during `bun install`. Without Node.js, Bun runs the `better-sqlite3` install script itself, cannot download the prebuilt binary, and tries a source compilation that usually fails. The dashboard then cannot open a database.

Install Node.js 22 (see the [required tools](setup-web.md#required-tools)), then reinstall:

```bash
rm -rf node_modules
bun install
```

With Node.js on `PATH`, the warning does not appear and no compiler is necessary.

## A pipeline command shows no output for minutes

The first `./scripts/netflow-db.sh` run compiles the Rust pipeline in release mode. The compilation takes several minutes and happens one time. Later runs start immediately.

## `unable to start nfdump executable "nfdump"`

The pipeline did not find an nfdump executable. This error can appear after several minutes, because the pipeline compiles and discovers input files first.

Build the pinned fork, then pass its path:

```bash
git submodule update --init --recursive
./vendor/scripts/compile-nfdump.sh
./scripts/netflow-db.sh pipeline ... --nfdump target/nfdump/libexec/nfdump
```

## nfdump starts but the pipeline rejects its output

The command used a system nfdump installation. A system nfdump does not have the output mode that the pipeline needs. Pass the pinned fork with `--nfdump target/nfdump/libexec/nfdump`.

## The pipeline stops on a missing member directory

Each member in the dataset `sources` must be a directory directly under `root_path`. Check these items in `datasets.json`:

- `root_path` is a real path on this computer, not the `/path/to/...` placeholder from the example file.
- Each `members` name matches a directory name under `root_path` exactly.

The [input directory layout](datasets.md#input-directory-layout) shows the expected structure.

## The dashboard shows "No datasets found"

The dashboard did not discover a database at `data/<dataset-id>/netflow.sqlite`.

- Run the [pipeline](setup-pipeline.md) to create the database.
- If the dataset `db_path` is outside `data/`, set `LOCAL_SQLITE_PATH` in `.env` to the database path.

## The dashboard shows "Failed to list datasets"

The server did not read your SQLite data. The usual cause is a missing `.env` file: without `ATLANTIS_DB_DRIVER=sqlite`, the local server tries to read a Cloudflare D1 database.

```bash
cp .env.example .env
```

Then restart `bun run dev:web`.

## The charts are empty

The dashboard opened a date range without processed data. Check these items:

- The pipeline date range covers the dates that you look at. The pipeline only processes the days between `--start-date` and `--end-date`.
- The date range in the dashboard is inside the processed range. A new visit opens at the earliest processed day, but a saved web address keeps its own dates.
- If the dataset sets `default_start_date` in `datasets.json`, that date is inside the processed range. Remove the field to let the pipeline use the earliest processed day.

Run the [verify command](setup-pipeline.md#verify-the-output) with `--require-data` to confirm that the database contains results.
