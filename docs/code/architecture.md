# Architecture

ATLANTIS has two web applications and one data pipeline.

## Data flow

```text
NetFlow or CSV input
        |
        v
Rust pipeline ----> SQLite database ---> Local SvelteKit dashboard
                           |
                           +-- external data-load process ---> D1 ---> Deployed dashboard

Astro source ---> Static landing site
```

The repository does not contain the general SQLite-to-D1 data-load process.

## Packages

| Path               | Responsibility                                     |
| ------------------ | -------------------------------------------------- |
| `apps/web`         | Provides the SvelteKit data dashboard.             |
| `apps/landing`     | Provides the Astro marketing site.                 |
| `tools/netflow-db` | Builds and maintains NetFlow databases.            |
| `infra`            | Defines the Alchemy deployment stacks.             |
| `scripts`          | Provides build and local-operation commands.       |
| `vendor`           | Contains optional third-party Git submodules.      |
| `docs/user`        | Contains installation and operation procedures.    |
| `docs/code`        | Contains architecture and development information. |
| `docs/agent`       | Contains generated plans and analysis artifacts.   |

## Web runtime

The local dashboard reads SQLite databases with `better-sqlite3`. It opens these databases in read-only mode.

The deployed dashboard reads the `DB` Cloudflare D1 binding from `cloudflare:workers`. `infra/cloudflare.ts` defines the worker and the D1 database as an Alchemy stack. Alchemy builds the worker with its own SvelteKit adapter and applies the D1 migrations during a deploy.

The campus dashboard runs the SQLite driver on a self-hosted Docker host. `infra/campus.ts` builds `apps/web/Dockerfile` on that host through an SSH Docker context and runs it as a container. The image holds the adapter-node build. The container mounts the host data directory at `/data` and publishes its port on the host loopback only. Users reach it through SSH port forwarding. The image tag is a content hash of the build inputs, so a changed input replaces the container and an unchanged deploy leaves it running.

The data mount is writable because the pipeline publishes WAL-mode databases. SQLite can read a WAL database only if it can create the `-shm` and `-wal` files next to it, even for a read-only connection. The dashboard still opens every database read-only with `query_only`, so it never writes data.

The container names its image as a plain `<name>:<hash>` string. Alchemy compares a container's properties at plan time only when every property is resolved, and an output of an image that is being rebuilt stays unresolved until apply, so the plan would record an in-place update that never swaps the image. The container instead binds to the image's `imageId` output. The binding orders the container after the image on create and before it on destroy, and it is not one of the properties that the container compares. The plan also builds the new image while it compares the image, so the tagged image exists before apply replaces the container.

The image pins Node.js 24.18.1. Node.js 24.19.0 and later abort the process when the garbage collector frees a `better-sqlite3` statement ([nodejs/node#65446](https://github.com/nodejs/node/issues/65446)).

The build selects one database driver. `apps/web/src/lib/server/db/d1.ts` reads D1, and `apps/web/src/lib/server/db/sqlite.ts` reads the pipeline SQLite files. Server code imports the driver as `#db`. [Development](development.md#choose-the-database-driver) explains the selection.

The landing site has no database. It builds static files in `apps/landing/dist`.

## Database ownership

The Rust pipeline in `tools/netflow-db` owns the pipeline-product semantics. It writes the canonical data tables and dataset metadata.

Native nfcapd ingestion uses the pinned nfdump fork in `vendor/nfdump`. The fork streams a private binary contract that the pipeline decodes directly.

The web application owns the D1 schema definition. The Drizzle files in `apps/web/drizzle` contain the D1 migrations. Alchemy records applied migrations in the `__alchemy_migrations` table of each D1 database.

Both implementations must keep compatible table and column contracts. No automated test compares the two schema definitions. Check both sides when you change one.

## Main technology

- SvelteKit 3, Svelte 5, and Vite 8
- Astro 6
- TypeScript
- Tailwind CSS 4
- Chart.js
- Rust 1.97.1
- SQLite and Cloudflare D1
- Bun 1.2.16
