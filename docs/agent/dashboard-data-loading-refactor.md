# Dashboard data loading and chart refactor

## Goal

Fix data-loading churn, chart re-render churn, and heavy eager work with incremental cleanup.

## Non-goals

- No global `ssr = false`
- No server-blocking move of heavy dashboard fetches into `+page.server.ts`

## Principles

- Keep SSR on globally
- Use SSR/load for shell + nav-critical + lightweight data only
- Keep heavy interactive/dashboard data progressive on client
- Centralize resource loading; remove per-component fetch orchestration
- Use `$derived` for state derivation; `$effect` only for imperative edges
- Keep stale chart data visible during incremental refetches
- Prefer small vertical slices over big-bang rewrites
- Lazy-load expensive secondary analysis when user intent is clear

## Learnings from PR1

- Small slices work well here; meaningful wins landed without infra churn
- Expensive analysis is a good fit for explicit user-triggered loading
- Chart correctness issues are often config/lifecycle problems, not library problems
- File detail state is already too dense; breakup should happen early, not later
- Existing mutable `Map` + effect-heavy patterns make small UI changes harder than they should be
- Plan should prioritize shared client resources and file-detail breakup sooner

## PR1: Immediate UX fixes

### Scope

- Fix NetFlow line-mode zero handling
- Remove production debug logging from chart click path
- Make singularities click-to-load instead of eager fanout

### Why first

- Smallest risk
- Immediate perf/UX win
- Clears obvious correctness issue

### Acceptance

- NetFlow line mode renders zero-value periods correctly
- File detail page does not start singularities analysis until user asks
- No chart click debug spam in browser console

## PR2: Route/data-loading cleanup

### Scope

- Move lightweight nav-critical data to route `load`
- Keep heavy detail/chart data out of blocking server loads
- Move the file detail page implementation out of `apps/web/src/routes/api/netflow/files/[slug]/+page.svelte`
- Replace the current route reuse hack with a normal page implementation under `apps/web/src/routes/netflow/files/[slug]/`
- Start with:
  - dataset list page
  - file jump page
  - dataset dashboard router bootstrap
  - file detail shell metadata + route structure cleanup
- Prefer `+page.ts` over `+page.server.ts` unless server-only access is required

### Notes

- Verified SvelteKit pattern:
  - SSR enabled by default
  - universal `load` runs on initial SSR, then client on later nav
  - `onMount` is client-only, so should not own primary page data
- The file detail page currently lives under `routes/api/` even though it is a UI page; fix that while breaking it apart

### Acceptance

- Primary pages render useful initial state without mount-only fetch dependency
- Navigation remains fast; no heavy blocking server load introduced
- Fewer route waterfalls on first render
- File detail page no longer lives in `routes/api/`

## PR3: Shared client resource layer + stale-while-refetch

### Scope

- Introduce a shared keyed client loader for file-detail data first
- Deduplicate fetch/cache/state handling now spread across the file-detail page and router cards
- Build the file-detail loader on a small reusable resource primitive, not a fully generic framework
- Keep `/details` as the eager bootstrap for summary + seeded structure/spectrum/IP-count data
- Normalize loader state internally, then derive router row view models from that cache
- Use a module-level keyed cache (`dataset + slug`) with a simple bounded eviction policy
- Keep stale structure/spectrum data visible while targeted refetches run
- Break the file-detail page and router card into smaller components while moving fetch ownership out of the page shell
- Leave singularities out of the new core loader model for now

### Candidate resources

- file details
- netflow stats
- IP stats
- protocol stats
- spectrum stats
- singularities requests

### Implementation direction

- File-detail loader owns fetch orchestration and cache updates; page composes derived row models only
- Row model exposes summary, per-side resource slots, and narrow actions such as `refreshStructure(source)` and `refreshSpectrum(source)`
- Resource shape stays explicit: `{ data, loading, error, refresh }`
- Components consume derived resource state; do not own fetch lifecycle
- Router cards become presentational; prop surface collapses to row models + formatting helpers

### Acceptance

- File-detail page state surface is materially smaller
- File-detail route owns a single loader API instead of page-local fetch/map orchestration
- Partial range extensions do not blank charts
- Shared cache/resource logic replaces bespoke per-chart fetch logic
- Reduced duplicate request/state code across chart components

## PR4: Chart architecture consolidation + singularities capability decoupling

### Status

- Not executed
- Prior attempt abandoned; regressions in render lifecycle / chart interaction
- Keep this as future work, not current branch scope

### Scope

- Extract shared chart controller/helpers for:
  - create/update/destroy
  - theme sync
  - crosshair sync
  - range selection
  - loading overlay strategy
- Reduce duplicated logic across NetFlow/IP/Protocol/Spectrum charts
- Replace avoidable `$effect`-driven state syncing with `$derived` where possible
- Fetch only current router for `SpectrumStatsChart`
- Remove filesystem-awareness from the web UI for singularities
- Stop treating singularities as a normal file-detail resource slot
- Gate singularities behind backend-declared capability/config if it remains user-accessible
- Prefer a future DB-backed or precomputed path over direct local-file/runtime assumptions

### Acceptance

- Shared chart lifecycle path used by core charts
- Fewer imperative effects; clearer reactive graph
- Spectrum fetch scope matches visible router selection
- File-detail UI no longer depends on per-file disk checks or direct local-system assumptions for singularities

## Success signals

- Faster first useful render
- Fewer blank/loading flashes during filter/range changes
- Lower code duplication in chart components
- Heavy analysis only when user requests it
- Clear SSR/client boundary: shell SSR, heavy data progressive
