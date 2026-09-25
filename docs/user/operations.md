# Operations

Use these procedures to verify and publish a database. This document also gives the Cloudflare and campus deployment commands.

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

## Deploy the campus dashboard

`infra/campus.ts` runs the dashboard as a Docker container on a self-hosted machine. It is an [Alchemy](https://alchemy.run) stack named `atlantis-campus`. The container reads the pipeline SQLite databases from a directory on that machine. It is not publicly reachable: it listens on the host loopback only, and users connect through SSH port forwarding.

The stack has three resources:

- A Docker context that reaches the host's Docker daemon over SSH.
- The web image, built from `apps/web/Dockerfile` on the host. The image holds the Node build of the dashboard with the SQLite driver.
- The web container. It mounts the data directory at `/data`, has a 1 GB memory limit, restarts unless stopped, and reports health from `/api/datasets`.

The image tag is a hash of the build inputs: the files that `apps/web/Dockerfile.dockerignore` admits, the Dockerfile, and the build arguments. A deploy after a source change builds a new image and replaces the container. A deploy without changes rebuilds from the Docker cache and leaves the container running.

Each stage names its image and container `atlantis-campus-web-<stage>`. The `prod` stage uses `atlantis-campus-web`.

### Prepare the host

The host needs:

- Docker Engine. The operator's account must be able to run `docker` without `sudo`, for example through the `docker` group.
- SSH key access for the operator. `ssh <host> docker info` must succeed without a password prompt. Docker uses the operator's SSH configuration, so a host alias from `~/.ssh/config` works.
- A data directory that holds `<dataset-id>/netflow.sqlite` for each dataset.

The operator's machine needs Docker's command-line client and the repository dependencies from `bun install`. It does not need a local Docker daemon, because builds run on the host.

### Configure the stack

Set these variables in the shell:

| Variable                      | Required | Meaning                                                                                                                                                             |
| ----------------------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ATLANTIS_CAMPUS_DOCKER_HOST` | yes      | Docker host as an SSH URL, for example `ssh://barbera` or `ssh://user@host.example.edu`.                                                                            |
| `ATLANTIS_CAMPUS_DATA_DIR`    | yes      | Absolute data directory on the host.                                                                                                                                |
| `ATLANTIS_CAMPUS_PORT`        | no       | Host loopback port for the dashboard. The default is `8080`. Pick a free port: `ssh <host> ss -ltn` lists the ports in use.                                         |
| `ATLANTIS_CAMPUS_DATA_USER`   | no       | `<uid>:<gid>` that the container runs as. The default is `1000:1000`. Use the owner of the data files, for example `$(id -u):$(id -g)` of that account on the host. |

Alchemy stores the stack state in Cloudflare, like the [Cloudflare stack](#set-up-cloudflare-access). Export `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` too. The stack creates no Cloudflare resources.

### Deploy and remove a stage

```bash
bun run plan:campus --stage <stage>
bun run deploy:campus --stage <stage>
bun run destroy:campus --stage <stage>
```

`plan:campus` builds the image on the host to compare it with the deployed image. It does not change the container. `deploy:campus` shows the plan and asks for approval. Add `--yes` to skip the prompt. The deploy prints the SSH command that users need.

Check the container on the host:

```bash
ssh <host> docker ps --filter name=atlantis-campus-web
```

The status reads `healthy` after the first health check passes.

`destroy:campus` removes the container, the current image, and the Docker context. It does not remove the data directory or its files. Images from earlier deploys keep their hash tags. Remove them on the host with `docker image rm atlantis-campus-web-<stage>:<tag>`.

### Load data

Copy each dataset to `<data-dir>/<dataset-id>/netflow.sqlite` on the host. The dashboard discovers every dataset directory. It picks up a new or replaced database on the next request, so the container does not need a restart.

Copy only a database that no process is writing. To replace a live database, use `sqlite-maintenance`, which replaces the file atomically. See [Publish a local SQLite database](#publish-a-local-sqlite-database).

The dashboard opens each database read-only. The mount is still writable, because SQLite creates the `netflow.sqlite-shm` and `netflow.sqlite-wal` files next to a WAL-mode database when it reads it. Set `ATLANTIS_CAMPUS_DATA_USER` to the owner of the data files, so that the container can create those files and the pipeline can still write them.

### Run the pipeline on the host

The stack does not build the pipeline image. The pipeline is a batch command, not a service, and its image build compiles nfdump and the Rust toolchain.

Run the pipeline from a checkout of this repository on the host with the [Docker wrapper](setup-pipeline.md#docker-setup). The wrapper writes to the checkout's `data/` directory, so set `ATLANTIS_CAMPUS_DATA_DIR` to that directory:

```bash
ssh <host>
git clone https://gitlab.com/onrg/netflow-analysis.git atlantis
cd atlantis
./scripts/netflow-db-docker.sh --capture-root /absolute/path/to/captures pipeline ...
```

The wrapper builds the `atlantis-netflow-db:local` image on the host when it is missing or out of date. Follow [Set up the data pipeline](setup-pipeline.md) for the `datasets.json` configuration and the pipeline commands.

### Connect as a user

Users need an SSH account on the host. Forward a local port to the dashboard port on the host loopback:

```bash
ssh -N -L 8080:127.0.0.1:<port> <host>
```

Then open `http://localhost:8080`. Replace the first `8080` with any free local port, and open that port instead. The tunnel stays open until you stop `ssh`.

A connection to `<host>:<port>` from another machine fails, because the container publishes its port on `127.0.0.1` only.

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
