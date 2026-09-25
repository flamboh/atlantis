//! Convert canonical buckets into persistent rows.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::OnceLock,
    time::{Duration, Instant},
};

use rayon::prelude::*;
use rusqlite::Connection;
use thiserror::Error;

use crate::{
    domain::{
        AddressSetRow, AddressTraffic, BucketKey, CanonicalBucket, CanonicalRows, DomainError,
        IpVersion, MaadMeasure,
    },
    maad,
    storage::{
        AddressCountStatsRow, AddressMaadStatsRow, BucketCoverageRow, MaadCurve, MaadQGridRow,
        PortCountStatsRow, ProtocolStatsRow, StatsBucketKey, StatsDimensions, StorageError,
        TrafficStatsRow, delete_stats_bucket_keys, insert_address_count_stats_rows,
        insert_address_maad_stats_rows, insert_bucket_coverage_rows, insert_maad_q_grid_rows,
        insert_port_count_stats_rows, insert_protocol_stats_rows, insert_traffic_stats_rows,
    },
};

#[derive(Debug, Error)]
pub enum PublishError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("unable to build MAAD worker pool: {0}")]
    MaadPool(String),
    #[error(transparent)]
    Maad(#[from] maad::MaadError),
    #[error("aggregate bucket lacks complete five-minute coverage: {0:?}")]
    IncompleteCoverage(BucketKey),
}

const MAX_MAAD_WORKERS: usize = 8;
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
    pub(crate) address_maad_insert_elapsed: Duration,
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
    pub(crate) address_maad_rows: u64,
    pub(crate) address_maad_blob_bytes: u64,
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
                + self.address_maad_insert_elapsed,
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
    if run_maad {
        insert_maad_q_grid_rows(connection, &maad_q_grid_rows())?;
    }
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
            let (address_maad, zero_weight_addresses) = maad_rows(&rows.address_sets)?;
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
            profile.address_maad_rows += count(address_maad.len());
            profile.address_maad_blob_bytes += address_maad
                .iter()
                .map(|row| {
                    count(
                        row.curve
                            .as_ref()
                            .map_or(0, |curve| curve.tau.len() + curve.tau_sd.len())
                            + row.spectrum.as_ref().map_or(0, Vec::len),
                    )
                })
                .sum::<u64>();
            let insert_started = Instant::now();
            insert_address_maad_stats_rows(connection, &address_maad)?;
            profile.address_maad_insert_elapsed += insert_started.elapsed();
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

/// The q grid of each IP version's stored tau and tau_sd arrays.
fn maad_q_grid_rows() -> [MaadQGridRow; 2] {
    let row = |ip_version: IpVersion, grid: maad::QGrid| MaadQGridRow {
        ip_version: i64::from(ip_version.number()),
        q_min: grid.q_min,
        q_step: grid.q_step,
        q_count: i64::try_from(grid.q_count).unwrap_or(i64::MAX),
    };
    [
        row(IpVersion::V4, maad::default_q_grid::<Ipv4Addr>()),
        row(IpVersion::V6, maad::default_q_grid::<Ipv6Addr>()),
    ]
}

/// Compute every MAAD measure for each scoped address set.
///
/// Returns the rows and the number of addresses excluded from weighted measures
/// because their summed weight was zero.
fn maad_rows(
    address_sets: &[AddressSetRow<'_>],
) -> Result<(Vec<AddressMaadStatsRow>, u64), PublishError> {
    if address_sets.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let pool = maad_pool()?;
    let measured = pool.install(|| {
        address_sets
            .par_iter()
            .map(scope_rows)
            .collect::<Result<Vec<_>, _>>()
    })?;
    let zero_weight_addresses = measured.iter().map(|(_, excluded)| count(*excluded)).sum();
    Ok((
        measured.into_iter().flat_map(|(rows, _)| rows).collect(),
        zero_weight_addresses,
    ))
}

/// One scope's MAAD results: distinct addresses, then packets and bytes with
/// the number of zero-weight addresses each weighted measure excluded.
struct ScopeResults {
    addresses: maad::MaadResult,
    weighted: [(maad::MaadResult, usize); 2],
}

fn scope_rows(
    addresses: &AddressSetRow<'_>,
) -> Result<(Vec<AddressMaadStatsRow>, usize), PublishError> {
    let results = match addresses.scope.ip_version {
        IpVersion::V4 => scope_results(addresses.addresses.iter().filter_map(
            |(address, traffic)| match address {
                IpAddr::V4(address) => Some((address, traffic)),
                IpAddr::V6(_) => None,
            },
        ))?,
        IpVersion::V6 => scope_results(addresses.addresses.iter().filter_map(
            |(address, traffic)| match address {
                IpAddr::V6(address) => Some((address, traffic)),
                IpAddr::V4(_) => None,
            },
        ))?,
    };
    let dimensions = dimensions(&addresses.key, addresses.scope);
    let row = |measure: MaadMeasure, result: &maad::MaadResult, zero_weight_addrs: usize| {
        maad_row(
            dimensions.clone(),
            addresses.address_side.as_str(),
            measure,
            result,
            zero_weight_addrs,
        )
    };
    let mut rows = vec![row(MaadMeasure::Addresses, &results.addresses, 0)];
    let mut zero_weight_addresses = 0;
    for (measure, (result, zero_weight_addrs)) in [MaadMeasure::Packets, MaadMeasure::Bytes]
        .into_iter()
        .zip(&results.weighted)
    {
        rows.push(row(measure, result, *zero_weight_addrs));
        zero_weight_addresses += zero_weight_addrs;
    }
    Ok((rows, zero_weight_addresses))
}

/// Store one measure's result with its curve rounded to f32. Only the addresses measure
/// computes a spectrum, so weighted measures store none rather than an empty one.
fn maad_row(
    dimensions: StatsDimensions,
    address_side: &str,
    measure: MaadMeasure,
    result: &maad::MaadResult,
    zero_weight_addrs: usize,
) -> AddressMaadStatsRow {
    let curve = match result.dimensions.as_slice() {
        [d0, d1, d2] => Some(MaadCurve {
            d0: d0.dim,
            d1: d1.dim,
            d2: d2.dim,
            tau: maad::encode_f32(result.structure.iter().map(|row| row.tau_tilde)),
            tau_sd: maad::encode_f32(result.structure.iter().map(|row| row.sd)),
        }),
        _ => None,
    };
    let metadata = &result.metadata;
    AddressMaadStatsRow {
        dimensions,
        address_side: address_side.to_owned(),
        measure: measure.as_str().to_owned(),
        total_addrs: i64::try_from(metadata.total_addrs).unwrap_or(i64::MAX),
        zero_weight_addrs: i64::try_from(zero_weight_addrs).unwrap_or(i64::MAX),
        min_prefix_length: metadata.min_prefix_length,
        max_prefix_length: metadata.max_prefix_length,
        curve,
        spectrum: (measure == MaadMeasure::Addresses)
            .then(|| maad::encode_f32(result.spectrum.iter().flat_map(|row| [row.alpha, row.f]))),
    }
}

/// Compute all measures over one shared prefix walk. Weighted measures leave out
/// zero-weight addresses, so a scope with any falls back to separate walks.
fn scope_results<A: maad::MaadAddress>(
    entries: impl Iterator<Item = (A, AddressTraffic)>,
) -> Result<ScopeResults, maad::MaadError> {
    let entries = entries.collect::<Vec<_>>();
    let zero_packets = entries
        .iter()
        .filter(|(_, traffic)| traffic.packets == 0)
        .count();
    let zero_bytes = entries
        .iter()
        .filter(|(_, traffic)| traffic.bytes == 0)
        .count();
    if zero_packets == 0 && zero_bytes == 0 {
        let (addresses, [packets, bytes]) =
            maad::compute_measures(entries.iter().map(|&(address, traffic)| {
                (address, [traffic.packets as f64, traffic.bytes as f64])
            }))?;
        return Ok(ScopeResults {
            addresses,
            weighted: [(packets, 0), (bytes, 0)],
        });
    }
    let weighted = |weight: fn(AddressTraffic) -> u64| {
        maad::compute_weighted(entries.iter().filter_map(|&(address, traffic)| {
            match weight(traffic) {
                0 => None,
                value => Some((address, value as f64)),
            }
        }))
    };
    Ok(ScopeResults {
        addresses: maad::compute(entries.iter().map(|&(address, _)| address)),
        weighted: [
            (weighted(|traffic| traffic.packets)?, zero_packets),
            (weighted(|traffic| traffic.bytes)?, zero_bytes),
        ],
    })
}

fn maad_pool() -> Result<&'static rayon::ThreadPool, PublishError> {
    match MAAD_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(
                std::thread::available_parallelism()
                    .map_or(1, std::num::NonZeroUsize::get)
                    .min(MAX_MAAD_WORKERS),
            )
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
                .query_row("SELECT COUNT(*) FROM address_maad_stats", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            60
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT group_concat(ip_version || ':' || q_min || ':' || q_step || ':' || q_count, ',')
                     FROM (SELECT * FROM maad_q_grid ORDER BY ip_version)",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "4:-0.5:0.125:33,6:-0.5:0.125:33"
        );
        assert_eq!(profile.bucket_keys, 1);
        assert_eq!(profile.write_calls, 1);
        assert_eq!(profile.traffic_rows, 10);
        assert_eq!(profile.protocol_rows, 10);
        assert_eq!(profile.address_count_rows, 20);
        assert_eq!(profile.port_count_rows, 40);
        assert_eq!(profile.maad_address_sets, 20);
        assert_eq!(profile.address_maad_rows, 60);
        assert_eq!(profile.maad_zero_weight_addresses, 0);
        assert_eq!(profile.address_maad_blob_bytes, 0);
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
                    "SELECT printf('%s|%s|%s|%s|%s|%d|%d|%s|%s|%s|%s|%s|%s|%s|%s', ip_version, src_locality, dst_locality, address_side, measure, total_addrs, zero_weight_addrs, min_prefix_length, max_prefix_length, d0, d1, d2, hex(tau), hex(tau_sd), hex(spectrum)) FROM address_maad_stats ORDER BY ip_version, src_locality, dst_locality, address_side, measure",
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
        let measures = |src_locality: &str, side: &str| {
            ["addresses", "packets", "bytes"]
                .map(|measure| (src_locality.to_owned(), side.to_owned(), measure.to_owned()))
        };
        assert_eq!(
            first
                .iter()
                .map(|row| (
                    row.dimensions.src_locality.clone(),
                    row.address_side.clone(),
                    row.measure.clone(),
                ))
                .collect::<Vec<_>>(),
            [
                measures("all", "source"),
                measures("external", "destination")
            ]
            .concat()
        );
        let tau =
            |row: &AddressMaadStatsRow| maad::decode_f32(&row.curve.as_ref().unwrap().tau).unwrap();
        assert_ne!(tau(&first[1]), tau(&first[0]));
        let uniform_packets = tau(&first[4]);
        let addresses = tau(&first[3]);
        assert_eq!(uniform_packets.len(), addresses.len());
        for (packets, addresses) in uniform_packets.iter().zip(&addresses) {
            assert!((packets - addresses).abs() <= f32::EPSILON * addresses.abs().max(1.0));
        }
        assert!(first[0].spectrum.is_some());
        assert!(first[1].spectrum.is_none() && first[2].spectrum.is_none());
    }

    #[test]
    fn maad_row_rounds_the_result_to_f32_and_keeps_dimensions_exact() {
        let addresses = (0..=255_u8).map(|last| Ipv4Addr::new(10, last % 16, last, 1));
        let result = maad::compute(addresses);
        let row = maad_row(
            dimensions(
                &BucketKey::new("r1", Granularity::FiveMinutes, 0, 300),
                Scope::new(IpVersion::V4, Locality::All, Locality::All),
            ),
            "source",
            MaadMeasure::Addresses,
            &result,
            0,
        );

        let curve = row.curve.unwrap();
        let grid = maad::default_q_grid::<Ipv4Addr>();
        assert_eq!(curve.tau.len(), grid.q_count * 4);
        for (index, (structure, (tau, sd))) in result
            .structure
            .iter()
            .zip(
                maad::decode_f32(&curve.tau)
                    .unwrap()
                    .into_iter()
                    .zip(maad::decode_f32(&curve.tau_sd).unwrap()),
            )
            .enumerate()
        {
            assert_eq!(structure.q, grid.q_min + index as f64 * grid.q_step);
            assert_eq!(tau, structure.tau_tilde as f32);
            assert_eq!(sd, structure.sd as f32);
        }
        assert_eq!(
            [curve.d0, curve.d1, curve.d2],
            [0, 1, 2].map(|index| result.dimensions[index].dim)
        );
        let spectrum = maad::decode_f32(&row.spectrum.unwrap()).unwrap();
        assert!(!result.spectrum.is_empty());
        assert_eq!(
            spectrum,
            result
                .spectrum
                .iter()
                .flat_map(|point| [point.alpha as f32, point.f as f32])
                .collect::<Vec<_>>()
        );
        assert_eq!(row.total_addrs, 256);
        assert_eq!(
            (row.min_prefix_length, row.max_prefix_length),
            (
                result.metadata.min_prefix_length,
                result.metadata.max_prefix_length
            )
        );
    }

    #[test]
    fn empty_results_store_no_curve_and_only_addresses_store_a_spectrum() {
        let result = maad::compute([Ipv4Addr::new(192, 0, 2, 1)]);
        let row = |measure| {
            maad_row(
                dimensions(
                    &BucketKey::new("r1", Granularity::FiveMinutes, 0, 300),
                    Scope::new(IpVersion::V4, Locality::All, Locality::All),
                ),
                "source",
                measure,
                &result,
                0,
            )
        };

        let addresses = row(MaadMeasure::Addresses);
        assert_eq!(addresses.curve, None);
        assert_eq!(addresses.spectrum, Some(Vec::new()));
        assert_eq!(addresses.total_addrs, 1);
        assert_eq!(row(MaadMeasure::Bytes).spectrum, None);
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

        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.dimensions.ip_version == 6));
        for row in &rows {
            assert_eq!(row.total_addrs, 256, "{}", row.measure);
            assert_eq!(row.min_prefix_length, Some(23), "{}", row.measure);
            assert_eq!(row.max_prefix_length, Some(63), "{}", row.measure);
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

        let row = |measure: &str| rows.iter().find(|row| row.measure == measure).unwrap();
        let counts = |measure: &str| (row(measure).total_addrs, row(measure).zero_weight_addrs);
        assert_eq!(excluded, 64 + 10);
        assert_eq!(counts("addresses"), (256, 0));
        assert_eq!(counts("packets"), (192, 64));
        assert_eq!(counts("bytes"), (246, 10));
        let packets = addresses
            .iter()
            .filter_map(|(address, traffic)| match address {
                IpAddr::V4(address) if traffic.packets > 0 => {
                    Some((address, traffic.packets as f64))
                }
                _ => None,
            });
        assert_eq!(
            row("packets").curve.as_ref().unwrap().tau,
            maad::encode_f32(
                maad::compute_weighted(packets)
                    .unwrap()
                    .structure
                    .iter()
                    .map(|structure| structure.tau_tilde)
            )
        );
    }
}
