# ATLANTIS domain context

ATLANTIS turns network-flow inputs into immutable-schema aggregate database products and serves
those products as time-series charts. The ingestion pipeline, database, API, and charts share one
data contract; none of those layers may reinterpret a missing input as traffic.

## Domain language

- **Capture unit:** the smallest item whose presence proves that a five-minute interval was
  observed. It is one physical member for `nfcapd`, or one resolved logical source across all CSV
  inputs in the pipeline.
- **Coverage:** evidence about expected capture units, independent of metric values.
- **Complete:** every expected unit was observed and none was rejected.
- **Partial:** some evidence is usable, but expected evidence is missing or rejected.
- **Unknown:** no expected unit was observed. Unknown is not zero.
- **Observed zero:** complete coverage with a numeric metric value of zero, including a valid CSV
  row excluded by the product's flow selection.
- **Derived availability:** whether a secondary result such as MAAD or an average exists. A missing
  derived value does not change capture coverage.
- **Physical member:** one native capture stream represented by canonical `nfcapd` files.
- **Logical source:** the queryable source exposed by a database product. It may combine physical
  members when its metrics are additive or name one exact member set for cardinality results.
- **Product database:** a database bound to its schema, flow selection, result configuration, and
  source layout. A semantic contract change produces a new product rather than silently changing an
  existing one.

## Invariants

- Time windows are half-open: `[start, end)`.
- Five-minute evidence is canonical. Coarser coverage adds the expected, observed, and rejected
  counts of its five-minute children.
- Metric rows and `bucket_coverage` rows are published in the same transaction.
- Missing `nfcapd` files produce evidence, not synthetic metric rows.
- A previously observed native input is not erased merely because the file later disappears.
- CSV files make temporal claims only through rows with usable timestamps. Internal gaps inside a
  source's evidenced envelope are unknown; empty and header-only files establish no envelope.
- Overlapping CSV inputs for the same source and bucket form one coverage unit.
- Server time-series responses are continuous `TimeBucket<T>` sequences. Unknown buckets contain
  `data: null`; complete buckets without stored additive rows receive the route's numeric zero
  payload; partial buckets expose numeric data only when usable metrics exist.
- Charts preserve real temporal spacing. Unknown data breaks a line, partial data is visibly
  qualified, and observed zero is plotted normally.

## Main boundaries

- `tools/netflow-db` owns input evidence, canonical aggregation, coverage materialization, product
  identity, and verification.
- `apps/web/src/lib/server/db/coverage.ts` owns construction of bounded API timelines from stored
  coverage and metric rows.
- API routes own metric-specific aggregation and zero payloads, but not coverage inference.
- Chart utilities own the shared visual vocabulary for unknown and partial buckets.

The accepted design and its tradeoffs are recorded in
[`docs/adr/0001-explicit-capture-coverage.md`](docs/adr/0001-explicit-capture-coverage.md).
