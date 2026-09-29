# Troubleshooting

## `bun install` warns `'better-sqlite3' is not yet supported in Bun`

Node.js was not on `PATH` during the install, so the dashboard cannot open databases. Install Node.js 24.18.1 (see [Requirements](requirements.md)), then reinstall:

```bash
rm -rf node_modules
bun install
```

## A pipeline command shows no output for minutes

The first run compiles the pipeline, which takes several minutes. Later runs start immediately.

## `unable to start nfdump executable "nfdump"`

The native pipeline needs the pinned nfdump fork passed explicitly:

```bash
git submodule update --init --recursive
./vendor/scripts/compile-nfdump.sh
./scripts/netflow-db.sh pipeline ... --nfdump target/nfdump/libexec/nfdump
```

A system nfdump starts but its output is rejected. Use the fork.

## The pipeline stops on a missing member directory

In `datasets.json`, check that:

- `root_path` is a real path on this computer, not the placeholder from the example file.
- Each `members` name exactly matches a directory directly under `root_path`.

## The dashboard shows "No datasets found"

No database exists at `data/<dataset-id>/netflow.sqlite`.

- Run the [pipeline](setup-pipeline.md#build-a-database).
- If the database is somewhere else, set `LOCAL_SQLITE_PATH` (one database) or `LOCAL_DATA_DIR` (a directory of `<dataset-id>/netflow.sqlite`) in `.env`.

## The dashboard shows "Failed to list datasets"

A database was found but could not be read. Run the [verify command](setup-pipeline.md#verify-the-output) on it. A database built by an older pipeline version must be rebuilt.

## The charts are empty

The dashboard's date range has no processed data.

- Check that the pipeline's `--start-date` and `--end-date` cover the dates you are viewing.
- A saved URL keeps its own dates. Open the dataset again from the home page.
- If `default_start_date` is set in `datasets.json`, check that it falls inside the processed range, or remove it.
