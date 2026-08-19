# ADR 0001: Store capture coverage separately from metrics

- Status: Accepted
- Date: 2026-08-19
- Issue: [#94](https://github.com/flamboh/atlantis/issues/94)

## Context

ATLANTIS previously represented missing native capture files with synthetic zero statistics. CSV
ingestion also filled intervals between observed rows. Both behaviors made “we observed no traffic”
indistinguishable from “we have no capture evidence,” and sparse API responses let charts compress
or span those gaps.

Coverage is source evidence, while flow totals, cardinalities, and MAAD are measurements derived
from the evidence that is available. Combining those concepts in nullable metric rows cannot express
complete zero, useful partial data, unknown data, and unavailable derived results without ambiguity.

## Decision

Persist one `bucket_coverage` row per source, granularity, and bucket alongside metric products. A
row records expected, observed, and rejected unit counts and derives a state of `complete`, `partial`,
or `unknown`.

The canonical five-minute unit is one physical member for `nfcapd` and one resolved source across
all configured CSV inputs. Coarser coverage is additive. Missing native files and internal CSV gaps
publish coverage without fabricated metrics. Valid selected-out CSV rows and successfully decoded
empty native files remain complete observed-zero buckets.

Partial products are allowed by default. `--require-complete` checks coverage after publication and
returns failure without deleting the inspectable database. Native content replacement requires
`--force`; newly arrived files repair prior missing evidence and invalidate affected coarse derived
rows for exact recomputation.

All aggregate time-series endpoints return a continuous shared envelope:

```ts
type TimeBucket<T> = {
  bucketStart: number;
  bucketEnd: number;
  coverage: {
    state: "complete" | "partial" | "unknown";
    observedUnits: number;
    expectedUnits: number;
  };
  data: T | null;
};
```

Unknown buckets have null data and break chart lines. Partial numeric data remains visible with a
hollow point, dashed adjoining segments, and coverage details. Complete zero is rendered as an
ordinary zero.

The coverage schema and semantics are part of product identity. Databases built under the prior
contract are not upgraded in place by the normal pipeline.

## Consequences

- Consumers can distinguish missing evidence from zero traffic without seeing filenames or decoder
  diagnostics.
- Logical sources and rollups can retain useful partial additive metrics.
- Every metric route must define its complete-zero payload and keep derived availability separate.
- Coverage storage and timeline generation add rows proportional to source buckets, so queries and
  ingestion must remain streamed and indexed.
- The in-progress UOregon database requires a one-off, source-specific migration after its rebuild
  completes. Its known five-minute `flows = 0` signal may classify missing physical files, but that
  inference must never enter the general pipeline.
