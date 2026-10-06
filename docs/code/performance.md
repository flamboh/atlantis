# Dashboard performance

Measure a production SQLite deployment with `bun run perf`. The command reads HTTP APIs and local build artifacts. It does not open or modify a database.

## Create an isolated stage

Use the self-hosted stack with a data directory containing `<product>/netflow.sqlite`. Keep host paths in an external shell file, outside the checkout. Export these settings before deploying:

```bash
export ATLANTIS_SELF_HOSTED_DOCKER_HOST=ssh://user@server.example.com
export ATLANTIS_SELF_HOSTED_DATA_DIR=/path/to/products
export ATLANTIS_SELF_HOSTED_DATA_USER=1000:1000
export ATLANTIS_SELF_HOSTED_STATE_DIR=/srv/atlantis-perf
export ATLANTIS_SELF_HOSTED_PORT=8090
export ATLANTIS_SELF_HOSTED_READ_ONLY=true
bun run plan:self-hosted --stage perf --no-input
bun run deploy:self-hosted --stage perf --yes --no-input
```

The `perf` stage uses the Docker context `atlantis-perf`, container `atlantis-perf-web`, and port `8090` by default. It requires read-only data, rejects port `8080`, and requires a state directory outside the checkout. Docker mounts the data directory read-only, and the SQLite driver also opens products read-only with `query_only` enabled. Completed SQLite products without a live WAL can be served this way. No copy, hardlink, or database change is needed.

The launcher starts Alchemy in `ATLANTIS_SELF_HOSTED_STATE_DIR`, so state and logs live under its `.alchemy/` directory. The build context still comes from the checkout. Reuse the same state directory and settings to redeploy or destroy that stage. Other stages preserve their existing names, port defaults, writable mounts, and checkout state default.

Reach a remote stage through its printed SSH tunnel. Verify `/api/datasets` and a chart before benchmarking. Keep the private product and reports out of commits.

## Record a baseline

Build from the same checkout as the deployment. Install the Chromium version pinned by Playwright once:

```bash
bun install --frozen-lockfile
bunx playwright install chromium
ATLANTIS_DB_DRIVER=sqlite bun run build:web
bun run perf --base-url http://127.0.0.1:8090 \
  --dataset sample --end-date 2026-06-30 \
  --output /tmp/atlantis-perf/baseline
```

The script discovers the selected dataset and sources over HTTP. `--start-date` defaults to the dataset's default date, and `--end-date` defaults to today's UTC date. Dates have the dashboard's inclusive Pacific-day semantics; API timestamps use half-open intervals. Pin both dates when comparing changes. The six-month query starts at the same date, and the hourly filtered and IPv6 variants cover up to seven days. A dataset without locality uses the all-directions filter.

Each metadata or aggregate API query gets one discarded warmup and five sequential measured requests. The aggregate routes cover traffic, characteristics, IP counts, protocol counts, coverage, and, when computed, MAAD dimensions, spectrum, and structure. API latency includes receipt of the full response body. These are warm-server measurements, without concurrency or OS-cache eviction.

Each page gets three fresh Chromium contexts, with a 1280 x 800 viewport, Pacific timezone, reduced motion, and no network or CPU throttling. Pages cover dataset selection, dashboard default and six-month ranges, an hourly filtered dashboard, file navigation, and a file detail when MAAD exists. `--file-slug` selects a populated five-minute bucket; the default is midnight on the start date. Use `--cpu-slowdown 4` for an additional CPU-throttled comparison, with a separate output prefix.

The reports contain TTFB, FCP, LCP, chart readiness, long tasks, observed TBT, resource timings, and JS heap before and after explicit GC. LCP uses the last candidate before scrolling. Initial readiness excludes deferred chart cards. The script scrolls to each deferred card and records all-chart readiness and heap.

TBT sums task time beyond 50 ms after FCP through the initial settling interval. It is not Lighthouse's interactive-window TBT. Chromium's DevTools protocol reports heap usage. GC between phases affects all-chart timings and remains constant across runs.

Chart readiness requires the loading text to disappear, a chart graphic to exist, and two animation frames to pass. It does not wait for every animation to finish. The default graphic selector supports canvas, accessible SVGs, and `data-chart-rendered="true"`. Use `--chart-selector` if a chart migration changes that contract. Unavailable MAAD cards may settle without a graphic.

Bundle sizes come from the Vite client manifest and SvelteKit's generated route nodes in `apps/web/.svelte-kit`. The JSON inventory includes every JS/CSS file, raw and gzip level 9, plus deduplicated static imports, boot code, and layouts for each route. Shared files count once within a route; route totals overlap. Dynamically imported files also appear in the inventory and observed browser resources. A served JS/CSS filename absent from the local build fails the run, so rebuild both deployment and local artifacts after a UI change.

For Docker deployments, export the production build instead of measuring a local build:

```bash
bun run perf:bundle /tmp/atlantis-perf/build
bun run perf --base-url http://127.0.0.1:8090 \
  --dataset sample --end-date 2026-06-30 \
  --bundle-dir /tmp/atlantis-perf/build/.svelte-kit/output/client \
  --output /tmp/atlantis-perf/baseline
```

The export command rebuilds the Dockerfile's `build` target and copies its artifacts through a temporary container. Docker reuses cached layers. The command removes only that temporary container. `ATLANTIS_PERF_DOCKER_CONTEXT` defaults to `atlantis-perf`. Local builds can contain extra Tailwind classes from test files that Docker excludes.

`--runs`, `--page-runs`, `--warmups`, `--timeout-ms`, `--settle-ms`, and `--bundle-dir` are configurable. p50/p95 use nearest-rank percentiles. With five API samples and three page samples, p95 is the slowest observed run. Increase sample counts for a more stable comparison.

JSON retains individual samples and errors. HTTP, browser, missing-chart, or build-mismatch errors cause a nonzero exit code and a failed report. Setup errors fail before a report is written.

The command writes `<output>.json` and `<output>.md`. Keep them outside the repository. Use the same machine, dataset, dates, sample counts, browser, and deployment limits for later layers, and avoid overlapping builds or tests with a baseline run.
