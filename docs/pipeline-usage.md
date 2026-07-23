# Pipeline usage

The pipeline lives in `tools/netflow-db/pipeline.py`.

It processes explicit CSV inputs or prepared nfcapd trees into the canonical
SQLite stats tables.

## Dataset run

Use `datasets.json` for dataset roots and output paths, then run a bounded
nfcapd tree ingest:

```bash
python tools/netflow-db/pipeline.py \
  --dataset uoregon \
  --start-date 2025-02-11 \
  --end-date 2025-02-12
```

Useful flags:

- `--database-path`: override the SQLite output path
- `--ip-prefix`: ingest flows when either endpoint belongs to the canonicalized CIDR
- `--src-visibility` / `--dst-visibility`: independently require `literal` or
  `anonymized` address visibility
- `--start-time` / `--end-time`: limit a half-open local time window. These must
  align to aggregate bucket boundaries so coarse rows stay complete.
- `--maad-bin`: path to the MAAD helper binary
- `--max-workers`: worker process count
- `--force`: rewrite selected nfcapd buckets even when marked processed

## Config run

For CSV imports or mixed inputs, pass a pipeline config:

```bash
python tools/netflow-db/pipeline.py \
  --config scripts/local/ugr16-csv.pipeline.json \
  --database-path data/ugr16/netflow.sqlite
```

Local helper:

```bash
scripts/local/build_ugr16_netflow.sh --config scripts/local/ugr16-csv.pipeline.json
```

### Flow selection

Selection criteria are combined with AND. The IP prefix matches either the
source or destination endpoint, while source and destination visibility are
independent. Dataset-mode selection flags require an explicit
`--database-path`, because a selected population is a distinct database
product.

Config-mode runs define the same semantics once at the top level:

```json
{
  "selection": {
    "ip_prefix": "192.0.2.0/24",
    "src_visibility": "literal",
    "dst_visibility": "anonymized"
  },
  "inputs": []
}
```

Omit any criterion to leave it unrestricted. CSV coverage is observed before
selection, so selected-out buckets remain dense zero buckets. Native nfcapd
ingestion pushes the CIDR predicate into every grouped scan and applies the
visibility predicate to grouped rows before statistics are accumulated.

### CSV duration mapping

The optional logical `columns.duration` mapping is measured in seconds. Decimal
seconds are converted exactly to integer milliseconds, so values may have at
most millisecond precision. A mapped, nonblank duration is authoritative for
the duration value, while mapped endpoints must still satisfy
`time_end >= time_start`. When duration is absent, ingestion derives it from
mapped `time_start` and `time_end` timestamps when both are available.

### Database and input identity

Every pipeline database is bound on first use to independently fingerprinted
schema, normalized flow selection, and result-configuration semantics. Result
configuration includes the pipeline timezone and the enabled MAAD backend
contract, but excludes paths, discovery windows, worker counts, and CSV
mappings. A populated database without this identity is not adopted; rebuild it
into a new database.

The observation-metrics schema adds duration and TTL sufficient statistics plus
port cardinality rows. It is a new database product, not an in-place migration:
build it at a fresh output path. Keep each flow selection in its own database as
well; neither a schema change nor a different prefix or visibility selection may
reuse an existing product database.

Each CSV, nfcapd file, and synthetic gap also records an exact input revision.
The revision combines SHA-256 content identity with a canonical decoder
fingerprint. Reusing a successfully processed locator with changed content or
decoder semantics is rejected instead of silently mixing results. Forced nfcapd
tree processing remains the explicit rewrite mechanism.

Hashing is guarded by file device, inode, size, modification time, and change
time snapshots before and after SHA-256 and again after decoding. A completed
input with the same snapshot reuses its persisted digest, so unchanged large
inputs are not reread on every run. A changed snapshot is hashed once and then
conflicts with the prior revision unless the nfcapd run is forced. This policy
detects ordinary replacement and in-place modification; filesystems or
adversarial writers capable of changing bytes while preserving every tracked
stat field remain outside the stability guarantee.

Canonical nfcapd tree runs also bind the database to their normalized logical
source membership. Renaming a source or reassigning physical members requires a
new database; a bounded rerun cannot safely rewrite historical buckets outside
its window. Synthetic gaps retain the expected native path and verify that it
is still absent immediately before and at the end of publication. A filesystem
entry that appears after discovery aborts and rolls back that publication. A
filesystem race after the final absence check but before SQLite commit remains
possible and is reconciled as changed input on the next run.

## Compile helpers

The canonical MAAD helper is built with:

```bash
scripts/build_maad_fast.sh
```

Native nfcapd ingestion requires the compiled one-pass reducer:

```bash
scripts/build_nfdump_reducer.sh
```

The reducer consumes the versioned `nfdump-csv-15-v1` field contract. This
keeps deployment independent of nfdump's internal reader ABI and supports old
type-2 capture blocks. Because CSV cannot expose extension presence, the native
contract treats a min/max TTL of `0` (or blank) as missing. There is no silent
Python fallback when the helper is unavailable; the pipeline fails closed.

`nfdump` must be on `PATH` for nfcapd inputs. Use
`scripts/run-with-nix-if-available.sh` when the local environment needs Nix
tooling.

## Analysis window exports

`extract_window` creates bounded SQLite and/or Parquet analysis artifacts from
a completed pipeline database:

```bash
python tools/netflow-db/extract_window \
  --source-db data/uoregon-0-220-v3/netflow.sqlite \
  --output-dir data/uoregon-0-220-v3/extracts/2025-06 \
  --start 2025-06-01 \
  --end 2025-07-01 \
  --output sqlite \
  --output parquet
```

These slices preserve the portable stats tables, including port cardinalities
and the duration/TTL sums and counts needed for weighted recomputation. Their
manifest records the source pipeline product fingerprint and normalized flow
selection. A selected pipeline product requires an explicit `--output-dir`, so
two prefixes or visibility selections cannot silently publish to the same
default path.

Window exports are analysis artifacts, not deployable web databases. They do
not include dataset metadata, processed-input provenance, coverage drilldown,
or the source `pipeline_product` table. Use the complete pipeline database for
the web application and operational inspection.

## Sanity check

If you edited backend Python, run:

```bash
cd tools/netflow-db
python -m py_compile *.py
```
