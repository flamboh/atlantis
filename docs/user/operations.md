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
  --start <YYYY-MM-DD> \
  --end <YYYY-MM-DD>
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

## Deploy the dashboard

`infra/cloudflare.ts` defines the dashboard deployment as an [Alchemy](https://alchemy.run) stack named `atlantis`. It has two resources:

- A D1 database. Alchemy applies the migrations in `apps/web/drizzle` during each deploy.
- A SvelteKit worker with the database bound as `DB`. Alchemy builds the worker with the D1 driver. It skips the build and the upload when no file in the memo scope has changed.

Each deploy targets one stage. The stage selects the physical names:

| Stage          | Worker             | D1 database           |
| -------------- | ------------------ | --------------------- |
| `prod`         | `atlantis`         | `atlantis-db`         |
| any other name | `atlantis-<stage>` | `atlantis-db-<stage>` |

Underscores in a stage name become hyphens. Destroying the `prod` stage keeps the production worker and database.

### Set up Cloudflare access

Export a Cloudflare API token and the account ID in the shell:

```bash
export CLOUDFLARE_API_TOKEN=<token>
export CLOUDFLARE_ACCOUNT_ID=<account-id>
```

Alchemy stores the stack state in the account's `alchemy-state-store` worker, and keeps its secrets in the account's Secrets Store. The first deploy on an account creates both.

### Preview, deploy, and remove a stage

```bash
bun run plan:cloudflare --stage <stage>
bun run deploy:cloudflare --stage <stage>
bun run destroy:cloudflare --stage <stage>
```

`plan:cloudflare` does not change anything. `deploy:cloudflare` shows the plan and asks for approval. Add `--yes` to skip the prompt. The deploy prints the worker URL.

A personal stage is a full copy of the deployment with an empty database. Use it to check D1 behavior before a production deploy.

### D1 migrations

Deploys apply pending migrations in order. Alchemy records applied migrations in the `__alchemy_migrations` table of the database. The migrations change the schema only. They do not copy pipeline data from SQLite to D1.

The repository does not have a general SQLite-to-D1 load command. Use the approved project data-load process after the schema deploy.

List the applied migrations:

```bash
bunx wrangler d1 execute atlantis-db --remote \
  --command "SELECT name, applied_at FROM __alchemy_migrations ORDER BY id"
```

If a database has a wrangler `d1_migrations` table and no `__alchemy_migrations` table, the first deploy copies its history into `__alchemy_migrations`. After that, Alchemy never writes to `d1_migrations`. Each recorded name must match a local migration, or the deploy stops.

### Deploy production

1. Record a D1 recovery point. See [Record a D1 recovery point](#record-a-d1-recovery-point).

2. Review the production plan.

   ```bash
   bun run plan:cloudflare --stage prod
   ```

   The plan must not replace or delete `atlantis-db` or the `atlantis` worker.

3. Deploy.

   ```bash
   bun run deploy:cloudflare --stage prod
   ```

4. Open the deployed dashboard and check a known dataset.

### First Alchemy deploy of production

Wrangler created the production worker and database. Alchemy adopts them by name:

- The D1 provider has no ownership marker. Alchemy adopts `atlantis-db` silently.
- The `atlantis` worker has no Alchemy tags, so Alchemy treats it as foreign. The stack sets `AdoptPolicy.adopt` for the `prod` stage only, so the deploy takes over the worker in place. The plan shows it as `create`. The takeover happens during the deploy, because the worker settings depend on the database output. `--adopt` is not needed.

A migration fails if the production database still contains tables from an older schema. In that case the deploy stops before it changes the worker. Reset the database contents first:

1. Record a D1 recovery point.
2. Drop every table in `atlantis-db` except the internal `_cf_*` tables.
3. Deploy production. Alchemy applies all migrations to the empty database.
4. Load the pipeline data with the approved data-load process.

To undo the reset, restore the recorded bookmark.

## Record a D1 recovery point

[Cloudflare D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) provides point-in-time recovery for production D1 databases.

Before a production deploy with new migrations, record the current bookmark:

```bash
bunx wrangler d1 time-travel info atlantis-db
```

Keep the bookmark with the release record.

## Restore D1

CAUTION: A Time Travel restore overwrites remote D1 data. Record the current bookmark before you restore an earlier bookmark.

```bash
bunx wrangler d1 time-travel restore atlantis-db --bookmark=<bookmark>
```

The restore command needs Cloudflare access and confirmation. Check the database name and the bookmark before you approve the restore.

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
