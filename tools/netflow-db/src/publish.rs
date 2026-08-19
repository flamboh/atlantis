//! Convert canonical buckets into persistent rows and build safe rollups.

use std::{
    collections::BTreeMap,
    net::IpAddr,
    time::{Duration, Instant},
};

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
        AddressCountStatsRow, AddressStructureStatsRow, BucketCoverageRow, PortCountStatsRow,
        ProtocolStatsRow, StatsBucketKey, StatsDimensions, StorageError, TrafficStatsRow,
        delete_stats_bucket_keys, insert_address_count_stats_rows,
        insert_address_structure_stats_rows, insert_bucket_coverage_rows,
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

/// Aggregate timings and work counts for one or more `write_buckets` calls.
///
/// Timers wrap batch boundaries rather than individual rows so profiling remains
/// cheap enough to keep enabled during a full-day pipeline run.
#[derive(Clone, Debug, Default)]
pub struct WriteBucketsProfile {
    pub(crate) total_elapsed: Duration,
    pub(crate) delete_elapsed: Duration,
    pub(crate) canonical_rows_elapsed: Duration,
    pub(crate) scalar_rows_elapsed: Duration,
    pub(crate) traffic_insert_elapsed: Duration,
    pub(crate) protocol_insert_elapsed: Duration,
    pub(crate) address_count_insert_elapsed: Duration,
    pub(crate) port_count_insert_elapsed: Duration,
    pub(crate) maad_elapsed: Duration,
    pub(crate) address_structure_insert_elapsed: Duration,
    pub(crate) write_calls: u64,
    pub(crate) bucket_keys: u64,
    pub(crate) traffic_rows: u64,
    pub(crate) protocol_rows: u64,
    pub(crate) address_count_rows: u64,
    pub(crate) port_count_rows: u64,
    pub(crate) maad_address_sets: u64,
    pub(crate) maad_addresses: u64,
    pub(crate) address_structure_rows: u64,
    pub(crate) address_structure_json_bytes: u64,
}

impl WriteBucketsProfile {
    pub(crate) fn include(&mut self, profile: Self) {
        self.total_elapsed += profile.total_elapsed;
        self.delete_elapsed += profile.delete_elapsed;
        self.canonical_rows_elapsed += profile.canonical_rows_elapsed;
        self.scalar_rows_elapsed += profile.scalar_rows_elapsed;
        self.traffic_insert_elapsed += profile.traffic_insert_elapsed;
        self.protocol_insert_elapsed += profile.protocol_insert_elapsed;
        self.address_count_insert_elapsed += profile.address_count_insert_elapsed;
        self.port_count_insert_elapsed += profile.port_count_insert_elapsed;
        self.maad_elapsed += profile.maad_elapsed;
        self.address_structure_insert_elapsed += profile.address_structure_insert_elapsed;
        self.write_calls += profile.write_calls;
        self.bucket_keys += profile.bucket_keys;
        self.traffic_rows += profile.traffic_rows;
        self.protocol_rows += profile.protocol_rows;
        self.address_count_rows += profile.address_count_rows;
        self.port_count_rows += profile.port_count_rows;
        self.maad_address_sets += profile.maad_address_sets;
        self.maad_addresses += profile.maad_addresses;
        self.address_structure_rows += profile.address_structure_rows;
        self.address_structure_json_bytes += profile.address_structure_json_bytes;
    }

    pub(crate) fn other_elapsed(&self) -> Duration {
        self.total_elapsed.saturating_sub(
            self.delete_elapsed
                + self.canonical_rows_elapsed
                + self.scalar_rows_elapsed
                + self.traffic_insert_elapsed
                + self.protocol_insert_elapsed
                + self.address_count_insert_elapsed
                + self.port_count_insert_elapsed
                + self.maad_elapsed
                + self.address_structure_insert_elapsed,
        )
    }
}

#[derive(Debug, Default)]
struct ScalarRowsProfile {
    materialize_elapsed: Duration,
    traffic_insert_elapsed: Duration,
    protocol_insert_elapsed: Duration,
    address_count_insert_elapsed: Duration,
    port_count_insert_elapsed: Duration,
}

/// Build every 30m, 1h, and local-day aggregate touched by the input envelope.
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
                StatisticalBucket::new(BucketKey::new(key.0.clone(), granularity, start, end))
            });
            builder.include(child)?;
        }
    }
    Ok(builders
        .into_values()
        .map(|builder| builder.finish())
        .collect())
}

/// Replace all row families for these bucket keys as one caller-owned transaction.
pub fn write_buckets(
    connection: &Connection,
    buckets: &[CanonicalBucket],
    run_maad: bool,
) -> Result<(), PublishError> {
    write_buckets_profiled(connection, buckets, run_maad).map(|_| ())
}

pub(crate) fn write_buckets_profiled(
    connection: &Connection,
    buckets: &[CanonicalBucket],
    run_maad: bool,
) -> Result<WriteBucketsProfile, PublishError> {
    let total_started = Instant::now();
    let mut profile = WriteBucketsProfile {
        write_calls: 1,
        bucket_keys: count(buckets.len()),
        ..WriteBucketsProfile::default()
    };
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
    let delete_started = Instant::now();
    delete_stats_bucket_keys(connection, &keys)?;
    profile.delete_elapsed += delete_started.elapsed();
    let coverage_rows = buckets
        .iter()
        .map(|bucket| {
            BucketCoverageRow::new(
                &bucket.key.source_id,
                bucket.key.granularity.as_str(),
                bucket.key.bucket_start,
                bucket.key.bucket_end,
                bucket.coverage,
            )
        })
        .collect::<Vec<_>>();
    insert_bucket_coverage_rows(connection, &coverage_rows)?;
    for bucket in buckets {
        let canonical_rows_started = Instant::now();
        let rows = bucket.rows();
        profile.canonical_rows_elapsed += canonical_rows_started.elapsed();
        profile.traffic_rows += count(rows.traffic_rows.len());
        profile.protocol_rows += count(rows.protocol_rows.len());
        profile.address_count_rows += count(rows.address_count_rows.len());
        profile.port_count_rows += count(rows.port_count_rows.len());

        let scalar = insert_rows(connection, &rows)?;
        profile.scalar_rows_elapsed += scalar.materialize_elapsed;
        profile.traffic_insert_elapsed += scalar.traffic_insert_elapsed;
        profile.protocol_insert_elapsed += scalar.protocol_insert_elapsed;
        profile.address_count_insert_elapsed += scalar.address_count_insert_elapsed;
        profile.port_count_insert_elapsed += scalar.port_count_insert_elapsed;
        if run_maad {
            profile.maad_address_sets += count(
                rows.address_sets
                    .iter()
                    .filter(|addresses| addresses.scope.ip_version == IpVersion::V4)
                    .count(),
            );
            profile.maad_addresses += rows
                .address_sets
                .iter()
                .filter(|addresses| addresses.scope.ip_version == IpVersion::V4)
                .map(|addresses| count(addresses.addresses.len()))
                .sum::<u64>();
            let maad_started = Instant::now();
            let address_structure = maad_rows(&rows.address_sets)?;
            profile.maad_elapsed += maad_started.elapsed();
            profile.address_structure_rows += count(address_structure.len());
            profile.address_structure_json_bytes += address_structure
                .iter()
                .map(|row| count(row.values_json.len() + row.metadata_json.len()))
                .sum::<u64>();
            let insert_started = Instant::now();
            insert_address_structure_stats_rows(connection, &address_structure)?;
            profile.address_structure_insert_elapsed += insert_started.elapsed();
        }
    }
    profile.total_elapsed = total_started.elapsed();
    Ok(profile)
}

fn insert_rows(
    connection: &Connection,
    rows: &CanonicalRows<'_>,
) -> Result<ScalarRowsProfile, PublishError> {
    let materialize_started = Instant::now();
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
    let materialize_elapsed = materialize_started.elapsed();

    let traffic_insert_started = Instant::now();
    insert_traffic_stats_rows(connection, &traffic)?;
    let traffic_insert_elapsed = traffic_insert_started.elapsed();
    let protocol_insert_started = Instant::now();
    insert_protocol_stats_rows(connection, &protocols)?;
    let protocol_insert_elapsed = protocol_insert_started.elapsed();
    let address_count_insert_started = Instant::now();
    insert_address_count_stats_rows(connection, &addresses)?;
    let address_count_insert_elapsed = address_count_insert_started.elapsed();
    let port_count_insert_started = Instant::now();
    insert_port_count_stats_rows(connection, &ports)?;
    let port_count_insert_elapsed = port_count_insert_started.elapsed();
    Ok(ScalarRowsProfile {
        materialize_elapsed,
        traffic_insert_elapsed,
        protocol_insert_elapsed,
        address_count_insert_elapsed,
        port_count_insert_elapsed,
    })
}

fn maad_rows(
    address_sets: &[AddressSetRow<'_>],
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

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use rusqlite::Connection;

    use super::*;
    use crate::{
        coverage::{BucketCoverage, CoverageState},
        domain::{AddressSide, FlowObservation, IpVersion, Scope, ScopedAddressesFact, Visibility},
        storage::init_stats_tables,
    };

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

        let profile = write_buckets_profiled(&connection, &[builder.finish()], true).unwrap();

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
        assert_eq!(profile.bucket_keys, 1);
        assert_eq!(profile.write_calls, 1);
        assert_eq!(profile.traffic_rows, 10);
        assert_eq!(profile.protocol_rows, 10);
        assert_eq!(profile.address_count_rows, 20);
        assert_eq!(profile.port_count_rows, 40);
        assert_eq!(profile.maad_address_sets, 10);
        assert_eq!(profile.address_structure_rows, 30);
        assert!(profile.address_structure_json_bytes > 0);
        assert!(profile.total_elapsed >= profile.other_elapsed());
    }

    #[test]
    fn address_insertion_order_does_not_change_persisted_products() {
        fn persist(addresses: impl IntoIterator<Item = IpAddr>) -> Connection {
            let connection = Connection::open_in_memory().unwrap();
            init_stats_tables(&connection).unwrap();
            let mut builder =
                StatisticalBucket::dense(BucketKey::new("r1", Granularity::FiveMinutes, 0, 300));
            builder
                .add(ScopedAddressesFact::new(
                    Scope::new(IpVersion::V4, Visibility::All, Visibility::All),
                    AddressSide::Source,
                    addresses,
                ))
                .unwrap();
            write_buckets(&connection, &[builder.finish()], true).unwrap();
            connection
        }

        fn product_rows(connection: &Connection) -> (Vec<String>, Vec<String>) {
            fn query(connection: &Connection, sql: &str) -> Vec<String> {
                connection
                    .prepare(sql)
                    .unwrap()
                    .query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap()
            }

            (
                query(
                    connection,
                    "SELECT printf('%s|%s|%s|%s|%d', ip_version, src_visibility, dst_visibility, address_side, unique_address_count) FROM address_count_stats ORDER BY ip_version, src_visibility, dst_visibility, address_side",
                ),
                query(
                    connection,
                    "SELECT printf('%s|%s|%s|%s|%s|%s|%s', ip_version, src_visibility, dst_visibility, address_side, structure_kind, values_json, metadata_json) FROM address_structure_stats ORDER BY ip_version, src_visibility, dst_visibility, address_side, structure_kind",
                ),
            )
        }

        let first = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let second = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));
        let third = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 3));
        let forward = persist([first, second, first, third]);
        let reverse = persist([third, first, second, second]);

        assert_eq!(product_rows(&forward), product_rows(&reverse));
    }

    #[test]
    fn rollups_keep_touched_edges_without_extending_the_input_envelope() {
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

        assert_eq!(rollups.len(), 3);
        for rollup in rollups {
            assert_eq!(rollup.coverage, BucketCoverage::new(6, 6, 0).unwrap());
        }
    }

    #[test]
    fn rollups_publish_structurally_complete_windows_with_partial_coverage() {
        let raw = (0..6)
            .map(|index| {
                let key = BucketKey::new(
                    "r1",
                    Granularity::FiveMinutes,
                    index * 300,
                    (index + 1) * 300,
                );
                if index == 2 {
                    StatisticalBucket::new(key)
                        .with_coverage(BucketCoverage::new(1, 0, 0).unwrap())
                        .finish()
                } else {
                    StatisticalBucket::dense(key).finish()
                }
            })
            .collect::<Vec<_>>();

        let rollups = build_rollups(&raw, |_| 0).unwrap();
        let thirty_minutes = rollups
            .iter()
            .find(|bucket| bucket.key.granularity == Granularity::ThirtyMinutes)
            .unwrap();

        assert_eq!(thirty_minutes.coverage.state(), CoverageState::Partial);
        assert_eq!(thirty_minutes.coverage.expected_units(), 6);
        assert_eq!(thirty_minutes.coverage.observed_units(), 5);
    }

    #[test]
    fn all_unknown_rollup_has_coverage_without_synthetic_metrics() {
        let raw = (0..6)
            .map(|index| {
                StatisticalBucket::new(BucketKey::new(
                    "r1",
                    Granularity::FiveMinutes,
                    index * 300,
                    (index + 1) * 300,
                ))
                .with_coverage(BucketCoverage::new(1, 0, 0).unwrap())
                .finish()
            })
            .collect::<Vec<_>>();

        let rollups = build_rollups(&raw, |_| 0).unwrap();
        let thirty_minutes = rollups
            .iter()
            .find(|bucket| bucket.key.granularity == Granularity::ThirtyMinutes)
            .unwrap();

        assert_eq!(thirty_minutes.coverage.state(), CoverageState::Unknown);
        assert!(thirty_minutes.traffic.is_empty());
        assert!(thirty_minutes.protocols.is_empty());
        assert!(thirty_minutes.addresses.is_empty());
        assert!(thirty_minutes.ports.is_empty());
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
