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

Scalar values must match exactly. MAAD dimensions and each element of the stored 32-bit float curves compare with an absolute tolerance. The default tolerance is `1e-10` and `--maad-absolute-tolerance` changes it. Builds whose MAAD values differ slightly can round to adjacent 32-bit floats, so use a tolerance above one 32-bit float step of the largest curve value (about `1e-6`) for those comparisons.

A missing reference row or a shared-value mismatch returns a nonzero exit status. So does a q grid that is missing or differs for an IP version whose MAAD curves both databases store in the window; `maad_q_grid.mismatched_ip_versions` in the report lists them.

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

`infra/cloudflare.ts` defines the deployment as an [Alchemy](https://alchemy.run) stack with a D1 database and a SvelteKit worker. Each deploy targets one stage:

| Stage          | Worker             | D1 database           |
| -------------- | ------------------ | --------------------- |
| `prod`         | `atlantis`         | `atlantis-db`         |
| any other name | `atlantis-<stage>` | `atlantis-db-<stage>` |

A stage name uses lowercase letters, digits, and single hyphens, for example `alice-dev`. Other names are rejected. Destroying the `prod` stage keeps the production worker and database.

The examples use Wrangler 4.141.0.

### Set up Cloudflare access

```bash
export CLOUDFLARE_API_TOKEN=<token>
export CLOUDFLARE_ACCOUNT_ID=<account-id>
```

The first Alchemy command on an account, `plan` included, offers to create Alchemy's state store. Later commands can offer to upgrade it.

### Preview, deploy, and remove a stage

```bash
bun run plan:cloudflare --stage <stage>
bun run deploy:cloudflare --stage <stage>
bun run destroy:cloudflare --stage <stage>
```

`plan:cloudflare` does not change the stage's worker or database. It can still create or upgrade the state store. `deploy:cloudflare` asks for approval; add `--yes` to skip the prompt. The deploy prints the worker URL.

A non-production stage starts with an empty database. Use one to check D1 behavior before a production deploy.

### D1 migrations

Deploys apply pending migrations from `apps/web/drizzle`. Migrations change the schema only; they do not load pipeline data.

List the applied migrations:

```bash
bunx wrangler@4.141.0 d1 execute atlantis-db --remote \
  --command "SELECT name, applied_at FROM __alchemy_migrations ORDER BY id"
```

### Deploy production

1. [Record a D1 recovery point](#record-a-d1-recovery-point).
2. Review the plan. It must not replace or delete `atlantis-db` or the `atlantis` worker.

   ```bash
   bun run plan:cloudflare --stage prod
   ```

3. Deploy.

   ```bash
   bun run deploy:cloudflare --stage prod
   ```

4. Open the dashboard and check a known dataset.

### Production cutover (pending)

Production still runs the Wrangler deployment. The first Alchemy deploy adopts the existing `atlantis` worker and `atlantis-db` database; the plan shows the worker as `create`. Planned steps:

1. [Record a D1 recovery point](#record-a-d1-recovery-point).
2. Drop every table in `atlantis-db` except the `_cf_*` tables.
3. Deploy production. All seven migrations in `apps/web/drizzle`, from `military_stature` through `compact_maad_storage`, run on the empty database.
4. Reload the pipeline data from products built with the current schema (locality dimensions and compact MAAD storage). The reload tooling is not decided yet.

To undo, [restore the bookmark](#restore-d1).

## Record a D1 recovery point

[D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) gives point-in-time recovery. Before a production deploy with new migrations, record the bookmark and keep it with the release record:

```bash
bunx wrangler@4.141.0 d1 time-travel info atlantis-db
```

## Restore D1

CAUTION: A restore overwrites remote D1 data. Record the current bookmark first.

```bash
bunx wrangler@4.141.0 d1 time-travel restore atlantis-db --bookmark=<bookmark>
```

## Deploy the landing site

1. Build the site with its public URL.

   ```bash
   PUBLIC_SITE_URL=https://example.com bun run build:landing
   ```

2. Deploy the static assets.

   ```bash
   bunx wrangler@4.141.0 deploy --config apps/landing/wrangler.jsonc
   ```

3. Open the public URL and check the main links.

For schema change rules, read [Development](../code/development.md#change-the-d1-schema).
