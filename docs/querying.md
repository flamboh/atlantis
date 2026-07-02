# Querying the Database

Each dataset has its own SQLite database at the dataset's configured `db_path`.

## Direct Access

```bash
sqlite3 data/<dataset>/netflow.sqlite
```

Useful SQLite commands:

```text
.tables
.schema traffic_stats
.schema protocol_stats
.schema address_count_stats
.schema address_structure_stats
```

## Schema Overview

**`traffic_stats`** — per-source flow/packet/byte counts:

- `source_id`, `granularity`, `bucket_start`, `bucket_end`
- `ip_version`, `src_visibility`, `dst_visibility`
- `flows`, `packets`, `bytes`

**`protocol_stats`** — unique protocol counts per source/time bucket.

**`address_count_stats`** — unique source/destination address counts per source/time bucket.

**`address_structure_stats`** — MAAD-backed address structure rows per source/time bucket.

`granularity` is one of `5m`, `30m`, `1h`, `1d`.

## Example Queries

Daily flow/packet summary for a time window:

```sql
SELECT
    DATE(bucket_start, 'unixepoch') AS day,
    source_id,
    SUM(flows) AS flows,
    SUM(packets) AS packets,
    SUM(bytes) AS bytes
FROM traffic_stats
WHERE source_id IN ('source1', 'source2')
  AND granularity = '1d'
  AND src_visibility = 'all'
  AND dst_visibility = 'all'
  AND bucket_start BETWEEN strftime('%s', '2025-01-01') AND strftime('%s', '2025-01-08')
GROUP BY day, source_id
ORDER BY day, source_id;
```

30-minute protocol breakdown for a single day:

```sql
SELECT
    datetime(bucket_start, 'unixepoch') AS bucket,
    protocol,
    protocol_count
FROM protocol_stats
WHERE source_id = 'source1'
  AND granularity = '30m'
  AND ip_version = 4
  AND src_visibility = 'all'
  AND dst_visibility = 'all'
  AND bucket_start BETWEEN strftime('%s', '2025-01-03') AND strftime('%s', '2025-01-04')
ORDER BY bucket_start, protocol;
```

Per-source address counts:

```sql
SELECT source_id, bucket_start, ip_version, address_side, address_count
FROM address_count_stats
WHERE granularity = '1h'
  AND source_id IN ('source1', 'source2')
  AND src_visibility = 'all'
  AND dst_visibility = 'all'
  AND bucket_start BETWEEN strftime('%s', '2025-01-01') AND strftime('%s', '2025-01-02')
ORDER BY source_id, bucket_start, ip_version, address_side;
```
