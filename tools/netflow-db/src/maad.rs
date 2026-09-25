//! In-process MAAD-compatible multifractal analysis for IPv4 and IPv6 address sets.

use rayon::prelude::*;
use serde::Serialize;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const MIN_MAAD_ADDRESSES: usize = 2;
const SCHEMA_VERSION: u32 = 3;
const DEFAULT_FULL_THRESHOLD: f64 = 0.05;
const DEFAULT_Q_STEP: f64 = 1.0 / 8.0;
const DEFAULT_Q_MIN: f64 = -0.5;
const DEFAULT_Q_MAX: f64 = 3.5;
const DEFAULT_IPV4_MIN_PREFIX_LENGTH: u8 = 8;
const DEFAULT_IPV4_MAX_PREFIX_LENGTH: u8 = 24;
const DEFAULT_IPV6_MIN_PREFIX_LENGTH: u8 = 23;
const DEFAULT_IPV6_MAX_PREFIX_LENGTH: u8 = 64;
const MAX_Q_VALUES: usize = 1025;
const GRID_EPSILON: f64 = 1e-12;

/// Fixed-width address bits whose prefixes MAAD partitions.
pub trait PrefixBits: Copy + Ord {
    const WIDTH: u8;

    /// The leading `prefix_length` bits, right-aligned.
    fn prefix(self, prefix_length: u8) -> Self;

    /// The right-aligned prefix one bit longer, with the new bit set to `bit`.
    fn child(self, bit: bool) -> Self;
}

impl PrefixBits for u32 {
    const WIDTH: u8 = 32;

    fn prefix(self, prefix_length: u8) -> Self {
        if prefix_length == 0 {
            0
        } else {
            self >> (Self::WIDTH - prefix_length)
        }
    }

    fn child(self, bit: bool) -> Self {
        (self << 1) | Self::from(bit)
    }
}

impl PrefixBits for u128 {
    const WIDTH: u8 = 128;

    fn prefix(self, prefix_length: u8) -> Self {
        if prefix_length == 0 {
            0
        } else {
            self >> (Self::WIDTH - prefix_length)
        }
    }

    fn child(self, bit: bool) -> Self {
        (self << 1) | Self::from(bit)
    }
}

/// An address family MAAD can analyze, with its upstream default prefix range.
pub trait MaadAddress: Copy + Into<IpAddr> {
    type Bits: PrefixBits;

    fn bits(self) -> Self::Bits;

    fn default_config() -> MaadConfig;
}

impl MaadAddress for Ipv4Addr {
    type Bits = u32;

    fn bits(self) -> u32 {
        u32::from(self)
    }

    fn default_config() -> MaadConfig {
        MaadConfig::ipv4()
    }
}

impl MaadAddress for Ipv6Addr {
    type Bits = u128;

    fn bits(self) -> u128 {
        u128::from(self)
    }

    fn default_config() -> MaadConfig {
        MaadConfig::ipv6()
    }
}

/// Configuration for the in-process MAAD estimator.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct MaadConfig {
    pub q_min: f64,
    pub q_max: f64,
    pub q_step: f64,
    pub min_prefix_length: u8,
    pub max_prefix_length: u8,
    pub full_threshold: f64,
}

impl MaadConfig {
    pub const fn ipv4() -> Self {
        Self::with_prefix_range(
            DEFAULT_IPV4_MIN_PREFIX_LENGTH,
            DEFAULT_IPV4_MAX_PREFIX_LENGTH,
        )
    }

    pub const fn ipv6() -> Self {
        Self::with_prefix_range(
            DEFAULT_IPV6_MIN_PREFIX_LENGTH,
            DEFAULT_IPV6_MAX_PREFIX_LENGTH,
        )
    }

    const fn with_prefix_range(min_prefix_length: u8, max_prefix_length: u8) -> Self {
        Self {
            q_min: DEFAULT_Q_MIN,
            q_max: DEFAULT_Q_MAX,
            q_step: DEFAULT_Q_STEP,
            min_prefix_length,
            max_prefix_length,
            full_threshold: DEFAULT_FULL_THRESHOLD,
        }
    }
}

impl Default for MaadConfig {
    fn default() -> Self {
        Self::ipv4()
    }
}

/// Configuration or input errors returned by the configurable estimator.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum MaadError {
    #[error("q bounds must be finite (q_min={q_min}, q_max={q_max})")]
    NonFiniteQBounds { q_min: f64, q_max: f64 },
    #[error("q_min must not exceed q_max (q_min={q_min}, q_max={q_max})")]
    InvalidQBounds { q_min: f64, q_max: f64 },
    #[error("q_step must be finite and positive (q_step={q_step})")]
    InvalidQStep { q_step: f64 },
    #[error(
        "q range must contain q_max on a uniform q_step grid (q_min={q_min}, q_max={q_max}, q_step={q_step})"
    )]
    QRangeNotAligned { q_min: f64, q_max: f64, q_step: f64 },
    #[error("q grid would contain {requested_values} values; the maximum is {max_values}")]
    QGridTooLarge {
        requested_values: f64,
        max_values: usize,
    },
    #[error("required q={q} is not on the configured q grid")]
    RequiredQNotOnGrid { q: f64 },
    #[error(
        "prefix range must satisfy 0 <= min < max < {address_bits} (min={min_prefix_length}, max={max_prefix_length})"
    )]
    InvalidPrefixRange {
        min_prefix_length: u8,
        max_prefix_length: u8,
        address_bits: u8,
    },
    #[error("full_threshold must be finite and in [0, 1) (full_threshold={full_threshold})")]
    InvalidFullThreshold { full_threshold: f64 },
    #[error("weight for {address} must be finite and positive (weight={weight})")]
    InvalidWeight { address: IpAddr, weight: f64 },
    #[error("summed weights must be finite (total={total})")]
    NonFiniteTotalWeight { total: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaadResult {
    pub schema_version: u32,
    pub metadata: MaadMetadata,
    pub structure: Vec<StructureRow>,
    pub spectrum: Vec<SpectrumRow>,
    pub dimensions: Vec<DimensionRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaadMetadata {
    pub input: &'static str,
    pub prefix_lengths: Vec<u8>,
    pub min_prefix_length: Option<u8>,
    pub max_prefix_length: Option<u8>,
    pub total_addrs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureRow {
    pub q: f64,
    pub tau_tilde: f64,
    pub sd: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SpectrumRow {
    pub alpha: f64,
    pub f: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DimensionRow {
    pub q: f64,
    pub dim: f64,
    pub sd: f64,
}

#[derive(Clone, Debug)]
struct PreparedMoment<M> {
    parent_masses: Vec<M>,
    child_masses: Vec<(M, Option<M>)>,
}

/// Compute MAAD-compatible output from one address family's address set.
pub fn compute<A: MaadAddress>(addresses: impl IntoIterator<Item = A>) -> MaadResult {
    compute_with_config(addresses, A::default_config())
        .expect("the default MAAD configuration must be valid")
}

/// Compute MAAD-compatible output using an explicitly validated configuration.
pub fn compute_with_config<A: MaadAddress>(
    addresses: impl IntoIterator<Item = A>,
    config: MaadConfig,
) -> Result<MaadResult, MaadError> {
    let q_values = validate_config(&config, A::Bits::WIDTH)?;
    let mut addresses: Vec<_> = addresses.into_iter().map(A::bits).collect();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.len() < MIN_MAAD_ADDRESSES {
        return Ok(empty_result(addresses.len()));
    }
    Ok(analyze(&addresses, &[], &[], &config).address_result(&q_values, config.q_step))
}

/// Compute measure-weighted MAAD structure and dimensions from `(address, weight)` pairs.
///
/// Weights of duplicate addresses are summed. Prefix validity and path pruning use distinct
/// address counts; moments and entropy use summed weights. The spectrum is always empty.
pub fn compute_weighted<A: MaadAddress>(
    entries: impl IntoIterator<Item = (A, f64)>,
) -> Result<MaadResult, MaadError> {
    compute_weighted_with_config(entries, A::default_config())
}

/// Compute measure-weighted MAAD output using an explicitly validated configuration.
pub fn compute_weighted_with_config<A: MaadAddress>(
    entries: impl IntoIterator<Item = (A, f64)>,
    config: MaadConfig,
) -> Result<MaadResult, MaadError> {
    let (_, [weighted]) = compute_measures_with_config(
        entries
            .into_iter()
            .map(|(address, weight)| (address, [weight])),
        config,
    )?;
    Ok(weighted)
}

/// Compute the distinct-address result and one measure-weighted result per weight column.
///
/// Every measure shares one sort and one walk over the prefix levels, because prefix validity
/// and path pruning depend only on the distinct addresses. Each result equals the one
/// [`compute`] or [`compute_weighted`] returns for the same addresses and weights.
pub fn compute_measures<A: MaadAddress, const K: usize>(
    entries: impl IntoIterator<Item = (A, [f64; K])>,
) -> Result<(MaadResult, [MaadResult; K]), MaadError> {
    compute_measures_with_config(entries, A::default_config())
}

/// Compute every measure using an explicitly validated configuration.
pub fn compute_measures_with_config<A: MaadAddress, const K: usize>(
    entries: impl IntoIterator<Item = (A, [f64; K])>,
    config: MaadConfig,
) -> Result<(MaadResult, [MaadResult; K]), MaadError> {
    let q_values = validate_config(&config, A::Bits::WIDTH)?;
    let mut entries = entries
        .into_iter()
        .map(|(address, weights)| {
            match weights
                .iter()
                .find(|weight| !(weight.is_finite() && **weight > 0.0))
            {
                Some(&weight) => Err(MaadError::InvalidWeight {
                    address: address.into(),
                    weight,
                }),
                None => Ok((address.bits(), weights)),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by(|left, right| {
        left.0.cmp(&right.0).then_with(|| {
            left.1
                .iter()
                .zip(&right.1)
                .map(|(left, right)| left.total_cmp(right))
                .find(|ordering| ordering.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    entries.dedup_by(|next, kept| {
        let duplicate = next.0 == kept.0;
        if duplicate {
            for (kept, next) in kept.1.iter_mut().zip(next.1) {
                *kept += next;
            }
        }
        duplicate
    });
    let totals: [f64; K] =
        std::array::from_fn(|k| entries.iter().map(|(_, weights)| weights[k]).sum::<f64>());
    if let Some(&total) = totals.iter().find(|total| !total.is_finite()) {
        return Err(MaadError::NonFiniteTotalWeight { total });
    }
    if entries.len() < MIN_MAAD_ADDRESSES {
        return Ok((
            empty_result(entries.len()),
            std::array::from_fn(|_| empty_result(entries.len())),
        ));
    }
    let addresses: Vec<_> = entries.iter().map(|&(address, _)| address).collect();
    let columns: Vec<Vec<f64>> = (0..K)
        .map(|k| entries.iter().map(|(_, weights)| weights[k]).collect())
        .collect();
    drop(entries);
    let analysis = analyze(&addresses, &columns, &totals, &config);
    Ok((
        analysis.address_result(&q_values, config.q_step),
        std::array::from_fn(|k| analysis.weighted_result(k, &q_values)),
    ))
}

/// Serialize a computed MAAD result using the established JSON field names.
pub fn write_json<W: Write>(result: &MaadResult, mut output: W) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut output, result)?;
    output.write_all(b"\n").map_err(serde_json::Error::io)
}

/// Prepared moments and level entropies for one measure.
struct MeasureAnalysis<M> {
    moments: Vec<PreparedMoment<M>>,
    entropies: Vec<f64>,
}

impl<M> MeasureAnalysis<M> {
    const fn new() -> Self {
        Self {
            moments: Vec::new(),
            entropies: Vec::new(),
        }
    }
}

/// Every measure's moments over the prefix levels that have a valid parent.
struct Analysis {
    total_addrs: usize,
    prefix_lengths: Vec<u8>,
    counts: MeasureAnalysis<usize>,
    weighted: Vec<MeasureAnalysis<f64>>,
}

impl Analysis {
    fn address_result(&self, q_values: &[f64], q_step: f64) -> MaadResult {
        if self.prefix_lengths.is_empty() {
            return empty_result(self.total_addrs);
        }
        let structure = compute_structure(&self.counts.moments, q_values);
        let spectrum = compute_spectrum(&structure, q_step);
        let dimensions = self.dimensions(&structure, &self.counts.entropies);
        self.result(structure, spectrum, dimensions)
    }

    fn weighted_result(&self, measure: usize, q_values: &[f64]) -> MaadResult {
        if self.prefix_lengths.is_empty() {
            return empty_result(self.total_addrs);
        }
        let weighted = &self.weighted[measure];
        let structure = compute_weighted_structure(&weighted.moments, q_values);
        let dimensions = self.dimensions(&structure, &weighted.entropies);
        self.result(structure, Vec::new(), dimensions)
    }

    fn dimensions(&self, structure: &[StructureRow], entropies: &[f64]) -> Vec<DimensionRow> {
        if structure.is_empty() {
            return Vec::new();
        }
        dimension_rows(structure, info_dimension(&self.prefix_lengths, entropies))
    }

    fn result(
        &self,
        structure: Vec<StructureRow>,
        spectrum: Vec<SpectrumRow>,
        dimensions: Vec<DimensionRow>,
    ) -> MaadResult {
        MaadResult {
            schema_version: SCHEMA_VERSION,
            metadata: MaadMetadata {
                input: "-",
                min_prefix_length: self.prefix_lengths.first().copied(),
                max_prefix_length: self.prefix_lengths.last().copied(),
                prefix_lengths: self.prefix_lengths.clone(),
                total_addrs: self.total_addrs,
            },
            structure,
            spectrum,
            dimensions,
        }
    }
}

fn empty_result(total_addrs: usize) -> MaadResult {
    MaadResult {
        schema_version: SCHEMA_VERSION,
        metadata: MaadMetadata {
            input: "-",
            prefix_lengths: Vec::new(),
            min_prefix_length: None,
            max_prefix_length: None,
            total_addrs,
        },
        structure: Vec::new(),
        spectrum: Vec::new(),
        dimensions: Vec::new(),
    }
}

/// Walk the prefix levels of sorted, distinct addresses from the root down.
///
/// Only a parent level and its child level are held at once, so memory stays linear in the
/// address count. Validity and path pruning use distinct-address counts; each weight column
/// contributes its own masses and entropies over the same selected parents.
fn analyze<B: PrefixBits>(
    addresses: &[B],
    weights: &[Vec<f64>],
    totals: &[f64],
    config: &MaadConfig,
) -> Analysis {
    let total_addrs = addresses.len();
    let mut analysis = Analysis {
        total_addrs,
        prefix_lengths: Vec::new(),
        counts: MeasureAnalysis::new(),
        weighted: weights.iter().map(|_| MeasureAnalysis::new()).collect(),
    };
    let mut parents = prefix_level(addresses, 0);
    let mut parent_masses: Vec<_> = weights
        .iter()
        .map(|column| weight_level(addresses, column, 0))
        .collect();
    let mut path_allowed = vec![true; parents.len()];

    for prefix_length in 0..=config.max_prefix_length {
        let children = prefix_level(addresses, prefix_length + 1);
        let child_masses: Vec<_> = weights
            .iter()
            .map(|column| weight_level(addresses, column, prefix_length + 1))
            .collect();
        let measured = prefix_length >= config.min_prefix_length;
        let propagate = prefix_length < config.max_prefix_length;
        let mut selected = Vec::new();
        let mut child_path_allowed = Vec::with_capacity(if propagate { children.len() } else { 0 });
        let mut next_child = 0;

        for (parent_index, &(prefix, count)) in parents.iter().enumerate() {
            let first_child = prefix.child(false);
            let last_child = prefix.child(true);
            while next_child < children.len() && children[next_child].0 < first_child {
                next_child += 1;
            }
            let child_start = next_child;
            while next_child < children.len() && children[next_child].0 <= last_child {
                next_child += 1;
            }
            let valid = is_valid_parent::<B>(count, prefix_length, config.full_threshold);
            if measured && path_allowed[parent_index] && valid && child_start < next_child {
                selected.push((parent_index, child_start, next_child));
            }
            if propagate {
                let is_branch = next_child - child_start == 2;
                let allowed = path_allowed[parent_index] && (!is_branch || valid);
                child_path_allowed.extend(std::iter::repeat_n(allowed, next_child - child_start));
            }
        }

        if !selected.is_empty() {
            analysis.prefix_lengths.push(prefix_length);
            analysis.counts.moments.push(select_moment(
                &selected,
                |index| parents[index].1,
                |index| children[index].1,
            ));
            analysis.counts.entropies.push(level_entropy(
                parents.iter().map(|&(_, count)| count as f64),
                total_addrs as f64,
            ));
            for (measure, weighted) in analysis.weighted.iter_mut().enumerate() {
                let parents = &parent_masses[measure];
                let children = &child_masses[measure];
                weighted.moments.push(select_moment(
                    &selected,
                    |index| parents[index],
                    |index| children[index],
                ));
                weighted
                    .entropies
                    .push(level_entropy(parents.iter().copied(), totals[measure]));
            }
        }

        debug_assert!(!propagate || child_path_allowed.len() == children.len());
        path_allowed = child_path_allowed;
        parents = children;
        parent_masses = child_masses;
    }

    analysis
}

fn prefix_level<B: PrefixBits>(addresses: &[B], prefix_length: u8) -> Vec<(B, usize)> {
    let mut prefixes: Vec<(B, usize)> = Vec::new();
    for &address in addresses {
        let prefix = address.prefix(prefix_length);
        if let Some((last_prefix, count)) = prefixes.last_mut()
            && *last_prefix == prefix
        {
            *count += 1;
        } else {
            prefixes.push((prefix, 1));
        }
    }
    prefixes
}

fn weight_level<B: PrefixBits>(addresses: &[B], weights: &[f64], prefix_length: u8) -> Vec<f64> {
    let mut level = Vec::new();
    let mut last_prefix = None;
    for (&address, &weight) in addresses.iter().zip(weights) {
        let prefix = address.prefix(prefix_length);
        match level.last_mut() {
            Some(total) if last_prefix == Some(prefix) => *total += weight,
            _ => level.push(weight),
        }
        last_prefix = Some(prefix);
    }
    level
}

fn select_moment<M: Copy>(
    selected: &[(usize, usize, usize)],
    parent: impl Fn(usize) -> M,
    child: impl Fn(usize) -> M,
) -> PreparedMoment<M> {
    PreparedMoment {
        parent_masses: selected
            .iter()
            .map(|&(parent_index, _, _)| parent(parent_index))
            .collect(),
        child_masses: selected
            .iter()
            .map(|&(_, child_start, child_end)| {
                (
                    child(child_start),
                    (child_end - child_start == 2).then(|| child(child_start + 1)),
                )
            })
            .collect(),
    }
}

fn level_entropy(masses: impl Iterator<Item = f64>, total: f64) -> f64 {
    masses
        .map(|mass| {
            let probability = mass / total;
            probability * probability.log2()
        })
        .sum::<f64>()
}

fn validate_config(config: &MaadConfig, address_bits: u8) -> Result<Vec<f64>, MaadError> {
    if !config.q_min.is_finite() || !config.q_max.is_finite() {
        return Err(MaadError::NonFiniteQBounds {
            q_min: config.q_min,
            q_max: config.q_max,
        });
    }
    if config.q_min > config.q_max {
        return Err(MaadError::InvalidQBounds {
            q_min: config.q_min,
            q_max: config.q_max,
        });
    }
    if !config.q_step.is_finite() || config.q_step <= 0.0 {
        return Err(MaadError::InvalidQStep {
            q_step: config.q_step,
        });
    }
    if config.min_prefix_length >= config.max_prefix_length
        || config.max_prefix_length >= address_bits
    {
        return Err(MaadError::InvalidPrefixRange {
            min_prefix_length: config.min_prefix_length,
            max_prefix_length: config.max_prefix_length,
            address_bits,
        });
    }
    if !config.full_threshold.is_finite() || !(0.0..1.0).contains(&config.full_threshold) {
        return Err(MaadError::InvalidFullThreshold {
            full_threshold: config.full_threshold,
        });
    }

    let step_count = (config.q_max - config.q_min) / config.q_step;
    if !step_count.is_finite() {
        return Err(MaadError::QRangeNotAligned {
            q_min: config.q_min,
            q_max: config.q_max,
            q_step: config.q_step,
        });
    }
    let rounded_step_count = step_count.round();
    if (step_count - rounded_step_count).abs() > GRID_EPSILON * rounded_step_count.abs().max(1.0) {
        return Err(MaadError::QRangeNotAligned {
            q_min: config.q_min,
            q_max: config.q_max,
            q_step: config.q_step,
        });
    }
    let requested_values = rounded_step_count + 1.0;
    if requested_values > MAX_Q_VALUES as f64 {
        return Err(MaadError::QGridTooLarge {
            requested_values,
            max_values: MAX_Q_VALUES,
        });
    }

    let step_count = rounded_step_count as usize;
    let mut q_values: Vec<_> = (0..=step_count)
        .map(|index| config.q_min + index as f64 * config.q_step)
        .collect();
    for required_q in [0.0_f64, 2.0] {
        let tolerance = GRID_EPSILON * required_q.abs().max(1.0);
        let Some(value) = q_values
            .iter_mut()
            .find(|value| (**value - required_q).abs() <= tolerance)
        else {
            return Err(MaadError::RequiredQNotOnGrid { q: required_q });
        };
        *value = required_q;
    }
    Ok(q_values)
}

fn is_valid_parent<B: PrefixBits>(count: usize, prefix_length: u8, full_threshold: f64) -> bool {
    count > 1 && (count as f64).log2() / f64::from(B::WIDTH - prefix_length) < 1.0 - full_threshold
}

fn one_moment<M: Copy>(prepared: &PreparedMoment<M>, power: impl Fn(M) -> f64) -> (f64, f64) {
    if prepared.parent_masses.is_empty() {
        return (0.0, 0.0);
    }
    let parent_powers: Vec<_> = prepared
        .parent_masses
        .iter()
        .map(|&mass| power(mass))
        .collect();
    let child_power_sums: Vec<_> = prepared
        .child_masses
        .iter()
        .map(|&(first, second)| match second {
            Some(second) => power(first) + power(second),
            None => power(first),
        })
        .collect();
    let this_z: f64 = parent_powers.iter().sum();
    let next_z: f64 = child_power_sums.iter().sum();
    if this_z <= 0.0 || next_z <= 0.0 {
        return (0.0, 0.0);
    }
    let d2 = parent_powers
        .iter()
        .zip(child_power_sums)
        .map(|(parent, children)| (parent / this_z - children / next_z).powi(2))
        .sum();
    (this_z.log2() - next_z.log2(), d2)
}

fn moment_masses<M: Copy>(moment: &PreparedMoment<M>) -> impl Iterator<Item = M> + '_ {
    moment.parent_masses.iter().copied().chain(
        moment
            .child_masses
            .iter()
            .flat_map(|&(first, second)| std::iter::once(first).chain(second)),
    )
}

fn compute_structure(prepared: &[PreparedMoment<usize>], q_values: &[f64]) -> Vec<StructureRow> {
    if prepared.is_empty() {
        return Vec::new();
    }
    let max_count = prepared
        .iter()
        .flat_map(moment_masses)
        .max()
        .unwrap_or_default();
    q_values
        .par_iter()
        .map(|&q| {
            let powers: Vec<_> = (0..=max_count)
                .map(|count| (count as f64).powf(q))
                .collect();
            structure_row(
                q,
                prepared
                    .iter()
                    .map(|moment| one_moment(moment, |count| powers[count])),
            )
        })
        .collect()
}

/// Weighted masses repeat heavily, so each q raises only the distinct masses to the power
/// and every moment looks its masses up by index.
fn compute_weighted_structure(
    prepared: &[PreparedMoment<f64>],
    q_values: &[f64],
) -> Vec<StructureRow> {
    let mut distinct: Vec<f64> = prepared.iter().flat_map(moment_masses).collect();
    distinct.sort_unstable_by(f64::total_cmp);
    distinct.dedup_by(|next, kept| next.total_cmp(kept).is_eq());
    let index = |mass: f64| {
        distinct
            .binary_search_by(|probe| probe.total_cmp(&mass))
            .expect("every mass is in the distinct table")
    };
    let indexed: Vec<PreparedMoment<usize>> = prepared
        .iter()
        .map(|moment| PreparedMoment {
            parent_masses: moment
                .parent_masses
                .iter()
                .map(|&mass| index(mass))
                .collect(),
            child_masses: moment
                .child_masses
                .iter()
                .map(|&(first, second)| (index(first), second.map(index)))
                .collect(),
        })
        .collect();
    q_values
        .par_iter()
        .map(|&q| {
            let powers: Vec<_> = distinct.iter().map(|mass| mass.powf(q)).collect();
            structure_row(
                q,
                indexed
                    .iter()
                    .map(|moment| one_moment(moment, |index| powers[index])),
            )
        })
        .collect()
}

fn structure_row(q: f64, moments: impl ExactSizeIterator<Item = (f64, f64)>) -> StructureRow {
    let count = moments.len() as f64;
    let (tau_sum, d2_sum) = moments.fold((0.0, 0.0), |(tau_sum, d2_sum), (tau, d2)| {
        (tau_sum + tau, d2_sum + d2)
    });
    StructureRow {
        q,
        tau_tilde: tau_sum / count,
        sd: d2_sum.sqrt() / count,
    }
}

/// Central-difference `(q, alpha, f)` estimates for every interior structure row.
fn spectrum_estimates(structure: &[StructureRow], q_step: f64) -> Vec<(f64, SpectrumRow)> {
    structure
        .windows(3)
        .map(|rows| {
            let row = rows[1];
            let alpha = (rows[2].tau_tilde - rows[0].tau_tilde) / (2.0 * q_step);
            (
                row.q,
                SpectrumRow {
                    alpha,
                    f: row.q * alpha - row.tau_tilde,
                },
            )
        })
        .collect()
}

/// The `(q_max, q_min)` critical region: the widest q range with a positive `f` estimate,
/// always covering `0..=1`.
fn compute_critical_region(structure: &[StructureRow], q_step: f64) -> (f64, f64) {
    let estimates = spectrum_estimates(structure, q_step);
    let positive = |keep: fn(f64) -> bool| {
        estimates
            .iter()
            .filter(move |(q, row)| keep(*q) && row.f > 0.0)
            .map(|(q, _)| *q)
    };
    (
        positive(|q| q >= 1.0).fold(1.0, f64::max),
        positive(|q| q <= 0.0).fold(0.0, f64::min),
    )
}

/// Spectrum rows from the critical region where alpha is non-increasing.
///
/// Alphas closer than `GRID_EPSILON` count as ties, which upstream's `a1 >= a2` keeps.
fn compute_spectrum(structure: &[StructureRow], q_step: f64) -> Vec<SpectrumRow> {
    let (q_max, q_min) = compute_critical_region(structure, q_step);
    let alphas: Vec<_> = spectrum_estimates(structure, q_step)
        .into_iter()
        .filter(|(q, _)| q_min <= *q && *q <= q_max)
        .map(|(_, row)| row)
        .collect();
    let mut rows = Vec::new();
    let mut started = false;
    for pair in alphas.windows(2) {
        let non_increasing = pair[0].alpha - pair[1].alpha >= -GRID_EPSILON;
        if !started && !non_increasing {
            continue;
        }
        if !non_increasing {
            break;
        }
        started = true;
        rows.push(pair[1]);
    }
    rows
}

fn dimension_rows(structure: &[StructureRow], information_dimension: f64) -> Vec<DimensionRow> {
    let from_structure = |q: f64| {
        structure
            .iter()
            .find(|row| row.q == q)
            .map(|row| DimensionRow {
                q,
                dim: row.tau_tilde / (q - 1.0),
                sd: row.sd,
            })
            .expect("validated q grids contain q=0 and q=2")
    };
    vec![
        from_structure(0.0),
        DimensionRow {
            q: 1.0,
            dim: information_dimension,
            sd: 0.0,
        },
        from_structure(2.0),
    ]
}

fn info_dimension(prefix_lengths: &[u8], entropies: &[f64]) -> f64 {
    let points: Vec<_> = prefix_lengths
        .iter()
        .zip(entropies)
        .map(|(&prefix_length, &entropy)| (-(f64::from(prefix_length)), entropy))
        .collect();
    let point_count = points.len() as f64;
    let mean_x = points.iter().map(|point| point.0).sum::<f64>() / point_count;
    let mean_y = points.iter().map(|point| point.1).sum::<f64>() / point_count;
    let denominator = points
        .iter()
        .map(|point| (point.0 - mean_x).powi(2))
        .sum::<f64>();
    if denominator == 0.0 {
        return 0.0;
    }
    points
        .iter()
        .map(|point| (point.0 - mean_x) * (point.1 - mean_y))
        .sum::<f64>()
        / denominator
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn close(left: f64, right: f64) {
        assert!((left - right).abs() <= 1e-12, "{left} != {right}");
    }

    fn reference_compute<A: MaadAddress>(
        addresses: impl IntoIterator<Item = A>,
        config: MaadConfig,
    ) -> MaadResult
    where
        A::Bits: Into<u128>,
    {
        let width = A::Bits::WIDTH;
        let addresses: BTreeSet<u128> = addresses
            .into_iter()
            .map(|address| address.bits().into())
            .collect();
        if addresses.len() < MIN_MAAD_ADDRESSES {
            return empty_result(addresses.len());
        }
        let counts = reference_prefix_counts(&addresses, width);
        let prepared = reference_prepare_valid_moments(&counts, &config, width);
        if prepared.is_empty() {
            return empty_result(addresses.len());
        }
        let (prefix_lengths, prepared): (Vec<_>, Vec<_>) = prepared.into_iter().unzip();
        let q_values = reference_q_values(&config);
        let structure = reference_structure(&prepared, &q_values);
        let spectrum = compute_spectrum(&structure, config.q_step);
        let dimensions =
            reference_dimensions(&counts, &prefix_lengths, &structure, addresses.len());
        MaadResult {
            schema_version: SCHEMA_VERSION,
            metadata: MaadMetadata {
                input: "-",
                min_prefix_length: prefix_lengths.first().copied(),
                max_prefix_length: prefix_lengths.last().copied(),
                prefix_lengths,
                total_addrs: addresses.len(),
            },
            structure,
            spectrum,
            dimensions,
        }
    }

    fn reference_prefix_counts(
        addresses: &BTreeSet<u128>,
        width: u8,
    ) -> Vec<BTreeMap<u128, usize>> {
        let mut counts = vec![BTreeMap::new(); usize::from(width) + 1];
        for &address in addresses {
            for prefix_length in 0..=width {
                let prefix = if prefix_length == 0 {
                    0
                } else {
                    address >> (width - prefix_length)
                };
                *counts[usize::from(prefix_length)]
                    .entry(prefix)
                    .or_default() += 1;
            }
        }
        counts
    }

    fn reference_prepare_valid_moments(
        counts: &[BTreeMap<u128, usize>],
        config: &MaadConfig,
        width: u8,
    ) -> Vec<(u8, PreparedMoment<usize>)> {
        let mut prepared = Vec::new();
        let mut path_allowed = BTreeMap::from([(0, true)]);

        for prefix_length in 0..=config.max_prefix_length {
            let parents = &counts[usize::from(prefix_length)];
            let children = &counts[usize::from(prefix_length) + 1];

            if prefix_length >= config.min_prefix_length {
                let mut parent_counts = Vec::new();
                let mut child_counts = Vec::new();
                for (&prefix, &count) in parents {
                    if !path_allowed[&prefix]
                        || !reference_valid_parent(
                            count,
                            prefix_length,
                            config.full_threshold,
                            width,
                        )
                    {
                        continue;
                    }
                    let child_counts_for_parent: Vec<_> = [prefix << 1, (prefix << 1) | 1]
                        .into_iter()
                        .filter_map(|child| children.get(&child).copied())
                        .collect();
                    if !child_counts_for_parent.is_empty() {
                        parent_counts.push(count);
                        child_counts.push(child_counts_for_parent);
                    }
                }
                if !parent_counts.is_empty() {
                    prepared.push((
                        prefix_length,
                        PreparedMoment {
                            parent_masses: parent_counts,
                            child_masses: child_counts
                                .into_iter()
                                .map(|children| (children[0], children.get(1).copied()))
                                .collect(),
                        },
                    ));
                }
            }

            if prefix_length < config.max_prefix_length {
                path_allowed = children
                    .keys()
                    .map(|&child| {
                        let parent = child >> 1;
                        let parent_count = parents[&parent];
                        let is_branch = children.contains_key(&(parent << 1))
                            && children.contains_key(&((parent << 1) | 1));
                        let allowed = path_allowed[&parent]
                            && (!is_branch
                                || reference_valid_parent(
                                    parent_count,
                                    prefix_length,
                                    config.full_threshold,
                                    width,
                                ));
                        (child, allowed)
                    })
                    .collect();
            }
        }

        prepared
    }

    fn reference_valid_parent(
        count: usize,
        prefix_length: u8,
        full_threshold: f64,
        width: u8,
    ) -> bool {
        count > 1 && (count as f64).log2() / f64::from(width - prefix_length) < 1.0 - full_threshold
    }

    fn reference_q_values(config: &MaadConfig) -> Vec<f64> {
        let count = ((config.q_max - config.q_min) / config.q_step).round() as usize;
        (0..=count)
            .map(|index| {
                let q = config.q_min + index as f64 * config.q_step;
                if q.abs() <= GRID_EPSILON {
                    0.0
                } else if (q - 2.0).abs() <= GRID_EPSILON {
                    2.0
                } else {
                    q
                }
            })
            .collect()
    }

    fn reference_structure(
        prepared: &[PreparedMoment<usize>],
        q_values: &[f64],
    ) -> Vec<StructureRow> {
        q_values
            .iter()
            .copied()
            .map(|q| {
                let (tau_sum, d2_sum) = prepared
                    .iter()
                    .map(|moment| {
                        let parent_powers: Vec<_> = moment
                            .parent_masses
                            .iter()
                            .map(|&count| (count as f64).powf(q))
                            .collect();
                        let child_power_sums: Vec<_> = moment
                            .child_masses
                            .iter()
                            .map(|&(first, second)| {
                                std::iter::once(first)
                                    .chain(second)
                                    .map(|count| (count as f64).powf(q))
                                    .sum::<f64>()
                            })
                            .collect();
                        let this_z: f64 = parent_powers.iter().sum();
                        let next_z: f64 = child_power_sums.iter().sum();
                        if this_z <= 0.0 || next_z <= 0.0 {
                            return (0.0, 0.0);
                        }
                        let d2 = parent_powers
                            .iter()
                            .zip(child_power_sums)
                            .map(|(parent, children)| (parent / this_z - children / next_z).powi(2))
                            .sum();
                        (this_z.log2() - next_z.log2(), d2)
                    })
                    .fold((0.0, 0.0), |(tau_sum, d2_sum), (tau, d2)| {
                        (tau_sum + tau, d2_sum + d2)
                    });
                let count = prepared.len() as f64;
                StructureRow {
                    q,
                    tau_tilde: tau_sum / count,
                    sd: d2_sum.sqrt() / count,
                }
            })
            .collect()
    }

    fn reference_dimensions(
        counts: &[BTreeMap<u128, usize>],
        prefix_lengths: &[u8],
        structure: &[StructureRow],
        total_addresses: usize,
    ) -> Vec<DimensionRow> {
        let total = total_addresses as f64;
        let points: Vec<_> = prefix_lengths
            .iter()
            .map(|&prefix_length| {
                let entropy = counts[usize::from(prefix_length)]
                    .values()
                    .map(|&count| {
                        let probability = count as f64 / total;
                        probability * probability.log2()
                    })
                    .sum::<f64>();
                (-(f64::from(prefix_length)), entropy)
            })
            .collect();
        let point_count = points.len() as f64;
        let mean_x = points.iter().map(|point| point.0).sum::<f64>() / point_count;
        let mean_y = points.iter().map(|point| point.1).sum::<f64>() / point_count;
        let denominator = points
            .iter()
            .map(|point| (point.0 - mean_x).powi(2))
            .sum::<f64>();
        let info = if denominator == 0.0 {
            0.0
        } else {
            points
                .iter()
                .map(|point| (point.0 - mean_x) * (point.1 - mean_y))
                .sum::<f64>()
                / denominator
        };
        let at = |q: f64| {
            structure
                .iter()
                .find(|row| row.q == q)
                .copied()
                .expect("reference q grids contain q=0 and q=2")
        };
        let (q0, q2) = (at(0.0), at(2.0));
        vec![
            DimensionRow {
                q: 0.0,
                dim: -q0.tau_tilde,
                sd: q0.sd,
            },
            DimensionRow {
                q: 1.0,
                dim: info,
                sd: 0.0,
            },
            DimensionRow {
                q: 2.0,
                dim: q2.tau_tilde,
                sd: q2.sd,
            },
        ]
    }

    fn assert_matches_reference<A: MaadAddress>(addresses: Vec<A>)
    where
        A::Bits: Into<u128>,
    {
        let config = A::default_config();
        let result = compute_with_config(addresses.clone(), config).unwrap();
        let reference = reference_compute(addresses, config);
        assert_eq!(result.metadata, reference.metadata);
        assert_eq!(result.structure.len(), reference.structure.len());
        assert_eq!(result.spectrum.len(), reference.spectrum.len());
        assert_eq!(result.dimensions.len(), reference.dimensions.len());
        for (actual, expected) in result.structure.iter().zip(reference.structure) {
            close(actual.q, expected.q);
            close(actual.tau_tilde, expected.tau_tilde);
            close(actual.sd, expected.sd);
        }
        for (actual, expected) in result.spectrum.iter().zip(reference.spectrum) {
            close(actual.alpha, expected.alpha);
            close(actual.f, expected.f);
        }
        for (actual, expected) in result.dimensions.iter().zip(reference.dimensions) {
            close(actual.q, expected.q);
            close(actual.dim, expected.dim);
            close(actual.sd, expected.sd);
        }
    }

    #[test]
    fn optimized_path_matches_the_ordered_map_reference() {
        let dense: Vec<_> = (0..=255)
            .map(|last| Ipv4Addr::new(10, 0, 0, last))
            .collect();
        let mut random = Vec::new();
        let mut state = 0x9e37_79b9_u32;
        for _ in 0..256 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            random.push(Ipv4Addr::from(state));
        }
        let duplicate_and_boundaries = vec![
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(192, 0, 2, 2),
        ];
        for addresses in [
            Vec::new(),
            vec![Ipv4Addr::LOCALHOST],
            dense,
            random,
            duplicate_and_boundaries,
        ] {
            assert_matches_reference(addresses);
        }
    }

    fn documentation_ipv6(bits: u128) -> Ipv6Addr {
        Ipv6Addr::from(u128::from(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0)) | bits)
    }

    #[test]
    fn ipv6_optimized_path_matches_the_ordered_map_reference() {
        let dense_subnets: Vec<_> = (0..=255_u128)
            .map(|subnet| documentation_ipv6(subnet << 64))
            .collect();
        let mut clustered = Vec::new();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for site in 0..8_u128 {
            for _ in 0..64 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let subnet = u128::from(state >> 60);
                clustered.push(documentation_ipv6(
                    (site << 80) | (subnet << 64) | u128::from(state),
                ));
            }
        }
        let duplicate_and_boundaries = vec![
            Ipv6Addr::UNSPECIFIED,
            Ipv6Addr::from(u128::MAX),
            documentation_ipv6(1),
            documentation_ipv6(1),
            documentation_ipv6(2),
        ];
        for addresses in [
            Vec::new(),
            vec![Ipv6Addr::LOCALHOST],
            dense_subnets,
            clustered,
            duplicate_and_boundaries,
        ] {
            assert_matches_reference(addresses);
        }
    }

    #[test]
    fn ipv6_uses_upstream_default_prefix_range() {
        let dense_subnets = (0..=255_u128).map(|subnet| documentation_ipv6(subnet << 64));

        let result = compute(dense_subnets);

        assert_eq!(result.metadata.total_addrs, 256);
        assert_eq!(
            result.metadata.prefix_lengths,
            (23..=63).collect::<Vec<_>>()
        );
        assert_eq!(result.structure.len(), 33);
        assert_eq!(
            result
                .dimensions
                .iter()
                .map(|row| row.q)
                .collect::<Vec<_>>(),
            vec![0.0, 1.0, 2.0]
        );
    }

    #[test]
    fn ipv6_nearly_full_threshold_uses_128_bit_capacity() {
        let full_slash_120 = (0..=255_u128).map(documentation_ipv6);
        let config = MaadConfig {
            min_prefix_length: 119,
            max_prefix_length: 121,
            ..MaadConfig::ipv6()
        };

        let result = compute_with_config(full_slash_120, config).unwrap();

        assert_eq!(result.metadata.prefix_lengths, vec![119]);
    }

    #[test]
    fn prefix_range_is_bounded_by_the_address_width() {
        let ipv4 = [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)];
        let ipv6 = [documentation_ipv6(1), documentation_ipv6(2)];
        let widest_ipv6 = MaadConfig {
            min_prefix_length: 23,
            max_prefix_length: 127,
            ..MaadConfig::ipv6()
        };

        assert_eq!(
            compute_with_config(ipv4, MaadConfig::ipv6()),
            Err(MaadError::InvalidPrefixRange {
                min_prefix_length: 23,
                max_prefix_length: 64,
                address_bits: 32,
            })
        );
        assert!(compute_with_config(ipv6, widest_ipv6).is_ok());
        assert!(
            compute_with_config(
                ipv6,
                MaadConfig {
                    max_prefix_length: 128,
                    ..widest_ipv6
                }
            )
            .is_err()
        );
    }

    #[test]
    fn empty_singleton_and_duplicate_sets_have_empty_results() {
        let empty = compute(std::iter::empty::<Ipv4Addr>());
        let singleton = compute([Ipv4Addr::new(192, 0, 2, 1)]);
        let duplicate = compute([Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 1)]);

        assert_eq!(empty.metadata.total_addrs, 0);
        assert_eq!(singleton.metadata.total_addrs, 1);
        assert_eq!(duplicate.metadata.total_addrs, 1);
        for result in [empty, singleton, duplicate] {
            assert_eq!(result.metadata.min_prefix_length, None);
            assert!(result.structure.is_empty());
            assert!(result.spectrum.is_empty());
            assert!(result.dimensions.is_empty());
        }
    }

    #[test]
    fn dense_set_matches_the_python_numerical_oracle() {
        let addresses = (0..2)
            .flat_map(|third| (0..=255).map(move |fourth| Ipv4Addr::new(10, 0, third, fourth)))
            .chain([Ipv4Addr::new(192, 0, 2, 1)]);

        let result = compute(addresses);

        assert_eq!(result.metadata.total_addrs, 513);
        assert_eq!(result.metadata.prefix_lengths, (8..=22).collect::<Vec<_>>());
        assert_eq!(result.metadata.min_prefix_length, Some(8));
        assert_eq!(result.metadata.max_prefix_length, Some(22));
        assert_eq!(result.structure.len(), 33);
        close(result.structure[0].q, -0.5);
        close(result.structure[0].tau_tilde, 0.0);
        close(result.structure[16].q, 1.5);
        close(result.structure[32].tau_tilde, 0.0);
        assert_eq!(result.spectrum.len(), 8);
        for row in &result.spectrum {
            close(row.alpha, 0.0);
            close(row.f, 0.0);
        }
        assert_eq!(result.dimensions.len(), 3);
        close(result.dimensions[0].q, 0.0);
        close(result.dimensions[0].dim, 0.0);
        close(result.dimensions[1].q, 1.0);
        close(result.dimensions[1].dim, 0.0);
        close(result.dimensions[1].sd, 0.0);
        close(result.dimensions[2].q, 2.0);
        close(result.dimensions[2].dim, 0.0);
    }

    #[test]
    fn linear_structure_curve_collapses_to_one_spectrum_point() {
        let addresses = (0..1024).map(|index| Ipv4Addr::from(index << 22));

        let result = compute(addresses);

        assert!(!result.spectrum.is_empty());
        for row in &result.spectrum {
            assert!((row.alpha - 1.0).abs() < 1e-9, "{row:?}");
            assert!((row.f - 1.0).abs() < 1e-9, "{row:?}");
        }
    }

    fn structure_curve(tau: impl Fn(f64) -> f64) -> Vec<StructureRow> {
        (0..=32)
            .map(|index| {
                let q = -0.5 + f64::from(index) * 0.125;
                StructureRow {
                    q,
                    tau_tilde: tau(q),
                    sd: 0.0,
                }
            })
            .collect()
    }

    #[test]
    fn exactly_tied_alphas_stay_in_the_spectrum() {
        let structure = structure_curve(|q| q - 1.0);

        let spectrum = compute_spectrum(&structure, 0.125);

        assert_eq!(compute_critical_region(&structure, 0.125), (3.375, -0.375));
        assert_eq!(spectrum.len(), 30);
        assert!(
            spectrum
                .iter()
                .all(|row| *row == SpectrumRow { alpha: 1.0, f: 1.0 })
        );
    }

    #[test]
    fn spectrum_stops_at_the_critical_region_q_max() {
        let structure = structure_curve(|q| if q <= 2.0 { q - 1.0 } else { 1.0 });

        let spectrum = compute_spectrum(&structure, 0.125);

        assert_eq!(compute_critical_region(&structure, 0.125), (1.875, -0.375));
        assert_eq!(spectrum.len(), 18);
        assert_eq!(spectrum.last(), Some(&SpectrumRow { alpha: 1.0, f: 1.0 }));
    }

    #[test]
    fn sparse_two_address_set_is_safe_at_the_32_bit_boundary() {
        let result = compute([Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)]);

        assert_eq!(result.metadata.prefix_lengths, (8..=24).collect::<Vec<_>>());
        assert_eq!(result.metadata.min_prefix_length, Some(8));
        assert_eq!(result.metadata.max_prefix_length, Some(24));
        assert_eq!(result.structure.len(), 33);
        assert!(result.structure.iter().all(|row| row.q.is_finite()));
        assert!(
            result
                .structure
                .iter()
                .all(|row| row.tau_tilde.is_finite() && row.sd.is_finite())
        );
    }

    #[test]
    fn json_uses_the_established_maad_contract() {
        let mut output = Vec::new();
        write_json(&compute(std::iter::empty::<Ipv4Addr>()), &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "{\"schemaVersion\":3,\"metadata\":{\"input\":\"-\",\"prefixLengths\":[],\"minPrefixLength\":null,\"maxPrefixLength\":null,\"totalAddrs\":0},\"structure\":[],\"spectrum\":[],\"dimensions\":[]}\n"
        );
    }

    #[test]
    fn default_config_uses_valid_prefixes_in_the_candidate_range() {
        let addresses = (0..2)
            .flat_map(|third| (0..=255).map(move |fourth| Ipv4Addr::new(10, 0, third, fourth)))
            .chain([Ipv4Addr::new(192, 0, 2, 1)]);

        let result = compute(addresses);

        assert_eq!(result.metadata.prefix_lengths, (8..=22).collect::<Vec<_>>());
        assert_eq!(result.metadata.min_prefix_length, Some(8));
        assert_eq!(result.metadata.max_prefix_length, Some(22));
        assert_eq!(result.structure.len(), 33);
    }

    #[test]
    fn configurable_q_grid_is_uniform_and_includes_dimension_qs() {
        let config = MaadConfig {
            q_min: -1.0,
            q_max: 2.0,
            q_step: 0.5,
            ..MaadConfig::default()
        };
        let result = compute_with_config(
            [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)],
            config,
        )
        .unwrap();

        let qs: Vec<_> = result.structure.iter().map(|row| row.q).collect();
        assert_eq!(qs, vec![-1.0, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0]);
        assert_eq!(
            result
                .dimensions
                .iter()
                .map(|row| row.q)
                .collect::<Vec<_>>(),
            vec![0.0, 1.0, 2.0]
        );
        let structure_sd = |q: f64| result.structure.iter().find(|row| row.q == q).unwrap().sd;
        assert_eq!(result.dimensions[0].sd, structure_sd(0.0));
        assert_eq!(result.dimensions[1].sd, 0.0);
        assert_eq!(result.dimensions[2].sd, structure_sd(2.0));
    }

    #[test]
    fn invalid_configuration_returns_a_typed_error() {
        let cases = [
            MaadConfig {
                q_step: 0.0,
                ..MaadConfig::default()
            },
            MaadConfig {
                q_min: 1.0,
                q_max: 0.0,
                ..MaadConfig::default()
            },
            MaadConfig {
                q_min: -0.25,
                q_max: 2.0,
                q_step: 0.5,
                ..MaadConfig::default()
            },
            MaadConfig {
                min_prefix_length: 25,
                max_prefix_length: 24,
                ..MaadConfig::default()
            },
            MaadConfig {
                min_prefix_length: 24,
                max_prefix_length: 24,
                ..MaadConfig::default()
            },
            MaadConfig {
                full_threshold: 1.0,
                ..MaadConfig::default()
            },
        ];

        for config in cases {
            assert!(
                compute_with_config(
                    [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)],
                    config,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn q_grid_rejects_unbounded_output_and_missing_required_values() {
        let too_many_values = MaadConfig {
            q_min: 0.0,
            q_max: 2.0,
            q_step: 0.001,
            ..MaadConfig::default()
        };
        let rounded_but_absent_q2 = MaadConfig {
            q_min: -1e13,
            q_max: 1e13,
            q_step: 5e12,
            ..MaadConfig::default()
        };

        assert!(matches!(
            compute_with_config::<Ipv4Addr>([], too_many_values),
            Err(MaadError::QGridTooLarge { .. })
        ));
        assert_eq!(
            compute_with_config::<Ipv4Addr>([], rounded_but_absent_q2),
            Err(MaadError::RequiredQNotOnGrid { q: 2.0 })
        );
    }

    #[test]
    fn one_sided_nearly_full_parents_are_removed_before_children_are_prepared() {
        let nearly_full = (0..200).map(|last| Ipv4Addr::new(10, 0, 0, last));
        let valid = [Ipv4Addr::new(10, 0, 1, 0), Ipv4Addr::new(10, 0, 1, 1)];
        let addresses = nearly_full.chain(valid);
        let config = MaadConfig {
            q_min: 0.0,
            q_max: 2.0,
            q_step: 1.0,
            min_prefix_length: 24,
            max_prefix_length: 25,
            ..MaadConfig::default()
        };

        let result = compute_with_config(addresses, config).unwrap();

        assert_eq!(result.metadata.prefix_lengths, vec![24, 25]);
        close(result.structure[0].tau_tilde, 0.0);
        close(result.structure[2].tau_tilde, 0.0);
    }

    #[test]
    fn sparse_sets_with_no_valid_candidate_level_are_safe() {
        let config = MaadConfig::default();
        let result = compute_with_config(
            [Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(192, 0, 2, 1)],
            config,
        )
        .unwrap();

        assert_eq!(result.metadata.total_addrs, 2);
        assert!(result.metadata.prefix_lengths.is_empty());
        assert!(result.metadata.min_prefix_length.is_none());
        assert!(result.structure.is_empty());
        assert!(result.dimensions.is_empty());
    }

    #[test]
    fn full_branching_ancestors_prune_their_descendants() {
        let addresses = (0..8)
            .chain(8..14)
            .map(|last| Ipv4Addr::new(192, 0, 2, last));
        let config = MaadConfig {
            min_prefix_length: 27,
            max_prefix_length: 29,
            ..MaadConfig::default()
        };

        let result = compute_with_config(addresses, config).unwrap();

        assert_eq!(result.metadata.prefix_lengths, vec![27]);
        assert_eq!(result.metadata.min_prefix_length, Some(27));
        assert_eq!(result.metadata.max_prefix_length, Some(27));
    }

    #[test]
    fn duplicate_addresses_do_not_change_a_non_empty_result() {
        let unique = [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)];
        let duplicate = [unique[0], unique[0], unique[1], unique[1], unique[1]];

        assert_eq!(compute(unique), compute(duplicate));
    }

    #[test]
    fn structure_sd_uses_upstream_root_mean_square_scaling() {
        let addresses = [
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(10, 0, 0, 3),
            Ipv4Addr::new(10, 0, 1, 1),
            Ipv4Addr::new(10, 0, 1, 2),
            Ipv4Addr::new(10, 0, 1, 129),
        ];
        let config = MaadConfig {
            q_min: 0.0,
            q_max: 2.0,
            q_step: 1.0,
            min_prefix_length: 23,
            max_prefix_length: 24,
            ..MaadConfig::default()
        };

        let result = compute_with_config(addresses, config).unwrap();
        let q2 = result.structure.iter().find(|row| row.q == 2.0).unwrap();

        close(q2.sd, (2.0 / 49.0_f64).sqrt() / 2.0);
    }

    fn pseudo_random_addresses(count: usize) -> Vec<Ipv4Addr> {
        let mut state = 0x2545_f491_u32;
        (0..count)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                Ipv4Addr::from(state)
            })
            .collect()
    }

    #[test]
    fn unit_weights_reproduce_the_unweighted_result_exactly() {
        let dense: Vec<_> = (0..2)
            .flat_map(|third| (0..=255).map(move |fourth| Ipv4Addr::new(10, 0, third, fourth)))
            .chain([Ipv4Addr::new(192, 0, 2, 1)])
            .collect();
        let clustered: Vec<_> = (0..64)
            .map(|index| Ipv4Addr::new(10, 1, index / 4, index * 3))
            .chain(pseudo_random_addresses(512))
            .collect();

        for addresses in [dense, clustered, pseudo_random_addresses(1024)] {
            let unweighted = compute(addresses.iter().copied());
            let weighted =
                compute_weighted(addresses.iter().map(|&address| (address, 1.0))).unwrap();

            assert!(!unweighted.structure.is_empty());
            assert_eq!(weighted.metadata, unweighted.metadata);
            assert_eq!(weighted.structure, unweighted.structure);
            assert_eq!(weighted.dimensions, unweighted.dimensions);
            assert!(weighted.spectrum.is_empty());
        }
    }

    #[test]
    fn ipv6_unit_weights_reproduce_the_unweighted_result_exactly() {
        let addresses: Vec<_> = (0..512_u128)
            .map(|index| documentation_ipv6((index % 37) << 72 | (index * 7919) << 40))
            .collect();
        let unweighted = compute(addresses.iter().copied());
        let weighted = compute_weighted(addresses.iter().map(|&address| (address, 1.0))).unwrap();

        assert!(!unweighted.structure.is_empty());
        assert_eq!(unweighted.metadata.min_prefix_length, Some(23));
        assert_eq!(weighted.metadata, unweighted.metadata);
        assert_eq!(weighted.structure, unweighted.structure);
        assert_eq!(weighted.dimensions, unweighted.dimensions);
        assert!(weighted.spectrum.is_empty());
    }

    #[test]
    fn shared_measures_equal_separate_computations() {
        let v4: Vec<_> = pseudo_random_addresses(2_000)
            .into_iter()
            .chain((0..=255).map(|last| Ipv4Addr::new(198, 51, 100, last)))
            .enumerate()
            .map(|(index, address)| {
                let index = index as u32;
                (
                    address,
                    [f64::from(index % 7 + 1), f64::from(index % 5 * 1_500 + 40)],
                )
            })
            .collect();
        let (addresses, [packets, bytes]) = compute_measures(v4.iter().copied()).unwrap();
        assert!(!addresses.spectrum.is_empty());
        assert_eq!(addresses, compute(v4.iter().map(|&(address, _)| address)));
        assert_eq!(
            packets,
            compute_weighted(v4.iter().map(|&(address, weights)| (address, weights[0]))).unwrap()
        );
        assert_eq!(
            bytes,
            compute_weighted(v4.iter().map(|&(address, weights)| (address, weights[1]))).unwrap()
        );

        let v6: Vec<_> = (0..1_024_u128)
            .map(|index| {
                (
                    documentation_ipv6((index % 37) << 72 | (index * 7919) << 40),
                    [(index % 3 + 1) as f64 * 0.5, 1e12 + index as f64],
                )
            })
            .collect();
        let (addresses, [first, second]) = compute_measures(v6.iter().copied()).unwrap();
        assert!(!addresses.structure.is_empty());
        assert_eq!(addresses, compute(v6.iter().map(|&(address, _)| address)));
        assert_eq!(
            first,
            compute_weighted(v6.iter().map(|&(address, weights)| (address, weights[0]))).unwrap()
        );
        assert_eq!(
            second,
            compute_weighted(v6.iter().map(|&(address, weights)| (address, weights[1]))).unwrap()
        );
    }

    #[test]
    fn weighted_result_ignores_order_and_sums_duplicate_addresses() {
        let entries: Vec<_> = pseudo_random_addresses(300)
            .into_iter()
            .chain((0..40).map(|last| Ipv4Addr::new(198, 51, 100, last)))
            .enumerate()
            .map(|(index, address)| (address, f64::from((index % 13) as u32 * 4 + 4)))
            .collect();
        let expected = compute_weighted(entries.iter().copied()).unwrap();

        let mut reversed = entries.clone();
        reversed.reverse();
        let mut rotated = entries.clone();
        rotated.rotate_left(117);
        let split: Vec<_> = entries
            .iter()
            .flat_map(|&(address, weight)| [(address, weight / 4.0), (address, weight * 3.0 / 4.0)])
            .rev()
            .collect();

        assert!(!expected.structure.is_empty());
        assert_eq!(compute_weighted(reversed).unwrap(), expected);
        assert_eq!(compute_weighted(rotated).unwrap(), expected);
        assert_eq!(compute_weighted(split).unwrap(), expected);
    }

    #[test]
    fn weighted_moments_and_entropy_match_a_hand_computed_case() {
        let entries = [
            (Ipv4Addr::new(10, 0, 0, 0), 1.0),
            (Ipv4Addr::new(10, 0, 0, 2), 3.0),
            (Ipv4Addr::new(10, 0, 0, 4), 4.0),
        ];
        let config = MaadConfig {
            q_min: 0.0,
            q_max: 2.0,
            q_step: 1.0,
            min_prefix_length: 29,
            max_prefix_length: 30,
            ..MaadConfig::default()
        };

        let result = compute_weighted_with_config(entries, config).unwrap();

        assert_eq!(result.metadata.total_addrs, 3);
        assert_eq!(result.metadata.prefix_lengths, vec![29, 30]);
        assert!(result.spectrum.is_empty());
        let tau = |q: f64| ((q - 1.0) + (2.0 * q - (1.0 + 3.0_f64.powf(q)).log2())) / 2.0;
        assert_eq!(result.structure.len(), 3);
        for row in &result.structure {
            close(row.tau_tilde, tau(row.q));
            close(row.sd, 0.0);
        }
        let dimensions: Vec<_> = result
            .dimensions
            .iter()
            .map(|row| (row.q, row.dim, row.sd))
            .collect();
        assert_eq!(dimensions.len(), 3);
        close(dimensions[0].0, 0.0);
        close(dimensions[0].1, 1.0);
        close(dimensions[1].0, 1.0);
        close(dimensions[1].1, 1.0);
        close(dimensions[1].2, 0.0);
        close(dimensions[2].0, 2.0);
        close(dimensions[2].1, (5.0 - 10.0_f64.log2()) / 2.0);
    }

    #[test]
    fn weighted_input_rejects_non_positive_and_non_finite_weights() {
        let valid = (Ipv4Addr::new(192, 0, 2, 1), 1.0);
        for weight in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let address = Ipv4Addr::new(192, 0, 2, 2);
            let error = compute_weighted([valid, (address, weight)]).unwrap_err();
            assert!(matches!(
                error,
                MaadError::InvalidWeight { address: rejected, .. } if rejected == IpAddr::V4(address)
            ));
        }
        assert!(matches!(
            compute_weighted([
                valid,
                (Ipv4Addr::new(192, 0, 2, 2), f64::MAX),
                (valid.0, f64::MAX)
            ]),
            Err(MaadError::NonFiniteTotalWeight { .. })
        ));
    }
}
