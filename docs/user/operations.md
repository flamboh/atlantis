# Operations

Use these procedures to verify and publish a database. This document also gives the available Cloudflare deployment commands.

The D1 and deployment sections apply to the hosted ATLANTIS deployment and need Cloudflare access. A local installation does not use them.

## Verify a SQLite database

Run all release checks against the candidate database:

```bash
./scripts/netflow-db.sh verify data/candidate/netflow.sqlite \
  --dataset-id example \
  --require-data \
  --require-maad-data \
  --require-processed \
  --require-rollup-parity \
  --require-no-raw-ip
```

Verification runs the same schema checks and representative query shapes that the web application uses.

Do not publish the database if this command fails.

## Compare a candidate with a reference

Compare a rebuilt candidate with a trusted historical database before you publish it:

```bash
./scripts/netflow-db.sh compare \
  data/candidate/netflow.sqlite \
  data/example/historical.sqlite \
  --start 2025-11-01 \
  --end 2025-12-01
```

`--start` and `--end` are half-open local date or time boundaries. The default timezone is `America/Los_Angeles`.

Scalar values must match exactly. MAAD JSON values compare with an absolute tolerance. The default tolerance is `1e-10` and `--maad-absolute-tolerance` changes it.

A missing reference row or a shared-value mismatch returns a nonzero exit status.

## Publish a local SQLite database

The maintenance command creates a checked SQLite backup. Then it atomically replaces the target without stale write-ahead-log sidecar files.

```bash
./scripts/netflow-db.sh sqlite-maintenance \
  data/candidate/netflow.sqlite \
  data/example/netflow.sqlite \
  --backup-existing data/backups/example-before-publish.sqlite
```

Do not copy an active SQLite main file with `cp`. An active database can have write-ahead-log sidecar files.

## Restore a local SQLite database

Use the backup as the next candidate:

```bash
./scripts/netflow-db.sh sqlite-maintenance \
  data/backups/example-before-publish.sqlite \
  data/example/netflow.sqlite \
  --backup-existing data/backups/example-failed-publish.sqlite
```

Run the compatibility check after the restore.

## Manage D1 migrations

The web D1 migrations are in `apps/web/drizzle`.

List the local migration state:

```bash
bun run --cwd apps/web d1:migrations:list
```

Apply migrations to a local D1 database:

```bash
bun run --cwd apps/web d1:migrations:apply:local
```

Apply migrations to the configured remote D1 database:

```bash
bun run --cwd apps/web d1:migrations:apply:remote
```

These commands change the schema. They do not copy pipeline data from SQLite to D1.

The repository does not have a general SQLite-to-D1 load command. Use the approved project data-load process before deployment.

## Record a D1 recovery point

[Cloudflare D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) provides point-in-time recovery for production D1 databases.

Before a remote migration, record the current bookmark:

```bash
bunx wrangler d1 time-travel info atlantis-db \
  --config apps/web/wrangler.jsonc
```

Keep the bookmark with the release record.

## Restore D1

CAUTION: A Time Travel restore overwrites remote D1 data. Record the current bookmark before you restore an earlier bookmark.

```bash
bunx wrangler d1 time-travel restore atlantis-db \
  --bookmark=<bookmark> \
  --config apps/web/wrangler.jsonc
```

The restore command needs Cloudflare access and confirmation. Check the database name and the bookmark before you approve the restore.

## Deploy the dashboard

1. Build and check the worker package.

   ```bash
   bun run build:web
   bun run --cwd apps/web deploy:dry-run
   ```

2. Apply required D1 migrations.

3. Deploy the dashboard.

   ```bash
   bun run --cwd apps/web deploy
   ```

4. Open the deployed dashboard and check a known dataset.

## Deploy the landing site

1. Build the site with its public URL.

   ```bash
   PUBLIC_SITE_URL=https://example.com bun run build:landing
   ```

2. Deploy the static assets.

   ```bash
   bunx wrangler deploy --config apps/landing/wrangler.jsonc
   ```

3. Open the public URL and check the main links.

For schema change rules, read [Development](../code/development.md#change-the-d1-schema).
