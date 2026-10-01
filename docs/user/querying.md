# Query a database

Each dataset is one SQLite database, by default at `data/<dataset-id>/netflow.sqlite`.

## Open a database

```bash
sqlite3 data/<dataset-id>/netflow.sqlite
```

List the tables:

```text
.tables
```

Show a table schema:

```text
.schema traffic_stats
```

## Main tables

| Table                 | Content                                                    |
| --------------------- | ---------------------------------------------------------- |
| `datasets`            | Public dataset metadata                                    |
| `source_members`      | Logical-source membership                                  |
| `traffic_stats`       | Flow, packet, byte, duration, and TTL metrics              |
| `protocol_stats`      | Unique protocol counts and protocol lists                  |
| `address_count_stats` | Unique source-address and destination-address counts       |
| `port_count_stats`    | Unique low-port and high-port counts                       |
| `address_maad_stats`  | MAAD dimensions, structure, and spectrum per measure       |
| `maad_q_grid`         | The q grid of each IP version's stored structure functions |
| `processed_inputs`    | Input processing state and provenance                      |

The `granularity` value is `5m`, `10m`, `30m`, `1h`, or `1d`.

Each stats table splits every bucket by direction with `src_locality` and `dst_locality`. Filter on both, or rows from different directions add up twice:

| Direction | `src_locality` | `dst_locality` |
| --------- | -------------- | -------------- |
| All       | `all`          | `all`          |
| Ingress   | `external`     | `internal`     |
| Egress    | `internal`     | `external`     |
| Lateral   | `internal`     | `internal`     |
| Transit   | `external`     | `external`     |

## Query traffic totals

This query gives daily traffic for a half-open time range:

```sql
SELECT
    datetime(bucket_start, 'unixepoch') AS bucket,
    source_id,
    SUM(flows) AS flows,
    SUM(packets) AS packets,
    SUM(bytes) AS bytes
FROM traffic_stats
WHERE granularity = '1d'
  AND src_locality = 'all'
  AND dst_locality = 'all'
  AND bucket_start >= strftime('%s', '<YYYY-MM-DD>')
  AND bucket_start < strftime('%s', '<YYYY-MM-DD>')
GROUP BY bucket_start, source_id
ORDER BY bucket_start, source_id;
```

## Query protocol counts

Each row contains a unique protocol count and a comma-separated protocol list.

```sql
SELECT
    datetime(bucket_start, 'unixepoch') AS bucket,
    source_id,
    unique_protocols_count,
    protocols_list
FROM protocol_stats
WHERE granularity = '30m'
  AND ip_version = 4
  AND src_locality = 'all'
  AND dst_locality = 'all'
ORDER BY bucket_start, source_id;
```

## Query address counts

```sql
SELECT
    source_id,
    bucket_start,
    ip_version,
    address_side,
    unique_address_count
FROM address_count_stats
WHERE granularity = '1h'
  AND src_locality = 'all'
  AND dst_locality = 'all'
ORDER BY source_id, bucket_start, ip_version, address_side;
```

## Query port counts

```sql
SELECT
    source_id,
    bucket_start,
    ip_version,
    port_side,
    port_range,
    unique_port_count
FROM port_count_stats
WHERE granularity = '1h'
  AND src_locality = 'all'
  AND dst_locality = 'all'
ORDER BY source_id, bucket_start, ip_version, port_side, port_range;
```

## Query MAAD results

`address_maad_stats` has one row per bucket, IP version, direction, address side, and `measure` (`addresses`, `packets`, or `bytes`). Always filter on `measure`.

- `d0`, `d1`, and `d2` are the generalized dimensions. They are `NULL` when a bucket has too few addresses.
- `tau` and `tau_sd` are little-endian 32-bit float arrays. Element `i` is at `q = q_min + i * q_step` from the `maad_q_grid` row with the same `ip_version`.
- `spectrum` holds little-endian 32-bit `(alpha, f)` pairs, and only for the `addresses` measure.
- Unless the dataset sets `maad_internal_side`, there are no rows for internal-side address sets: the `source` side where `src_locality = 'internal'` and the `destination` side where `dst_locality = 'internal'`. `datasets.maad_internal_side` records which applies.

```sql
SELECT
    source_id,
    bucket_start,
    address_side,
    d0,
    d1,
    d2,
    total_addrs
FROM address_maad_stats
WHERE granularity = '1h'
  AND ip_version = 4
  AND src_locality = 'all'
  AND dst_locality = 'all'
  AND measure = 'packets'
ORDER BY source_id, bucket_start, address_side;
```

Decode a curve outside SQL, for example in Python:

```python
import struct

values = struct.unpack(f"<{len(blob) // 4}f", blob)
```

## Query observation averages

```sql
SELECT
    source_id,
    bucket_start,
    average_duration_ms,
    average_min_ttl,
    average_max_ttl
FROM traffic_stats
WHERE granularity = '1h'
  AND ip_version = 4
  AND src_locality = 'all'
  AND dst_locality = 'all'
ORDER BY source_id, bucket_start;
```
