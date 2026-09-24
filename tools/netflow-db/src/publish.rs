//! Convert canonical buckets into persistent rows.

use std::{
    net::IpAddr,
    sync::OnceLock,
    time::{Duration, Instant},
};

use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;
use thiserror::Error;

use crate::{
    domain::{
        AddressSetRow, AddressTraffic, BucketKey, CanonicalBucket, CanonicalRows, DomainError,
        IpVersion, MaadMeasure,
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
    #[error("unable to build MAAD worker pool: {0}")]
    MaadPool(String),
    #[error(transparent)]
    Maad(#[from] maad::MaadError),
    #[error("aggregate bucket lacks complete five-minute coverage: {0:?}")]
    IncompleteCoverage(BucketKey),
}

const MAAD_WORKERS: usize = 2;
static MAAD_POOL: OnceLock<Result<rayon::ThreadPool, String>> = OnceLock::new();

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
    #[cfg(test)]
    pub(crate) write_calls: u64,
    #[cfg(test)]
    pub(crate) bucket_keys: u64,
    pub(crate) traffic_rows: u64,
    pub(crate) protocol_rows: u64,
    pub(crate) address_count_rows: u64,
    pub(crate) port_count_rows: u64,
    pub(crate) maad_address_sets: u64,
    pub(crate) maad_addresses: u64,
    pub(crate) maad_zero_weight_addresses: u64,
    pub(crate) address_structure_rows: u64,
    pub(crate) address_structure_json_bytes: u64,
}

impl WriteBucketsProfile {
    #[cfg(test)]
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
        #[cfg(test)]
        write_calls: 1,
        #[cfg(test)]
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
            profile.maad_address_sets += count(rows.address_sets.len());
            profile.maad_addresses += rows
                .address_sets
                .iter()
                .map(|addresses| count(addresses.addresses.len()))
                .sum::<u64>();
            let maad_started = Instant::now();
            let (address_structure, zero_weight_addresses) = maad_rows(&rows.address_sets)?;
            profile.maad_elapsed += maad_started.elapsed();
            profile.maad_zero_weight_addresses += zero_weight_addresses;
            if zero_weight_addresses > 0 {
                tracing::info!(
                    source_id = %bucket.key.source_id,
                    granularity = bucket.key.granularity.as_str(),
                    bucket_start = bucket.key.bucket_start,
                    zero_weight_addresses,
                    "excluded zero-weight addresses from weighted MAAD measures"
                );
            }
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

/// Compute every MAAD measure for each scoped address set.
///
/// Returns the rows and the number of addresses excluded from weighted measures
/// because their summed weight was zero.
fn maad_rows(
    address_sets: &[AddressSetRow<'_>],
) -> Result<(Vec<AddressStructureStatsRow>, u64), PublishError> {
    if address_sets.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let work = address_sets
        .iter()
        .flat_map(|addresses| MaadMeasure::ALL.map(|measure| (addresses, measure)))
        .collect::<Vec<_>>();
    let pool = maad_pool()?;
    let measured = pool.install(|| {
        work.par_iter()
            .map(|&(addresses, measure)| measure_rows(addresses, measure))
            .collect::<Result<Vec<_>, _>>()
    })?;
    let zero_weight_addresses = measured.iter().map(|(_, excluded)| count(*excluded)).sum();
    Ok((
        measured.into_iter().flat_map(|(rows, _)| rows).collect(),
        zero_weight_addresses,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WeightedMetadata<'a> {
    #[serde(flatten)]
    metadata: &'a maad::MaadMetadata,
    zero_weight_addrs: usize,
}

fn measure_rows(
    addresses: &AddressSetRow<'_>,
    measure: MaadMeasure,
) -> Result<(Vec<AddressStructureStatsRow>, usize), PublishError> {
    let dimensions = dimensions(&addresses.key, addresses.scope);
    let row =
        |structure_kind: &str, values_json: String, metadata_json: &str| AddressStructureStatsRow {
            dimensions: dimensions.clone(),
            address_side: addresses.address_side.as_str().to_owned(),
            measure: measure.as_str().to_owned(),
            structure_kind: structure_kind.to_owned(),
            values_json,
            metadata_json: metadata_json.to_owned(),
        };
    let weight: fn(AddressTraffic) -> u64 = match measure {
        MaadMeasure::Addresses => {
            let result = address_maad_result(addresses);
            let metadata_json = serde_json::to_string(&result.metadata)?;
            return Ok((
                vec![
                    row(
                        "structure",
                        serde_json::to_string(&result.structure)?,
                        &metadata_json,
                    ),
                    row(
                        "spectrum",
                        serde_json::to_string(&result.spectrum)?,
                        &metadata_json,
                    ),
                    row(
                        "dimension",
                        serde_json::to_string(&result.dimensions)?,
                        &metadata_json,
                    ),
                ],
                0,
            ));
        }
        MaadMeasure::Packets => |traffic| traffic.packets,
        MaadMeasure::Bytes => |traffic| traffic.bytes,
    };
    let (result, zero_weight_addrs) = weighted_maad_result(addresses, weight)?;
    let metadata_json = serde_json::to_string(&WeightedMetadata {
        metadata: &result.metadata,
        zero_weight_addrs,
    })?;
    Ok((
        vec![
            row(
                "structure",
                serde_json::to_string(&result.structure)?,
                &metadata_json,
            ),
            row(
                "dimension",
                serde_json::to_string(&result.dimensions)?,
                &metadata_json,
            ),
        ],
        zero_weight_addrs,
    ))
}

fn address_maad_result(addresses: &AddressSetRow<'_>) -> maad::MaadResult {
    let family = addresses.addresses.iter().map(|(address, _)| address);
    match addresses.scope.ip_version {
        IpVersion::V4 => maad::compute(family.filter_map(|address| match address {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        })),
        IpVersion::V6 => maad::compute(family.filter_map(|address| match address {
            IpAddr::V6(address) => Some(address),
            IpAddr::V4(_) => None,
        })),
    }
}

/// Weighted MAAD over addresses with a positive summed weight, and the count of
/// same-family addresses excluded because their weight was zero.
fn weighted_maad_result(
    addresses: &AddressSetRow<'_>,
    weight: fn(AddressTraffic) -> u64,
) -> Result<(maad::MaadResult, usize), maad::MaadError> {
    let mut zero_weight_addrs = 0;
    let mut positive = |traffic| match weight(traffic) {
        0 => {
            zero_weight_addrs += 1;
            None
        }
        value => Some(value as f64),
    };
    let entries = addresses.addresses.iter();
    let result = match addresses.scope.ip_version {
        IpVersion::V4 => {
            maad::compute_weighted(entries.filter_map(|(address, traffic)| match address {
                IpAddr::V4(address) => positive(traffic).map(|value| (address, value)),
                IpAddr::V6(_) => None,
            }))?
        }
        IpVersion::V6 => {
            maad::compute_weighted(entries.filter_map(|(address, traffic)| match address {
                IpAddr::V6(address) => positive(traffic).map(|value| (address, value)),
                IpAddr::V4(_) => None,
            }))?
        }
    };
    Ok((result, zero_weight_addrs))
}

fn maad_pool() -> Result<&'static rayon::ThreadPool, PublishError> {
    match MAAD_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(MAAD_WORKERS)
            .thread_name(|index| format!("maad-{index}"))
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(pool) => Ok(pool),
        Err(error) => Err(PublishError::MaadPool(error.clone())),
    }
}

fn dimensions(key: &BucketKey, scope: crate::domain::Scope) -> StatsDimensions {
    StatsDimensions {
        source_id: key.source_id.clone(),
        granularity: key.granularity.as_str().to_owned(),
        bucket_start: key.bucket_start,
        bucket_end: key.bucket_end,
        ip_version: i64::from(scope.ip_version.number()),
        src_locality: scope.src_locality.as_str().to_owned(),
        dst_locality: scope.dst_locality.as_str().to_owned(),
    }
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use rusqlite::Connection;

    use super::*;
    use crate::{
        domain::{
            AddressSide, AddressTotals, EndpointLocality, FlowObservation, Granularity, IpVersion,
            Locality, Scope, ScopedAddressesFact, StatisticalBucket,
        },
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
                .unwrap()
                .with_locality(EndpointLocality::Internal, EndpointLocality::External),
            )
            .unwrap();

        let profile = write_buckets_profiled(&connection, &[builder.finish()], true).unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT flows FROM traffic_stats WHERE ip_version = 4 AND src_locality = 'all' AND dst_locality = 'all'",
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
            140
        );
        assert_eq!(profile.bucket_keys, 1);
        assert_eq!(profile.write_calls, 1);
        assert_eq!(profile.traffic_rows, 10);
        assert_eq!(profile.protocol_rows, 10);
        assert_eq!(profile.address_count_rows, 20);
        assert_eq!(profile.port_count_rows, 40);
        assert_eq!(profile.maad_address_sets, 20);
        assert_eq!(profile.address_structure_rows, 140);
        assert_eq!(profile.maad_zero_weight_addresses, 0);
        assert!(profile.address_structure_json_bytes > 0);
        assert!(profile.total_elapsed >= profile.other_elapsed());
    }

    #[test]
    fn address_insertion_order_does_not_change_persisted_products() {
        fn persist(addresses: impl IntoIterator<Item = (IpAddr, AddressTraffic)>) -> Connection {
            let connection = Connection::open_in_memory().unwrap();
            init_stats_tables(&connection).unwrap();
            let mut builder =
                StatisticalBucket::dense(BucketKey::new("r1", Granularity::FiveMinutes, 0, 300));
            builder
                .add(ScopedAddressesFact::new(
                    Scope::new(IpVersion::V4, Locality::All, Locality::All),
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
                    "SELECT printf('%s|%s|%s|%s|%d', ip_version, src_locality, dst_locality, address_side, unique_address_count) FROM address_count_stats ORDER BY ip_version, src_locality, dst_locality, address_side",
                ),
                query(
                    connection,
                    "SELECT printf('%s|%s|%s|%s|%s|%s|%s|%s', ip_version, src_locality, dst_locality, address_side, measure, structure_kind, values_json, metadata_json) FROM address_structure_stats ORDER BY ip_version, src_locality, dst_locality, address_side, measure, structure_kind",
                ),
            )
        }

        let first = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let second = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));
        let third = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 3));
        let traffic = AddressTraffic::new;
        let forward = persist([
            (first, traffic(1, 10)),
            (second, traffic(2, 200)),
            (first, traffic(3, 30)),
            (third, traffic(4, 4_000)),
        ]);
        let reverse = persist([
            (third, traffic(4, 4_000)),
            (first, traffic(4, 40)),
            (second, traffic(1, 100)),
            (second, traffic(1, 100)),
        ]);

        assert_eq!(product_rows(&forward), product_rows(&reverse));
    }

    #[test]
    fn maad_rows_preserve_scope_order_and_bytes() {
        let key = BucketKey::new("r1", Granularity::FiveMinutes, 0, 300);
        let first_addresses = (0..64_u8)
            .map(|index| {
                (
                    IpAddr::V4(Ipv4Addr::new(192, index % 4, index, 1)),
                    AddressTraffic::new(u64::from(index % 5 + 1).pow(3), 100),
                )
            })
            .collect::<AddressTotals>();
        let second_addresses = (0..64_u8)
            .map(|index| {
                (
                    IpAddr::V4(Ipv4Addr::new(198, index % 4, index, 1)),
                    AddressTraffic::new(5, 5 * u64::from(index) + 5),
                )
            })
            .collect::<AddressTotals>();
        let rows = [
            AddressSetRow {
                key: key.clone(),
                scope: Scope::new(IpVersion::V4, Locality::All, Locality::All),
                address_side: AddressSide::Source,
                addresses: &first_addresses,
            },
            AddressSetRow {
                key,
                scope: Scope::new(IpVersion::V4, Locality::External, Locality::All),
                address_side: AddressSide::Destination,
                addresses: &second_addresses,
            },
        ];

        let (first, first_excluded) = maad_rows(&rows).unwrap();
        let (second, second_excluded) = maad_rows(&rows).unwrap();

        assert_eq!(first, second);
        assert_eq!((first_excluded, second_excluded), (0, 0));
        let kinds = |src_locality: &str, side: &str| {
            [
                ("addresses", "structure"),
                ("addresses", "spectrum"),
                ("addresses", "dimension"),
                ("packets", "structure"),
                ("packets", "dimension"),
                ("bytes", "structure"),
                ("bytes", "dimension"),
            ]
            .map(|(measure, kind)| {
                (
                    src_locality.to_owned(),
                    side.to_owned(),
                    measure.to_owned(),
                    kind.to_owned(),
                )
            })
        };
        assert_eq!(
            first
                .iter()
                .map(|row| (
                    row.dimensions.src_locality.clone(),
                    row.address_side.clone(),
                    row.measure.clone(),
                    row.structure_kind.clone(),
                ))
                .collect::<Vec<_>>(),
            [kinds("all", "source"), kinds("external", "destination")].concat()
        );
        let packets_structure = |scope_rows: &[AddressStructureStatsRow]| {
            scope_rows
                .iter()
                .find(|row| row.measure == "packets" && row.structure_kind == "structure")
                .unwrap()
                .values_json
                .clone()
        };
        let addresses_structure = |scope_rows: &[AddressStructureStatsRow]| {
            scope_rows
                .iter()
                .find(|row| row.measure == "addresses" && row.structure_kind == "structure")
                .unwrap()
                .values_json
                .clone()
        };
        assert_ne!(
            packets_structure(&first[..7]),
            addresses_structure(&first[..7])
        );
        let tau = |json: String| -> Vec<f64> {
            serde_json::from_str::<Vec<serde_json::Value>>(&json)
                .unwrap()
                .iter()
                .map(|row| row["tauTilde"].as_f64().unwrap())
                .collect()
        };
        let uniform_packets = tau(packets_structure(&first[7..]));
        let addresses = tau(addresses_structure(&first[7..]));
        assert_eq!(uniform_packets.len(), addresses.len());
        for (packets, addresses) in uniform_packets.iter().zip(&addresses) {
            assert!((packets - addresses).abs() < 1e-12);
        }
    }

    #[test]
    fn maad_rows_use_the_ipv6_prefix_range_for_ipv6_scopes() {
        let base = u128::from(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0));
        let addresses = (0..=255_u128)
            .map(|subnet| {
                (
                    IpAddr::V6(Ipv6Addr::from(base | (subnet << 64))),
                    AddressTraffic::new(1 + (subnet % 7) as u64, 1_500),
                )
            })
            .collect::<AddressTotals>();
        let rows = [AddressSetRow {
            key: BucketKey::new("r1", Granularity::FiveMinutes, 0, 300),
            scope: Scope::new(IpVersion::V6, Locality::All, Locality::All),
            address_side: AddressSide::Source,
            addresses: &addresses,
        }];

        let (rows, _) = maad_rows(&rows).unwrap();

        assert_eq!(rows.len(), 7);
        assert!(rows.iter().all(|row| row.dimensions.ip_version == 6));
        for row in &rows {
            let metadata: serde_json::Value = serde_json::from_str(&row.metadata_json).unwrap();
            assert_eq!(metadata["totalAddrs"], 256, "{}", row.measure);
            assert_eq!(metadata["minPrefixLength"], 23, "{}", row.measure);
            assert_eq!(metadata["maxPrefixLength"], 63, "{}", row.measure);
        }
    }

    #[test]
    fn weighted_measures_exclude_zero_weight_addresses_and_count_them() {
        let addresses = (0..=255_u8)
            .map(|last| {
                let packets = u64::from(last % 4 != 0);
                let bytes = if last < 10 { 0 } else { 40 * u64::from(last) };
                (
                    IpAddr::V4(Ipv4Addr::new(10, last % 16, last, 1)),
                    AddressTraffic::new(packets, bytes),
                )
            })
            .chain([(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
                AddressTraffic::new(0, 0),
            )])
            .collect::<AddressTotals>();
        let rows = [AddressSetRow {
            key: BucketKey::new("r1", Granularity::FiveMinutes, 0, 300),
            scope: Scope::new(IpVersion::V4, Locality::All, Locality::All),
            address_side: AddressSide::Destination,
            addresses: &addresses,
        }];

        let (rows, excluded) = maad_rows(&rows).unwrap();

        let metadata = |measure: &str| -> serde_json::Value {
            serde_json::from_str(
                &rows
                    .iter()
                    .find(|row| row.measure == measure && row.structure_kind == "structure")
                    .unwrap()
                    .metadata_json,
            )
            .unwrap()
        };
        assert_eq!(excluded, 64 + 10);
        assert_eq!(metadata("addresses")["totalAddrs"], 256);
        assert!(metadata("addresses").get("zeroWeightAddrs").is_none());
        assert_eq!(metadata("packets")["totalAddrs"], 192);
        assert_eq!(metadata("packets")["zeroWeightAddrs"], 64);
        assert_eq!(metadata("bytes")["totalAddrs"], 246);
        assert_eq!(metadata("bytes")["zeroWeightAddrs"], 10);
        let packets = addresses
            .iter()
            .filter_map(|(address, traffic)| match address {
                IpAddr::V4(address) if traffic.packets > 0 => {
                    Some((address, traffic.packets as f64))
                }
                _ => None,
            });
        assert_eq!(
            rows.iter()
                .find(|row| row.measure == "packets" && row.structure_kind == "structure")
                .unwrap()
                .values_json,
            serde_json::to_string(&maad::compute_weighted(packets).unwrap().structure).unwrap()
        );
    }
}
