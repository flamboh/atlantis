# Project Structure

## Packages

| Path               | Role                                         |
| ------------------ | -------------------------------------------- |
| `apps/web`         | SvelteKit visualization dashboard            |
| `apps/web/drizzle` | Canonical D1 migration files for the web app |
| `apps/landing`     | Marketing/SEO landing page                   |
| `tools/netflow-db` | Rust ingestion pipeline + DB schema          |
| `vendor/*`         | Optional third-party analysis submodules     |
| `scripts/`         | Local workflow and build helper scripts      |
| `docs/`            | Project documentation                        |
| `plans/`           | Generated implementation plans               |
| `data/`            | SQLite databases (gitignored)                |

## Stack

- **Frontend**: SvelteKit 2, TypeScript, TailwindCSS 4, Chart.js
- **Database**: SQLite via `better-sqlite3`
- **Pipeline**: Rust 1.97.1, SQLite, `nfdump`
- **Runtime**: Bun 1.2+

## Dev Commands

```bash
bun run dev          # start web app
bun run build:web    # production build
bun run lint         # ESLint
bun run typecheck    # TypeScript check
bun run format       # Prettier
bun run test:web     # Vitest (frontend unit tests)
bun run test:db      # Rust pipeline tests
bun run test:e2e     # Playwright
```

Pipeline checks:

```bash
cargo test --workspace --all-features --locked
```
