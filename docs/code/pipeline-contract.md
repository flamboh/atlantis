# Pipeline contract

This document explains the invariants of a pipeline database. Read it before you change result semantics or input identity.

## Product identity

The pipeline binds each database to one product identity. The identity contains these values:

- The schema version and table versions
- The normalized flow selection
- The pipeline timezone
- The native decoder contract
- The MAAD enabled state

The pipeline rejects a database when its identity differs. Build a new database for a different product identity.

A database with populated pipeline tables and no product identity is not adopted. Rebuild that database at a new path.

## Flow selection

Selection conditions use AND logic. The IP prefix matches either endpoint.

Source visibility and destination visibility are independent conditions. Keep each selected population in a separate database.

Coverage is observed before selection. Thus, selected-out buckets remain as dense zero buckets.

Native nfcapd input pushes the IP prefix condition into the nfdump filter. Visibility conditions apply before statistics accumulate.

`daily_active_sources` is a separate, fixed selection policy for the UOregon `/16` candidate
products. It accepts exactly one IPv4 `/16` and exactly one `nfcapd_tree` input. For each complete
local calendar day, it makes two bounded passes over every unique physical member:

1. Select anonymized IPv4 source traffic in the `/16` that uses TCP or UDP and source port 1024 or
   greater. Sum flows, packets, and bytes by exact source address across physical members.
2. Mark sources active at the inclusive thresholds of 3 flows, 20 packets, and 2,000 bytes. Publish
   only the same qualifying-flow population from active sources into the existing five-minute and
   rollup contracts.

There is no destination-port filter and no TCP-flag or SYN filter. Overlapping logical sources do
not double-count activity because the first pass deduplicates their physical members. A day with
any missing physical capture is not published. Activity resets at local midnight, including DST
days.

The normalized product identity records the entire fixed policy. It is not compatible with an old
prefix-only database. A late or changed input can alter the active set for every five-minute bucket
in a day, so repair requires a whole-day `--force` rebuild inside the existing day transaction.

## Coordinated subset runs

Repeat `--dataset` for two or more registry entries to build coordinated subset products. Each entry
supplies its own root and source configuration. Entries do not point to a parent dataset or a
`source_dataset`.

Multi mode supports only `daily_active_sources`. The selected entries must resolve to the same
nfcapd root and logical source layout, and one whole local-day window. The command rejects `--config`,
`--database-path`, partial time bounds, and ambiguous selection overrides. It takes each output path
from the corresponding registry entry.

The run coordinates one daily eligibility scan and one publication scan across the subsets. These
remain two physical phases because qualification needs the complete local day before any bucket can
publish. Local-day completeness is shared, so an incomplete required day blocks publication for every
subset.

Each subset keeps its own immutable product database, product identity, transactions, resume state,
and MAAD configuration. Active sets are still resolved independently. Overlapping subsets may both
receive the same qualifying flow.

Outputs commit sequentially rather than as one cross-database transaction. If the process stops
between product commits, sibling databases can differ by at most the local day that was in flight.
The next run sees the missing completion marker and rebuilds that day for each unfinished product.

## Input identity

Each input records an exact revision. The revision contains a SHA-256 content identity and a canonical decoder fingerprint.

The pipeline rejects changed content at a completed input locator. It does not silently mix two input revisions.

The pipeline checks device, inode, size, modification time, and change time around hashing. Unchanged input can reuse its saved digest.

The pipeline also checks native gaps before publication. A new file at a previously absent locator
stops the transaction so the run can process that evidence instead.

The `--force` option is the explicit rewrite mechanism for nfcapd input.

## Source layout identity

Canonical nfcapd runs bind the logical-source membership to the database. A logical source can contain one or more physical members.

Use a new database after you rename a source or change its members. A bounded run cannot safely change older buckets.

## Capture coverage

Capture evidence is stored independently from metric values. Every source bucket has a coverage
state of `complete`, `partial`, or `unknown`, backed by additive expected, observed, and rejected
unit counts. Unknown coverage is not an observed zero.

The canonical five-minute coverage unit is one physical member for nfcapd input and one resolved
source across all configured CSV inputs. Overlapping CSV inputs therefore contribute one unit per
source bucket. Coarser coverage is the additive rollup of those units.

Missing nfcapd files and internal CSV gaps publish coverage without fabricated zero statistics.
Successfully decoded empty nfcapd files and valid rows removed by flow selection remain complete
observed-zero buckets. CSV files establish bounds only through usable row timestamps; empty or
header-only files establish no bounds.

Partial products are valid by default. `--require-complete` reports failure after publication if the
requested five-minute coverage is incomplete, leaving the database available for inspection.

## Time and aggregation

The canonical input granularity is five minutes. The pipeline also creates 30-minute, one-hour, and one-day rows.

Time windows use the configured pipeline timezone. The default timezone is `America/Los_Angeles`.

The `--start-time` and `--end-time` limits are half-open. Their boundaries must align with local-day boundaries so aggregate rows stay complete.

The observation schema stores duration and TTL sums and counts. It also stores port-cardinality rows.

## Native decoder contract

Native nfcapd input uses the pinned Atlantis nfdump fork in `vendor/nfdump`. The pipeline invokes the fork with `-o atlantis`.

The fork's stdout is the private `atlantis-flow-stream-v1` binary contract. The pipeline decodes this stream directly into canonical scopes before MAAD runs. There is no intermediate reduce step.

The pipeline stops when the fork executable is absent or incompatible.

Request resolution stores the executable's canonical path, one SHA-256 content identity, and a
cheap device/inode/size/timestamp snapshot. The binary identity is part of the product config and
the native input decoder fingerprint, so replacing the executable requires a fresh product and
cannot mix revisions in a resumed database. The snapshot is rechecked around activity and decode
scans and immediately before native publication commits; a change rolls that transaction back.

Before creating an output directory, lock, or database, the pipeline runs one bounded probe against
an isolated empty `-R` directory. The probe requires the exact empty Atlantis stream (header,
terminator, and EOF), including for incomplete-day requests.

To update the fork, rebase its `atlantis-binary-v1` branch onto a reviewed upstream nfdump tag and run the fork's serial test suite. Then advance this repository's submodule pointer and run `./vendor/scripts/compile-nfdump.sh`. Treat a protocol or normalization change as a versioned wire-contract change. Update the Rust decoder and the provenance revision in the same change.

## Analysis-window exports

The `extract-window` command creates bounded SQLite or Parquet analysis artifacts:

```bash
./scripts/netflow-db.sh extract-window \
  --source-db data/uoregon/netflow.sqlite \
  --output-dir data/uoregon/extracts/<YYYY-MM> \
  --start <YYYY-MM-DD> \
  --end <YYYY-MM-DD> \
  --output sqlite \
  --output parquet
```

The end value is exclusive. Exports read a consistent source snapshot. Each export publishes a manifest and SQLite or Zstd-compressed Parquet artifacts.

The manifest records the source product and normalized selection. `--timezone` or `NETFLOW_TIMEZONE` sets the window timezone. The default timezone is `America/Los_Angeles`.

An analysis export is not a deployable web database. It omits dataset metadata, provenance details, and the source product table.
