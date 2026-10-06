# Operations

Publish databases and deploy the dashboard.

## Publish a database

1. [Verify](setup-pipeline.md#verify-the-output) the new database. Do not publish it if verification fails.

2. Swap it into place. This keeps a backup and handles SQLite's `-wal`/`-shm` files, which a plain `cp` does not:

   ```bash
   ./scripts/netflow-db.sh sqlite-maintenance \
     data/candidate/netflow.sqlite \
     data/example/netflow.sqlite \
     --backup-existing data/backups/example-before-publish.sqlite
   ```

To roll back, run the same command with the backup as the first argument.

### Compare with an older build

Before replacing a trusted database, check that the rebuild agrees with it:

```bash
./scripts/netflow-db.sh compare \
  data/candidate/netflow.sqlite \
  data/example/netflow.sqlite \
  --start <YYYY-MM-DD> \
  --end <YYYY-MM-DD>
```

`--end` is exclusive. Any mismatch exits nonzero. If only MAAD values differ by float rounding between builds, rerun with `--maad-absolute-tolerance 1e-6`.

## Deploy the self-hosted dashboard

This runs the dashboard as a Docker container on a server. It listens only on the server's loopback interface, and users reach it through an SSH tunnel.

### Prerequisites

On the server:

- Docker, usable by your account without `sudo`.
- A data directory with `<dataset-id>/netflow.sqlite` for each dataset.

On your machine:

- The repository with `bun install` done, and the Docker CLI (the image builds on the server, so no local daemon is needed).
- `ssh <server> docker info` works without a password prompt.

Alchemy keeps the deploy state in `.alchemy/` in your checkout. Deploy and tear down from the same checkout, or it loses track of the container.

### Deploy

```bash
export ATLANTIS_SELF_HOSTED_DOCKER_HOST=ssh://user@server.example.com
export ATLANTIS_SELF_HOSTED_DATA_DIR=/path/to/data
bun run deploy:self-hosted --stage prod
```

Optional variables:

- `ATLANTIS_SELF_HOSTED_PORT`: server loopback port, default `8080`.
- `ATLANTIS_SELF_HOSTED_DATA_USER`: `<uid>:<gid>` that owns the data files, default `1000:1000`. Get it with `id -u` and `id -g` on the server.
- `ATLANTIS_SELF_HOSTED_READ_ONLY`: mount the data directory read-only, default `false`. Serve completed products without a live WAL when enabling this.
- `ATLANTIS_SELF_HOSTED_STATE_DIR`: absolute directory for Alchemy state and logs, default the checkout. Reuse it for every plan, deploy, and destroy of the stage.

The isolated `perf` stage defaults to port `8090` and read-only data. It requires an external state directory. See [Dashboard performance](../code/performance.md) for setup and benchmark commands.

The deploy asks for approval (`--yes` skips it) and prints the tunnel command. Deploy again after pulling new code. An unchanged checkout leaves the container alone. `bun run plan:self-hosted --stage prod` previews the change.

Check that the container is healthy:

```bash
ssh <server> docker ps --filter name=atlantis-self-hosted-web
```

Common problems:

- Datasets fail to open: the data files are not owned by `ATLANTIS_SELF_HOSTED_DATA_USER`. SQLite must create `-wal` and `-shm` files next to each database, even to read it.
- Port `8080` is taken on the server: set `ATLANTIS_SELF_HOSTED_PORT` to a free port. `ssh <server> ss -ltn` lists the ports in use.

### Connect

Users need an SSH account on the server. Run the printed tunnel command, for example:

```bash
ssh -N -L 8080:127.0.0.1:8080 user@server.example.com
```

Then open `http://localhost:8080`.

### Add or replace datasets

Copy each database to `<data-dir>/<dataset-id>/netflow.sqlite`. The dashboard picks it up on the next request, without a restart. To replace a database in use, [publish it with `sqlite-maintenance`](#publish-a-database) instead of copying over it.

To build databases on the server, clone the repository there, run the [Docker pipeline](setup-pipeline.md#docker-setup), and point `ATLANTIS_SELF_HOSTED_DATA_DIR` at the checkout's `data/` directory.

### Tear down

```bash
bun run destroy:self-hosted --stage prod
```

This removes the container, its current image, and the Docker context. The data directory is untouched.

## Deploy to Cloudflare

`infra/cloudflare.ts` deploys a worker and a D1 database. It needs Cloudflare access:

```bash
export CLOUDFLARE_API_TOKEN=<token>
export CLOUDFLARE_ACCOUNT_ID=<account-id>
```

| Stage       | Worker             | D1 database           |
| ----------- | ------------------ | --------------------- |
| `prod`      | `atlantis`         | `atlantis-db`         |
| other names | `atlantis-<stage>` | `atlantis-db-<stage>` |

Stage names use lowercase letters, digits, and single hyphens, for example `alice-dev`.

```bash
bun run plan:cloudflare --stage <stage>
bun run deploy:cloudflare --stage <stage>
bun run destroy:cloudflare --stage <stage>
```

The deploy applies pending migrations from `apps/web/drizzle` and prints the worker URL. Migrations change the schema only and do not load data. A new stage starts with an empty database. Destroying `prod` keeps its worker and database.

### Deploy production

1. Record a D1 recovery point and keep the bookmark:

   ```bash
   bunx wrangler@4.141.0 d1 time-travel info atlantis-db
   ```

2. Run `bun run plan:cloudflare --stage prod`. It must not replace or delete `atlantis-db` or the `atlantis` worker.
3. Run `bun run deploy:cloudflare --stage prod`.
4. Open the dashboard and check a known dataset.

To roll back the data, restore the bookmark. This overwrites the remote database:

```bash
bunx wrangler@4.141.0 d1 time-travel restore atlantis-db --bookmark=<bookmark>
```

### Production cutover (pending)

Production still runs the older Wrangler deployment. The first Alchemy deploy adopts the existing `atlantis` worker and `atlantis-db` database (the plan shows the worker as `create`). The planned steps:

1. Record a D1 recovery point.
2. Drop every table in `atlantis-db` except the `_cf_*` tables.
3. Deploy production, which applies every migration to the empty database.
4. Reload the data from databases built with the current schema. The reload tooling is not decided yet.

## Deploy the landing site

```bash
PUBLIC_SITE_URL=https://example.com bun run build:landing
bunx wrangler@4.141.0 deploy --config apps/landing/wrangler.jsonc
```

Open the public URL and check the main links.
