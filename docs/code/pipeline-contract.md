# Pipeline contract

This document explains the invariants of a pipeline database. Read it before you change result semantics or input identity.

## Product identity

The pipeline binds each database to one product identity. The identity contains these values:

- The schema version and table versions
- The normalized flow selection
- The canonical endpoint-locality rules
- The pipeline timezone
- The native decoder contract
- The concentration contract version, measure set, and fixed prefix lengths
- The MAAD enabled state, contract version, configuration, and measure set, plus
  `"internal_side": false` when the product skips internal-side address sets

The pipeline rejects a database when its identity differs. Build a new database for a different product identity.

A database with populated pipeline tables and no product identity is not adopted. Rebuild that database at a new path.

## Flow selection

Selection conditions use AND logic. The IP prefix matches either endpoint.

Source locality and destination locality are independent conditions. Keep each selected population in a separate database.

Coverage is observed before selection. Thus, selected-out buckets remain as dense zero buckets.

Native nfcapd input pushes the IP prefix condition into the nfdump filter. Locality conditions apply before statistics accumulate.

`daily_active_sources` is a separate, fixed selection policy for per-`/16` active-source products.
It accepts exactly one IPv4 `/16` and exactly one `nfcapd_tree` input. For each complete
local calendar day, it makes two bounded passes over every unique physical member:

1. Select IPv4 traffic from an internal source in the `/16` that uses TCP or UDP and source port
   1024 or greater. Sum flows, packets, and bytes by exact source address across physical members.
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

## Endpoint locality and direction

Each dataset declares locality rules (see [dataset configuration](../user/datasets.md#classify-internal-and-external-endpoints)).
An endpoint is `internal` when any rule matches: a listed CIDR prefix, a listed address, or, with
`tos_anonymized`, the capture anonymizer's flag in the low two source-ToS bits (bit 1 for the source,
bit 0 for the destination). Every other endpoint is `external`. Adapters classify each flow once,
before selection and aggregation. CSV and native nfcapd input use the same rules, and the native
stream carries the ToS flags in its record tag.

The stats tables keep the endpoint pair in `src_locality` and `dst_locality`. Every bucket and IP
version stores five dense rows: `all`/`all` and the four exact pairs. Missing combinations are
zero-filled rows, and the exact pairs sum to the `all` row for additive metrics at every
granularity. Readers derive direction from the pair:

| `src_locality` | `dst_locality` | Direction |
| -------------- | -------------- | --------- |
| `internal`     | `external`     | egress    |
| `external`     | `internal`     | ingress   |
| `internal`     | `internal`     | lateral   |
| `external`     | `external`     | transit   |
| `all`          | `all`          | all       |

Transit remains a first-class scope so misclassified or unexpected records stay visible.

The result configuration identity contains the canonical rules: the `tos_anonymized` flag and a
count and SHA-256 digest of the aggregated prefix list and the deduplicated address list. Reordering
rules or splitting a prefix does not change the identity. Any change to the matched address space
does, and address-file contents are read on every run. The digest keeps private address lists out of
database metadata and export manifests.

## MAAD measures

Each scope and address side keeps one entry per unique address with its summed packets and bytes.
Rollups merge children by summing those counters per address, so a rollup's per-address totals
equal the sums over its five-minute children. The per-address map adds 16 bytes to each address
entry.

Every MAAD computation runs three measures on the same entries, for both IP versions and every
granularity. `addresses` weighs each address as 1 and stores structure, spectrum, and dimensions.
`packets` and `bytes` weight moments and D1 entropy by the summed counter and store structure and
dimensions only. Prefix validity always uses distinct-address counts. A weighted measure excludes
addresses whose counter sums to 0 and records the excluded count in `zero_weight_addrs`. The
measure set is part of the result configuration identity.

## Internal-side MAAD scopes

An internal-side address set holds only internal addresses: the source addresses of
internal-source traffic (`src_locality = 'internal'`, `address_side = 'source'`) and the
destination addresses of internal-destination traffic (`dst_locality = 'internal'`,
`address_side = 'destination'`). By default the pipeline does not compute MAAD for these sets, for
any measure. It skips the computation itself, not only the write. `all`/`all` scopes and every
external-side set are still computed. The address-count rows of every scope are unaffected.

Per IP version and bucket, the default computes six of the ten address sets: both sides of
`all`/`all` and `external`/`external`, the source side of `external`/`internal`, and the
destination side of `internal`/`external`. A `daily_active_sources` product selects only
internal sources, so its traffic falls in `internal`/`internal` and `internal`/`external`; the
default leaves it with MAAD for both sides of `all`/`all` and the destination side of
`internal`/`external`. Its other computed scopes stay empty.

Set `maad_internal_side: true` in the dataset entry or at the top level of a pipeline config to
compute every set. Only the skipping product records the setting in its identity, as
`"internal_side": false` inside the `maad` result configuration. A product that computes every set
keeps the identity it had before the setting existed, so existing databases stay extendable with
the setting turned on, and a database built with one setting rejects runs with the other. Coordinated
subset runs must agree on it.

The `datasets.maad_internal_side` column mirrors the setting for the dashboard, which cannot read
the product identity in D1. The column defaults to 1 everywhere. The pipeline always writes it explicitly, and adds it when it opens a database
from before the setting existed. `verify` reads the setting from the product identity, fails when
the column disagrees, rejects any internal-side MAAD row in a skipping product, and with
`--require-maad-data` requires a MAAD row for every address set the product computes. `compare`
counts reference rows for skipped sets as `skipped_reference_rows` instead of reference-only rows
and rejects candidate rows for them. `merge-shards` refuses a skipping shard that stores
internal-side rows.

## MAAD storage

`address_maad_stats` holds one row per bucket, scope, address side, and measure. Dimensions are
`REAL`. The structure function (`tau`), its standard deviation (`tau_sd`), and the spectrum are
little-endian f32 BLOBs. The only rounding is `value as f32`, which rounds to the nearest even.
Nothing else rounds, truncates, or drops a value.

- `tau` and `tau_sd` hold `q_count` values each. Element `i` is at `q_min + i * q_step` from the
  `maad_q_grid` row of the same IP version.
- `spectrum` holds interleaved `(alpha, f)` pairs. A zero-length BLOB is a computed, empty
  spectrum. `NULL` means the measure does not compute one.
- A result below the minimum address count stores `NULL` dimensions and curves.
- D0 is `-tau(0)` and D2 is `tau(2)`. Their standard deviations are the `tau_sd` values at those q.
  D1 has no standard deviation.

The schema rejects BLOBs that are not whole f32 values or whole spectrum pairs, and a `tau_sd`
whose length differs from `tau`. `verify` checks every curve against the q grid and the spectrum
presence of every measure. The dashboard API fails a request whose curve lacks a grid or disagrees
with `q_count` instead of drawing a partial curve. `compare` checks dimensions and each curve
element within the MAAD tolerance, and requires identical `maad_q_grid` rows for every IP version
whose curves both databases store in the window. Changing this encoding is a MAAD contract change,
so it needs a fresh product database.

Native and CSV ingestion drop flows whose reported packet count is 0 before any statistic
accumulates, so they add no traffic, protocols, ports, or addresses. The bucket keeps its observed
coverage, and publishing logs the dropped flow count per bucket. A CSV row without a packet count
has an unknown count rather than a reported zero, so it still contributes.

## Concentration statistics

`address_concentration_stats` stores conventional concentration controls for every address set and measure that MAAD computes.
It uses the same bucket, locality scope, address side, and IP version keys as `address_maad_stats`.
Disabling MAAD disables concentration statistics. The `maad_internal_side` setting applies to both tables.

Each measure uses the same per-address weights as MAAD and excludes zero weights.
`entry_count` counts the retained addresses. `weight_total` stores their summed weight, including below the minimum count of two.
Below that minimum, all statistical columns are `NULL`.
An empty MAAD curve caused by prefix pruning does not suppress concentration values above that minimum.

- `hhi` is the sum of squared weight shares. For `addresses`, it equals `1 / entry_count`.
- `top1_share`, `top10_share`, and `top100_share` sum the largest address weights and divide by `weight_total`.
- Prefix entropy is Shannon entropy in bits after summing weights by fixed-length prefix.
  IPv4 uses `/8`, `/16`, `/24`, and `/32`. IPv6 uses `/32`, `/48`, `/64`, and `/128`.
  Columns for the other IP version are `NULL`. Both families use `entropy_p32`.

The pipeline computes concentration from MAAD's sorted address entries without another flow scan or full weight sort.
Rollups recompute from merged per-address child maps. They never average child statistics.
All statistic columns and `weight_total` are `REAL` values without f32 rounding. `entry_count` is an integer.

The schema and result configuration record this contract. Existing products require a fresh database path.
`verify --require-maad-data` requires matching concentration rows for every computed MAAD set and measure.
`verify` checks retained counts against MAAD and rejects skipped internal-side rows.
`compare` applies `--maad-absolute-tolerance` to concentration values and `weight_total`, and compares `entry_count` exactly.
`merge-shards` validates concentration day ownership and internal-side skipping before copying rows.
Analysis-window exports include the new table.

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

The canonical input granularity is five minutes. The pipeline also creates 10-minute, 30-minute, one-hour, and one-day rows. Every rollup is a local-time-aligned, half-open window built from its five-minute children; a 10-minute row covers exactly two of them.

Time windows use the configured pipeline timezone. The default timezone is `America/Los_Angeles`.

The `--start-time` and `--end-time` limits are half-open. Their boundaries must align with local-day boundaries so aggregate rows stay complete.

The observation schema stores duration and TTL sums and counts. It also stores port-cardinality rows.

## Day-sharded products

Every stored row belongs to exactly one local day. The coarsest rollup is `1d`, and every rollup
bucket (`30m`, `1h`, `1d`) is computed from the local wall clock, so it never crosses local
midnight. nfcapd-tree windows must start and end on local-day boundaries, and the pipeline
processes and commits one local day at a time together with its `daily_product_completion`
markers.

Zero-fill depends on the requested window, which is not part of the product identity. Each
member's first and last capture in the whole tree bound its coverage. With an explicit end date,
the pipeline also publishes coverage for every five-minute bucket of the requested window. Without
one, days outside a member's capture bounds get no coverage rows for that member, yet still receive
completion markers. A day therefore publishes the same rows alone or inside a longer range only
when every run names an explicit end date.

`netflow-db merge-shards` relies on this. It combines pipeline products built over disjoint
local-day ranges into a new product and refuses before it writes anything unless:

- every shard has the same SQLite schema (the `datasets` table is compared by its columns, so a
  table upgraded in place matches a fresh one), the table contract this build writes, the same product
  identity (schema, selection, and result configuration, including the nfdump path and digest),
  the same nfcapd source layout, and the same dataset metadata;
- every shard has identical `maad_q_grid` rows, including none at all when MAAD is disabled. The
  merge keeps the first shard's copy of this and the other shared tables;
- every shard is an nfcapd-tree product without CSV inputs;
- every completion marker names the shard's product identity;
- every completion marker has one five-minute `bucket_coverage` row for each local five-minute
  bucket of its source and day, which rejects shards built without an explicit end date;
- completed days do not overlap across shards;
- every row of every day-owned table (stats, `bucket_coverage`, `input_evidence`,
  `processed_inputs`) lies inside one of its own shard's completed days.

The last two checks together prove that shard keys are disjoint. Plain inserts would also fail on
any primary-key conflict. The merge writes a private temporary file beside the output: it copies
the first shard with the SQLite backup API, then attaches, inserts, commits, and detaches each
remaining shard in turn, with journaling and synchronous writes off. It checks each insert's row
count against validation, syncs the file, and renames it into place, so the output appears only
when the merge is complete. A dataset `default_start_date` that every shard shares is kept,
which covers configured dates. Otherwise every shard must hold the date inferred from its own
earliest five-minute traffic, or the fallback date if it has no traffic. The merge then sets the
date from the earliest traffic in the merged product, and uses the fallback only when the whole
product has no traffic. It recomputes that date inside every shard's insert transaction, so a
partial product always carries the date of the days it holds. The copied markers make a later `pipeline` run over the merged days a no-op. A merged
product is itself a valid shard.

`--consume` keeps peak disk near the output size plus one shard. After every check passes, the
merge renames the first shard into the temporary output (or copies it across filesystems), then
commits each remaining shard with a rollback journal and `synchronous=FULL`. Only after that
commit returns does it delete the shard and its sidecars. A shard counts as consumed as soon as
the rename, cross-filesystem copy, or commit succeeds. Any later failure keeps the temporary
output and reports it with the consumed shards. This includes a failed directory sync or a
failed shard-file deletion, which is reported as its own cleanup error. The completed days of
the temporary output are exactly the consumed shards, and merging it with the remaining shards
resumes the job. Tests cover failures during a shard insert, and deletion failures right after
the first shard's rename and after a later shard's commit. Crash durability is not tested. Once the output
has been renamed into place, a failed directory sync is reported as a published product, not as
a partial merge.

### Cluster launcher

`scripts/netflow-db-cluster.sh` drives a sharded build over plain SSH (`BatchMode`); every host uses
the same `--remote-dir` path, so the deployed nfdump path, and with it the product identity, is
identical on all shards. It splits the inclusive range into `min(days, slots)` contiguous shards
that differ by at most one day and assigns them to slots round-robin across hosts. Each shard runs
as a detached remote job named by dataset and day range, with its log and exit status in
`<remote-dir>/work`. The launcher records the host and range layout in `<work-dir>/layout` and
refuses a rerun with a different layout. A rerun reattaches running shards, lets finished days
resume as no-ops, copies every shard back as a consistent SQLite snapshot, deletes any partial
merge from an earlier run, merges with `--consume` unless `--keep-shards` is set, and runs
`verify`. The remote shard databases remain the source of truth until the user deletes them.

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
  --source-db data/example/netflow.sqlite \
  --output-dir data/example/extracts/<YYYY-MM> \
  --start <YYYY-MM-DD> \
  --end <YYYY-MM-DD> \
  --output sqlite \
  --output parquet
```

The end value is exclusive. Exports read a consistent source snapshot. Each export publishes a manifest and SQLite or Zstd-compressed Parquet artifacts.

The manifest records the source product and normalized selection. `--timezone` or `NETFLOW_TIMEZONE` sets the window timezone. The default timezone is `America/Los_Angeles`.

An analysis export is not a deployable web database. It omits dataset metadata, provenance details, and the source product table.
