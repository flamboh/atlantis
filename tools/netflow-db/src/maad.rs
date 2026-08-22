//! In-process MAAD-compatible multifractal analysis for IPv4 address sets.

use serde::Serialize;
use std::io::Write;
use std::net::Ipv4Addr;

const MIN_MAAD_ADDRESSES: usize = 2;
const SCHEMA_VERSION: u32 = 2;
const DEFAULT_FULL_THRESHOLD: f64 = 0.05;
const DEFAULT_Q_STEP: f64 = 1.0 / 8.0;
const DEFAULT_Q_MIN: f64 = -0.5;
const DEFAULT_Q_MAX: f64 = 3.5;
const DEFAULT_MIN_PREFIX_LENGTH: u8 = 8;
const DEFAULT_MAX_PREFIX_LENGTH: u8 = 24;
const MAX_PARENT_PREFIX_LENGTH: u8 = 31;
const MAX_Q_VALUES: usize = 1025;
const GRID_EPSILON: f64 = 1e-12;

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

impl Default for MaadConfig {
    fn default() -> Self {
        Self {
            q_min: DEFAULT_Q_MIN,
            q_max: DEFAULT_Q_MAX,
            q_step: DEFAULT_Q_STEP,
            min_prefix_length: DEFAULT_MIN_PREFIX_LENGTH,
            max_prefix_length: DEFAULT_MAX_PREFIX_LENGTH,
            full_threshold: DEFAULT_FULL_THRESHOLD,
        }
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
        "prefix range must satisfy 0 <= min < max <= 31 (min={min_prefix_length}, max={max_prefix_length})"
    )]
    InvalidPrefixRange {
        min_prefix_length: u8,
        max_prefix_length: u8,
    },
    #[error("full_threshold must be finite and in [0, 1) (full_threshold={full_threshold})")]
    InvalidFullThreshold { full_threshold: f64 },
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
}

#[derive(Clone, Debug)]
struct PreparedMoment {
    parent_counts: Vec<usize>,
    child_counts: Vec<Vec<usize>>,
}

/// Compute MAAD-compatible output from an IPv4 address set.
pub fn compute(addresses: impl IntoIterator<Item = Ipv4Addr>) -> MaadResult {
    compute_with_config(addresses, MaadConfig::default())
        .expect("the default MAAD configuration must be valid")
}

/// Compute MAAD-compatible output using an explicitly validated configuration.
pub fn compute_with_config(
    addresses: impl IntoIterator<Item = Ipv4Addr>,
    config: MaadConfig,
) -> Result<MaadResult, MaadError> {
    let q_values = validate_config(&config)?;
    let mut addresses: Vec<_> = addresses.into_iter().map(u32::from).collect();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.len() < MIN_MAAD_ADDRESSES {
        return Ok(empty_result(addresses.len()));
    }
    let counts = build_prefix_counts(&addresses);
    let prepared = prepare_valid_moments(&counts, &config);
    if prepared.is_empty() {
        return Ok(empty_result(addresses.len()));
    }
    let (prefix_lengths, prepared): (Vec<_>, Vec<_>) = prepared.into_iter().unzip();
    let structure = compute_structure(&prepared, &q_values);
    let spectrum = compute_spectrum(&structure, config.q_step);
    let dimensions = compute_dimensions(&counts, &prefix_lengths, &structure, addresses.len());
    Ok(MaadResult {
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
    })
}

/// Serialize a computed MAAD result using the established JSON field names.
pub fn write_json<W: Write>(result: &MaadResult, mut output: W) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut output, result)?;
    output.write_all(b"\n").map_err(serde_json::Error::io)
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

fn build_prefix_counts(addresses: &[u32]) -> Vec<Vec<(u32, usize)>> {
    let mut counts = Vec::with_capacity(33);
    for prefix_length in 0..=32_u8 {
        let mut prefixes = Vec::new();
        for &address in addresses {
            let prefix = prefix_of(address, prefix_length);
            if let Some((last_prefix, count)) = prefixes.last_mut()
                && *last_prefix == prefix
            {
                *count += 1;
            } else {
                prefixes.push((prefix, 1));
            }
        }
        counts.push(prefixes);
    }
    counts
}

const fn prefix_of(address: u32, prefix_length: u8) -> u32 {
    if prefix_length == 0 {
        0
    } else {
        address >> (32 - prefix_length)
    }
}

fn validate_config(config: &MaadConfig) -> Result<Vec<f64>, MaadError> {
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
        || config.max_prefix_length > MAX_PARENT_PREFIX_LENGTH
    {
        return Err(MaadError::InvalidPrefixRange {
            min_prefix_length: config.min_prefix_length,
            max_prefix_length: config.max_prefix_length,
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

fn is_valid_parent(count: usize, prefix_length: u8, full_threshold: f64) -> bool {
    count > 1 && (count as f64).log2() / f64::from(32 - prefix_length) < 1.0 - full_threshold
}

fn prepare_valid_moments(
    counts: &[Vec<(u32, usize)>],
    config: &MaadConfig,
) -> Vec<(u8, PreparedMoment)> {
    let mut prepared = Vec::new();
    let mut path_allowed = vec![true; counts[0].len()];

    for prefix_length in 0..=config.max_prefix_length {
        let parents = &counts[usize::from(prefix_length)];
        let children = &counts[usize::from(prefix_length) + 1];

        if prefix_length >= config.min_prefix_length {
            let moment = prepare_moment_at_length(
                parents,
                children,
                &path_allowed,
                prefix_length,
                config.full_threshold,
            );
            if !moment.parent_counts.is_empty() {
                prepared.push((prefix_length, moment));
            }
        }

        if prefix_length < config.max_prefix_length {
            path_allowed = propagate_allowed_paths(
                parents,
                children,
                &path_allowed,
                prefix_length,
                config.full_threshold,
            );
        }
    }

    prepared
}

fn prepare_moment_at_length(
    parents: &[(u32, usize)],
    children: &[(u32, usize)],
    path_allowed: &[bool],
    prefix_length: u8,
    full_threshold: f64,
) -> PreparedMoment {
    let mut parent_counts = Vec::new();
    let mut child_counts = Vec::new();
    let mut next_child = 0;

    for (parent_index, &(prefix, count)) in parents.iter().enumerate() {
        let first_child = prefix << 1;
        while next_child < children.len() && children[next_child].0 < first_child {
            next_child += 1;
        }
        let child_start = next_child;
        while next_child < children.len() && children[next_child].0 <= first_child | 1 {
            next_child += 1;
        }
        if path_allowed[parent_index]
            && is_valid_parent(count, prefix_length, full_threshold)
            && child_start < next_child
        {
            parent_counts.push(count);
            child_counts.push(
                children[child_start..next_child]
                    .iter()
                    .map(|&(_, child_count)| child_count)
                    .collect(),
            );
        }
    }

    PreparedMoment {
        parent_counts,
        child_counts,
    }
}

fn propagate_allowed_paths(
    parents: &[(u32, usize)],
    children: &[(u32, usize)],
    path_allowed: &[bool],
    prefix_length: u8,
    full_threshold: f64,
) -> Vec<bool> {
    let mut child_path_allowed = Vec::with_capacity(children.len());
    let mut next_child = 0;

    for (parent_index, &(prefix, count)) in parents.iter().enumerate() {
        let first_child = prefix << 1;
        while next_child < children.len() && children[next_child].0 < first_child {
            next_child += 1;
        }
        let child_start = next_child;
        while next_child < children.len() && children[next_child].0 <= first_child | 1 {
            next_child += 1;
        }
        let is_branch = next_child - child_start == 2;
        let allowed = path_allowed[parent_index]
            && (!is_branch || is_valid_parent(count, prefix_length, full_threshold));
        child_path_allowed.extend(std::iter::repeat_n(allowed, next_child - child_start));
    }

    debug_assert_eq!(child_path_allowed.len(), children.len());
    child_path_allowed
}

fn one_moment(prepared: &PreparedMoment, powers: &[f64]) -> (f64, f64) {
    if prepared.parent_counts.is_empty() {
        return (0.0, 0.0);
    }
    let parent_powers: Vec<_> = prepared
        .parent_counts
        .iter()
        .map(|&count| powers[count])
        .collect();
    let child_power_sums: Vec<_> = prepared
        .child_counts
        .iter()
        .map(|children| children.iter().map(|&count| powers[count]).sum::<f64>())
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

fn compute_structure(prepared: &[PreparedMoment], q_values: &[f64]) -> Vec<StructureRow> {
    if prepared.is_empty() {
        return Vec::new();
    }
    let max_count = prepared
        .iter()
        .flat_map(|moment| {
            moment
                .parent_counts
                .iter()
                .chain(moment.child_counts.iter().flatten())
        })
        .copied()
        .max()
        .unwrap_or_default();
    q_values
        .iter()
        .copied()
        .map(|q| {
            let powers: Vec<_> = (0..=max_count)
                .map(|count| (count as f64).powf(q))
                .collect();
            let (tau_sum, d2_sum) = prepared
                .iter()
                .map(|moment| one_moment(moment, &powers))
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

fn compute_spectrum(structure: &[StructureRow], q_step: f64) -> Vec<SpectrumRow> {
    let alphas: Vec<_> = structure
        .windows(3)
        .map(|rows| {
            let row = rows[1];
            let alpha = (rows[2].tau_tilde - rows[0].tau_tilde) / (2.0 * q_step);
            SpectrumRow {
                alpha,
                f: row.q * alpha - row.tau_tilde,
            }
        })
        .collect();
    let mut rows = Vec::new();
    let mut started = false;
    for pair in alphas.windows(2) {
        let decreasing = pair[0].alpha - pair[1].alpha > GRID_EPSILON;
        if !started && !decreasing {
            continue;
        }
        if !decreasing {
            break;
        }
        started = true;
        rows.push(pair[1]);
    }
    rows
}

fn compute_dimensions(
    counts: &[Vec<(u32, usize)>],
    prefix_lengths: &[u8],
    structure: &[StructureRow],
    total_addresses: usize,
) -> Vec<DimensionRow> {
    if prefix_lengths.is_empty() || structure.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![DimensionRow {
        q: 1.0,
        dim: info_dimension(counts, prefix_lengths, total_addresses),
    }];
    rows.extend(
        structure
            .iter()
            .filter(|row| row.q.abs() < 1e-12 || (row.q - 2.0).abs() < 1e-12)
            .map(|row| DimensionRow {
                q: row.q,
                dim: row.tau_tilde / (row.q - 1.0),
            }),
    );
    rows
}

fn info_dimension(
    counts: &[Vec<(u32, usize)>],
    prefix_lengths: &[u8],
    total_addresses: usize,
) -> f64 {
    let total = total_addresses as f64;
    let points: Vec<_> = prefix_lengths
        .iter()
        .map(|&prefix_length| {
            let entropy = counts[usize::from(prefix_length)]
                .iter()
                .map(|&(_, count)| {
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

    fn reference_compute(
        addresses: impl IntoIterator<Item = Ipv4Addr>,
        config: MaadConfig,
    ) -> MaadResult {
        let addresses: BTreeSet<_> = addresses.into_iter().map(u32::from).collect();
        if addresses.len() < MIN_MAAD_ADDRESSES {
            return empty_result(addresses.len());
        }
        let counts = reference_prefix_counts(&addresses);
        let prepared = reference_prepare_valid_moments(&counts, &config);
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

    fn reference_prefix_counts(addresses: &BTreeSet<u32>) -> Vec<BTreeMap<u32, usize>> {
        let mut counts = vec![BTreeMap::new(); 33];
        for &address in addresses {
            for prefix_length in 0..=32_u8 {
                *counts[usize::from(prefix_length)]
                    .entry(prefix_of(address, prefix_length))
                    .or_default() += 1;
            }
        }
        counts
    }

    fn reference_prepare_valid_moments(
        counts: &[BTreeMap<u32, usize>],
        config: &MaadConfig,
    ) -> Vec<(u8, PreparedMoment)> {
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
                        || !reference_valid_parent(count, prefix_length, config.full_threshold)
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
                            parent_counts,
                            child_counts,
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
                                ));
                        (child, allowed)
                    })
                    .collect();
            }
        }

        prepared
    }

    fn reference_valid_parent(count: usize, prefix_length: u8, full_threshold: f64) -> bool {
        count > 1 && (count as f64).log2() / f64::from(32 - prefix_length) < 1.0 - full_threshold
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

    fn reference_structure(prepared: &[PreparedMoment], q_values: &[f64]) -> Vec<StructureRow> {
        q_values
            .iter()
            .copied()
            .map(|q| {
                let (tau_sum, d2_sum) = prepared
                    .iter()
                    .map(|moment| {
                        let parent_powers: Vec<_> = moment
                            .parent_counts
                            .iter()
                            .map(|&count| (count as f64).powf(q))
                            .collect();
                        let child_power_sums: Vec<_> = moment
                            .child_counts
                            .iter()
                            .map(|children| {
                                children
                                    .iter()
                                    .map(|&count| (count as f64).powf(q))
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
        counts: &[BTreeMap<u32, usize>],
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
        let mut rows = vec![DimensionRow { q: 1.0, dim: info }];
        rows.extend(
            structure
                .iter()
                .filter(|row| row.q.abs() < 1e-12 || (row.q - 2.0).abs() < 1e-12)
                .map(|row| DimensionRow {
                    q: row.q,
                    dim: row.tau_tilde / (row.q - 1.0),
                }),
        );
        rows
    }

    fn assert_matches_reference(addresses: Vec<Ipv4Addr>) {
        let config = MaadConfig::default();
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

    #[test]
    fn empty_singleton_and_duplicate_sets_have_empty_results() {
        let empty = compute(std::iter::empty());
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
        assert!(result.spectrum.is_empty());
        assert_eq!(result.dimensions.len(), 3);
        close(result.dimensions[0].q, 1.0);
        close(result.dimensions[0].dim, 0.0);
        close(result.dimensions[1].q, 0.0);
        close(result.dimensions[1].dim, 0.0);
        close(result.dimensions[2].q, 2.0);
        close(result.dimensions[2].dim, 0.0);
    }

    #[test]
    fn linear_structure_curve_has_no_spectrum_rows() {
        let addresses = (0..1024).map(|index| Ipv4Addr::from(index << 22));

        let result = compute(addresses);

        assert!(result.spectrum.is_empty());
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
        write_json(&compute(std::iter::empty()), &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "{\"schemaVersion\":2,\"metadata\":{\"input\":\"-\",\"prefixLengths\":[],\"minPrefixLength\":null,\"maxPrefixLength\":null,\"totalAddrs\":0},\"structure\":[],\"spectrum\":[],\"dimensions\":[]}\n"
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
            vec![1.0, 0.0, 2.0]
        );
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
            compute_with_config([], too_many_values),
            Err(MaadError::QGridTooLarge { .. })
        ));
        assert_eq!(
            compute_with_config([], rounded_but_absent_q2),
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
}
