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
| `scripts`          | Provides build and local-operation commands.       |
| `vendor`           | Contains optional third-party Git submodules.      |
| `docs/user`        | Contains installation and operation procedures.    |
| `docs/code`        | Contains architecture and development information. |
| `docs/agent`       | Contains generated plans and analysis artifacts.   |

## Web runtime

The local dashboard reads SQLite databases with `better-sqlite3`. It opens these databases in read-only mode.

The deployed dashboard reads the `DB` Cloudflare D1 binding from `cloudflare:workers`.

The build selects one database driver. `apps/web/src/lib/server/db/d1.ts` reads D1, and `apps/web/src/lib/server/db/sqlite.ts` reads the pipeline SQLite files. Server code imports the driver as `#db`. [Development](development.md#choose-the-database-driver) explains the selection.

The landing site has no database. It builds static files in `apps/landing/dist`.

## Database ownership

The Rust pipeline in `tools/netflow-db` owns the pipeline-product semantics. It writes the canonical data tables and dataset metadata.

Native nfcapd ingestion uses the pinned nfdump fork in `vendor/nfdump`. The fork streams a private binary contract that the pipeline decodes directly.

The web application owns the D1 schema definition. The Drizzle files in `apps/web/drizzle` contain the D1 migrations.

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
