# Query a database

Each dataset uses one SQLite database at `data/<dataset-id>/netflow.sqlite`. A dataset that sets `db_path` uses that location instead.

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

| Table                     | Content                                                    |
| ------------------------- | ---------------------------------------------------------- |
| `datasets`                | Public dataset metadata                                    |
| `source_members`          | Logical-source membership                                  |
| `traffic_stats`           | Flow, packet, byte, duration, and TTL metrics              |
| `protocol_stats`          | Unique protocol counts and protocol lists                  |
| `address_count_stats`     | Unique source-address and destination-address counts       |
| `port_count_stats`        | Unique low-port and high-port counts                       |
| `address_structure_stats` | MAAD structure, spectrum, and dimension values per measure |
| `processed_inputs`        | Input processing state and provenance                      |

The `granularity` value is `5m`, `10m`, `30m`, `1h`, or `1d`.

Each stats table stores an endpoint locality pair per bucket: `src_locality` and `dst_locality`, each `all`, `internal`, or `external`. Only five pairs are ever stored — mixed pairs with `all` on only one side never occur:

| Direction | `src_locality` | `dst_locality` | Meaning                                                              |
| --------- | -------------- | -------------- | -------------------------------------------------------------------- |
| `all`     | `all`          | `all`          | All traffic                                                          |
| `ingress` | `external`     | `internal`     | external → internal                                                  |
| `egress`  | `internal`     | `external`     | internal → external                                                  |
| `lateral` | `internal`     | `internal`     | internal → internal                                                  |
| `transit` | `external`     | `external`     | external → external (kept for visibility into misclassified records) |

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

`address_structure_stats` stores one MAAD analysis per bucket, IP version, locality pair, address
side, and `measure`:

| `measure`   | Weights each address by                  | Stored `structure_kind` rows         |
| ----------- | ---------------------------------------- | ------------------------------------ |
| `addresses` | 1 (distinct addresses)                   | `structure`, `spectrum`, `dimension` |
| `packets`   | Packets summed over the bucket and scope | `structure`, `dimension`             |
| `bytes`     | Bytes summed over the bucket and scope   | `structure`, `dimension`             |

Weighted measures keep the address-count prefix tests and weight only the moments and the entropy.
Addresses with zero summed packets or bytes are left out of that measure. Their count is stored in
`metadata_json` as `zeroWeightAddrs`. Always filter on `measure`. Otherwise results for different
measures mix:

```sql
SELECT
    source_id,
    bucket_start,
    address_side,
    values_json
FROM address_structure_stats
WHERE granularity = '1h'
  AND ip_version = 4
  AND src_locality = 'all'
  AND dst_locality = 'all'
  AND measure = 'packets'
  AND structure_kind = 'dimension'
ORDER BY source_id, bucket_start, address_side;
```

## Query observation averages

The database stores sums and counts for safe rollup calculations. It also stores calculated averages for direct queries.

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
