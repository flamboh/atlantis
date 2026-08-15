# AGENTS.md

ATLANTIS turns NetFlow captures and CSV imports into queryable aggregate databases, then visualizes them in a web dashboard.

## Repository Map

- `tools/netflow-db`: Python 3.13 ingestion, aggregation, verification, and analysis-window exports. Native `nfcapd` ingestion uses `nfdump` plus the compiled reducer.
- `apps/web`: Svelte 5/SvelteKit 2 dashboard and API routes. It reads local SQLite during development and Cloudflare D1 in deployment.
- `apps/landing`: Astro marketing and SEO site.
- `vendor/*`: Third-party analysis submodules. Treat these as read-only; build repo-local binaries through the scripts in `scripts/`.
- `data/`, `.env`, and `datasets.json`: Machine-local inputs and generated databases. Keep paths and dataset contents out of commits.

## Engineering Contracts

- Treat ingestion, storage, API queries, and charts as one data contract. When a stored field or dimension changes, account for every Python writer and verifier, the local SQLite schema, the Drizzle schema and migrations, TypeScript query code, and focused tests.
- A pipeline database is a product bound to its schema, flow selection, result configuration, and logical source membership. Semantic changes produce a fresh product database; never silently mix incompatible results in an existing one.
- Assume captures and databases are large. Preserve streaming, batching, bounded concurrency, and bounded-memory processing instead of materializing an entire dataset for convenience.
- Preserve half-open time-window and additive-rollup semantics across ingestion and queries. Missing `nfcapd` buckets are explicit zero-filled observations within proven source bounds, not absent data to ignore.
- Keep the system greenfield: replace obsolete paths cleanly. Add compatibility code only when a deployed database or external contract in scope requires it.

## Svelte

Keep derived state derived. Put interaction-driven updates in event handlers or function bindings, use `{@attach}` for DOM or external-library lifecycle, and use `createSubscriber` for external sources. Reserve `$effect` for genuine external side effects, and never mutate Svelte state from an effect.

Reuse the existing chart registries, chart utilities, and shared filter components when extending dashboard behavior. Keep metric and filter semantics out of presentation-only components.

## Verification

- Before completion, run `bun run format`, `bun run lint`, and `bun run typecheck` successfully.
- Run focused tests for the changed surface: `bun run test:web` for dashboard/API behavior, `bun run test:db` for pipeline behavior, and `bun run test:e2e` for user flows that need browser coverage.
- Use `bun run test`, not `bun test`; the latter invokes Bun's test runner instead of the repository's Vitest orchestration.
- When editing the landing site, also run `bun run --cwd apps/landing lint` and `bun run build:landing`; the root lint script only covers `apps/web`.

## Pull Requests

Use the Conventional Commit style for PR titles.

Every PR description must give a reviewer a fast path to approval:

- Name the flows to exercise, required setup or test data, and expected results.
- Call out important edge cases, failure states, and business-logic decisions.
- List automated verification and any remaining manual verification.

Keep this focused on observable behavior and decisions rather than an exhaustive implementation summary.
