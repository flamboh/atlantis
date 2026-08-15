//! Convert canonical buckets into persistent rows and build safe rollups.

use std::{collections::BTreeMap, net::IpAddr};

use rayon::prelude::*;
use rusqlite::Connection;
use thiserror::Error;

use crate::{
    domain::{
        AddressSetRow, BucketKey, CanonicalBucket, CanonicalRows, DomainError, Granularity,
        IpVersion, StatisticalBucket,
    },
    maad,
    storage::{
        AddressCountStatsRow, AddressStructureStatsRow, PortCountStatsRow, ProtocolStatsRow,
        StatsBucketKey, StatsDimensions, StorageError, TrafficStatsRow, delete_stats_bucket_keys,
        insert_address_count_stats_rows, insert_address_structure_stats_rows,
        insert_port_count_stats_rows, insert_protocol_stats_rows, insert_traffic_stats_rows,
    },
};

#[derive(Debug, Error)]
pub enum PublishError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("unable to serialize MAAD rows: {0}")]
    Json(#[from] serde_json::Error),
    #[error("aggregate bucket lacks complete five-minute coverage: {0:?}")]
    IncompleteCoverage(BucketKey),
}

/// Build all complete 30m, 1h, and local-day aggregates touched by the input.
pub fn build_rollups(
    raw: &[CanonicalBucket],
    day_floor: impl Fn(i64) -> i64,
) -> Result<Vec<CanonicalBucket>, PublishError> {
    let mut builders: BTreeMap<(String, Granularity, i64), StatisticalBucket> = BTreeMap::new();
    for child in raw {
        let day_start = day_floor(child.key.bucket_start);
        // This lands in the following civil day even when the current local day
        // has 23 or 25 hours. Flooring it yields the exact next local midnight.
        let day_end = day_floor(day_start + 36 * 3_600);
        for (granularity, start, end) in [
            (
                Granularity::ThirtyMinutes,
                child.key.bucket_start.div_euclid(1_800) * 1_800,
                child.key.bucket_start.div_euclid(1_800) * 1_800 + 1_800,
            ),
            (
                Granularity::OneHour,
                child.key.bucket_start.div_euclid(3_600) * 3_600,
                child.key.bucket_start.div_euclid(3_600) * 3_600 + 3_600,
            ),
            (Granularity::OneDay, day_start, day_end),
        ] {
            let key = (child.key.source_id.clone(), granularity, start);
            let builder = builders.entry(key.clone()).or_insert_with(|| {
                StatisticalBucket::dense(BucketKey::new(key.0.clone(), granularity, start, end))
            });
            builder.include(child)?;
        }
    }
    Ok(builders
        .into_values()
        .map(|builder| builder.finish())
        .filter(CanonicalBucket::has_complete_five_minute_coverage)
        .collect())
}

/// Replace all row families for these bucket keys as one caller-owned transaction.
pub fn write_buckets(
    connection: &Connection,
    buckets: &[CanonicalBucket],
    run_maad: bool,
) -> Result<(), PublishError> {
    let keys = buckets
        .iter()
        .map(|bucket| {
            StatsBucketKey::new(
                &bucket.key.source_id,
                bucket.key.granularity.as_str(),
                bucket.key.bucket_start,
            )
        })
        .collect::<Vec<_>>();
    delete_stats_bucket_keys(connection, &keys)?;
    for bucket in buckets {
        let rows = bucket.rows();
        insert_rows(connection, &rows)?;
        if run_maad {
            insert_address_structure_stats_rows(connection, &maad_rows(&rows.address_sets)?)?;
        }
    }
    Ok(())
}

fn insert_rows(connection: &Connection, rows: &CanonicalRows) -> Result<(), PublishError> {
    let traffic = rows
        .traffic_rows
        .iter()
        .map(|row| {
            let metrics = &row.metrics;
            TrafficStatsRow {
                dimensions: dimensions(&row.key, row.scope),
                flows: metrics.flows,
                flows_tcp: metrics.flows_tcp,
                flows_udp: metrics.flows_udp,
                flows_icmp: metrics.flows_icmp,
                flows_other: metrics.flows_other,
                packets: metrics.packets,
                packets_tcp: metrics.packets_tcp,
                packets_udp: metrics.packets_udp,
                packets_icmp: metrics.packets_icmp,
                packets_other: metrics.packets_other,
                bytes: metrics.bytes,
                bytes_tcp: metrics.bytes_tcp,
                bytes_udp: metrics.bytes_udp,
                bytes_icmp: metrics.bytes_icmp,
                bytes_other: metrics.bytes_other,
                duration_sum_ms: metrics.duration_sum_ms,
                duration_count: metrics.duration_count,
                average_duration_ms: row.average_duration_ms,
                min_ttl_sum: metrics.min_ttl_sum,
                min_ttl_count: metrics.min_ttl_count,
                average_min_ttl: row.average_min_ttl,
                max_ttl_sum: metrics.max_ttl_sum,
                max_ttl_count: metrics.max_ttl_count,
                average_max_ttl: row.average_max_ttl,
            }
        })
        .collect::<Vec<_>>();
    let protocols = rows
        .protocol_rows
        .iter()
        .map(|row| ProtocolStatsRow {
            dimensions: dimensions(&row.key, row.scope),
            unique_protocols_count: i64::try_from(row.unique_protocols_count).unwrap_or(i64::MAX),
            protocols_list: row.protocols_list.clone(),
        })
        .collect::<Vec<_>>();
    let addresses = rows
        .address_count_rows
        .iter()
        .map(|row| AddressCountStatsRow {
            dimensions: dimensions(&row.key, row.scope),
            address_side: row.address_side.as_str().to_owned(),
            unique_address_count: i64::try_from(row.unique_address_count).unwrap_or(i64::MAX),
        })
        .collect::<Vec<_>>();
    let ports = rows
        .port_count_rows
        .iter()
        .map(|row| PortCountStatsRow {
            dimensions: dimensions(&row.key, row.scope),
            port_side: row.port_side.as_str().to_owned(),
            port_range: row.port_range.as_str().to_owned(),
            unique_port_count: i64::try_from(row.unique_port_count).unwrap_or(i64::MAX),
        })
        .collect::<Vec<_>>();
    insert_traffic_stats_rows(connection, &traffic)?;
    insert_protocol_stats_rows(connection, &protocols)?;
    insert_address_count_stats_rows(connection, &addresses)?;
    insert_port_count_stats_rows(connection, &ports)?;
    Ok(())
}

fn maad_rows(
    address_sets: &[AddressSetRow],
) -> Result<Vec<AddressStructureStatsRow>, PublishError> {
    Ok(address_sets
        .par_iter()
        .filter(|addresses| addresses.scope.ip_version == IpVersion::V4)
        .map(|addresses| {
            let result = maad::compute(addresses.addresses.iter().filter_map(
                |address| match address {
                    IpAddr::V4(address) => Some(*address),
                    IpAddr::V6(_) => None,
                },
            ));
            let metadata_json = serde_json::to_string(&result.metadata)?;
            let dimensions = dimensions(&addresses.key, addresses.scope);
            Ok::<_, serde_json::Error>([
                AddressStructureStatsRow {
                    dimensions: dimensions.clone(),
                    address_side: addresses.address_side.as_str().to_owned(),
                    structure_kind: "structure".into(),
                    values_json: serde_json::to_string(&result.structure)?,
                    metadata_json: metadata_json.clone(),
                },
                AddressStructureStatsRow {
                    dimensions: dimensions.clone(),
                    address_side: addresses.address_side.as_str().to_owned(),
                    structure_kind: "spectrum".into(),
                    values_json: serde_json::to_string(&result.spectrum)?,
                    metadata_json: metadata_json.clone(),
                },
                AddressStructureStatsRow {
                    dimensions,
                    address_side: addresses.address_side.as_str().to_owned(),
                    structure_kind: "dimension".into(),
                    values_json: serde_json::to_string(&result.dimensions)?,
                    metadata_json,
                },
            ])
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect())
}

fn dimensions(key: &BucketKey, scope: crate::domain::Scope) -> StatsDimensions {
    StatsDimensions {
        source_id: key.source_id.clone(),
        granularity: key.granularity.as_str().to_owned(),
        bucket_start: key.bucket_start,
        bucket_end: key.bucket_end,
        ip_version: i64::from(scope.ip_version.number()),
        src_visibility: scope.src_visibility.as_str().to_owned(),
        dst_visibility: scope.dst_visibility.as_str().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use rusqlite::Connection;

    use super::*;
    use crate::{domain::FlowObservation, storage::init_stats_tables};

    #[test]
    fn canonical_bucket_persists_without_raw_addresses() {
        let connection = Connection::open_in_memory().unwrap();
        init_stats_tables(&connection).unwrap();
        let mut builder =
            StatisticalBucket::dense(BucketKey::new("r1", Granularity::FiveMinutes, 0, 300));
        builder
            .add(
                FlowObservation::new(
                    IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
                    IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)),
                    6,
                    2,
                    128,
                    0,
                )
                .unwrap(),
            )
            .unwrap();

        write_buckets(&connection, &[builder.finish()], true).unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT flows FROM traffic_stats WHERE ip_version = 4 AND src_visibility = 'all' AND dst_visibility = 'all'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM address_structure_stats", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            30
        );
    }

    #[test]
    fn rollups_skip_incomplete_larger_windows() {
        let raw = (0..6)
            .map(|index| {
                StatisticalBucket::dense(BucketKey::new(
                    "r1",
                    Granularity::FiveMinutes,
                    index * 300,
                    (index + 1) * 300,
                ))
                .finish()
            })
            .collect::<Vec<_>>();

        let rollups = build_rollups(&raw, |_| 0).unwrap();

        assert_eq!(rollups.len(), 1);
        assert_eq!(rollups[0].key.granularity, Granularity::ThirtyMinutes);
    }

    #[test]
    fn daily_rollup_uses_the_next_local_midnight_on_short_days() {
        let raw = (0..276)
            .map(|index| {
                StatisticalBucket::dense(BucketKey::new(
                    "r1",
                    Granularity::FiveMinutes,
                    index * 300,
                    (index + 1) * 300,
                ))
                .finish()
            })
            .collect::<Vec<_>>();
        let day_floor = |timestamp| if timestamp < 82_800 { 0 } else { 82_800 };

        let rollups = build_rollups(&raw, day_floor).unwrap();
        let day = rollups
            .iter()
            .find(|bucket| bucket.key.granularity == Granularity::OneDay)
            .unwrap();

        assert_eq!(day.key.bucket_end, 82_800);
        assert!(day.has_complete_five_minute_coverage());
    }
}
