# Project Structure

## Packages

| Path               | Role                                                 |
| ------------------ | ---------------------------------------------------- |
| `apps/web`         | SvelteKit visualization dashboard                    |
| `apps/web/drizzle` | Canonical D1 migration files for the web app         |
| `apps/landing`     | Marketing/SEO landing page                           |
| `tools/netflow-db` | Python ingestion pipeline + DB schema                |
| `vendor/*`         | Optional analyzer submodules and local build outputs |
| `scripts/`         | One-off migration scripts                            |
| `docs/`            | Project documentation                                |
| `plans/`           | Generated implementation plans                       |
| `data/`            | SQLite databases (gitignored)                        |

## Stack

- **Frontend**: SvelteKit 2, TypeScript, TailwindCSS 4, Chart.js
- **Database**: SQLite via `better-sqlite3`
- **Pipeline**: Python 3, `nfdump`
- **Runtime**: Bun 1.2+

## Dev Commands

```bash
bun run dev          # start web app
bun run build:web    # production build
bun run lint         # ESLint
bun run typecheck    # TypeScript check
bun run format       # Prettier
bun run test:web     # Vitest (frontend unit tests)
bun run test:db      # pytest (pipeline tests)
bun run test:e2e     # Playwright
```

Vendor compile helpers:

```bash
./vendor/scripts/compile-burstify.sh
./vendor/scripts/compile-maad.sh
./vendor/scripts/compile-all.sh
```

Python sanity check after editing pipeline code:

```bash
python -m compileall tools/netflow-db
```
