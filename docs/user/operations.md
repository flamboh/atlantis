# Operations

Use these procedures to verify and publish a database. This document also gives the Cloudflare and self-hosted deployment commands.

The D1 and Cloudflare deployment sections apply to the hosted ATLANTIS deployment and need Cloudflare access. A local installation does not use them.

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

## Deploy the self-hosted dashboard

This runs the dashboard as a Docker container on a machine you control. Users reach it through an SSH tunnel; it is not exposed to the network.

### Prerequisites

On the host:

- Docker Engine, usable by your account without `sudo` (for example through the `docker` group).
- A data directory with `<dataset-id>/netflow.sqlite` for each dataset.

On your machine:

- The repository with `bun install` done.
- Docker's command-line client. A local Docker daemon is not needed; the image builds on the host.
- SSH key access to the host: `ssh <host> docker info` must succeed without a password prompt. Host aliases from `~/.ssh/config` work.
- `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, as for the [Cloudflare stack](#set-up-cloudflare-access). Alchemy keeps the deploy state in Cloudflare; no Cloudflare resources are created.

### Configure

Export these variables:

| Variable                           | Required | Meaning                                                                                                                                                |
| ---------------------------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ATLANTIS_SELF_HOSTED_DOCKER_HOST` | yes      | The host as an SSH URL, for example `ssh://user@host.example.com`, `ssh://user@host.example.com:2222`, or an `~/.ssh/config` alias as `ssh://my-host`. |
| `ATLANTIS_SELF_HOSTED_DATA_DIR`    | yes      | Absolute path of the data directory on the host.                                                                                                       |
| `ATLANTIS_SELF_HOSTED_PORT`        | no       | Port on the host's loopback interface. Default `8080`. `ssh <host> ss -ltn` lists ports already in use.                                                |
| `ATLANTIS_SELF_HOSTED_DATA_USER`   | no       | `<uid>:<gid>` the container runs as. Default `1000:1000`. Use the owner of the data files (`id -u` and `id -g` on the host).                           |

The container must run as the data owner because SQLite creates `-shm` and `-wal` files next to each database even when it only reads it.

### Deploy

```bash
bun run deploy:self-hosted --stage <stage>
```

The deploy shows its plan and asks for approval; add `--yes` to skip the prompt. It prints the tunnel command for users. `bun run plan:self-hosted --stage <stage>` previews the change; it builds the image on the host but leaves the container alone.

Deploy again after pulling new code. An unchanged checkout leaves the running container alone.

Check that the container is healthy:

```bash
ssh <host> docker ps --filter name=atlantis-self-hosted-web
```

### Connect

Users need an SSH account on the host. Run the tunnel command that the deploy printed, for example:

```bash
ssh -N -L 8080:127.0.0.1:8080 user@host.example.com
```

Then open `http://localhost:8080`. To use a different local port, change the first number and open that port instead. The tunnel stays open until you stop `ssh`.

### Tear down

```bash
bun run destroy:self-hosted --stage <stage>
```

This removes the container, its current image, and the Docker context. The data directory is untouched. Images from earlier deploys remain; remove them on the host with `docker image rm atlantis-self-hosted-web-<stage>:<tag>`.

### Add or replace datasets

Copy each database to `<data-dir>/<dataset-id>/netflow.sqlite` on the host. The dashboard picks up new and replaced databases on the next request, without a restart.

Copy only a database that no process is writing. To replace a live database, use `sqlite-maintenance`; see [Publish a local SQLite database](#publish-a-local-sqlite-database).

### Run the pipeline on the host

Use the [Docker wrapper](setup-pipeline.md#docker-setup) from a checkout on the host. It writes to the checkout's `data/` directory, so point `ATLANTIS_SELF_HOSTED_DATA_DIR` there:

```bash
ssh <host>
git clone https://github.com/flamboh/atlantis.git atlantis
cd atlantis
./scripts/netflow-db-docker.sh --capture-root /data/netflow/example pipeline ...
```

See [Set up the data pipeline](setup-pipeline.md) for `datasets.json` and the pipeline commands.

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
