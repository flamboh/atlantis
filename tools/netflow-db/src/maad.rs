//! In-process MAAD-compatible multifractal analysis for IPv4 address sets.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::net::Ipv4Addr;

const MIN_MAAD_ADDRESSES: usize = 2;
const SPILLOVER_THRESHOLD: f64 = 0.05;
const DELTA_Q: f64 = 1.0 / 8.0;
const MIN_Q: f64 = -0.5;
const MAX_Q: f64 = 3.5;

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
    parent_counts: Vec<f64>,
    child_counts: Vec<Vec<f64>>,
}

/// Compute MAAD-compatible output from an IPv4 address set.
pub fn compute(addresses: impl IntoIterator<Item = Ipv4Addr>) -> MaadResult {
    let addresses: BTreeSet<u32> = addresses.into_iter().map(u32::from).collect();
    if addresses.len() < MIN_MAAD_ADDRESSES {
        return empty_result(addresses.len());
    }
    let counts = build_prefix_counts(&addresses);
    let min_prefix_length = first_atomic_length(&counts);
    let max_prefix_length = first_spillover_length(&counts);
    if min_prefix_length > max_prefix_length {
        return empty_result(addresses.len());
    }
    let prefix_lengths: Vec<_> = (min_prefix_length..=max_prefix_length).collect();
    let prepared = prepare_moments(&counts, &prefix_lengths);
    let structure = compute_structure(&prepared);
    let spectrum = compute_spectrum(&structure);
    let dimensions = compute_dimensions(&counts, &prefix_lengths, &structure, addresses.len());
    MaadResult {
        schema_version: 1,
        metadata: MaadMetadata {
            input: "-",
            min_prefix_length: Some(min_prefix_length),
            max_prefix_length: Some(max_prefix_length),
            total_addrs: addresses.len(),
        },
        structure,
        spectrum,
        dimensions,
    }
}

/// Serialize a computed MAAD result using the established JSON field names.
pub fn write_json<W: Write>(result: &MaadResult, mut output: W) -> Result<(), serde_json::Error> {
    serde_json::to_writer(&mut output, result)?;
    output.write_all(b"\n").map_err(serde_json::Error::io)
}

fn empty_result(total_addrs: usize) -> MaadResult {
    MaadResult {
        schema_version: 1,
        metadata: MaadMetadata {
            input: "-",
            min_prefix_length: None,
            max_prefix_length: None,
            total_addrs,
        },
        structure: Vec::new(),
        spectrum: Vec::new(),
        dimensions: Vec::new(),
    }
}

fn build_prefix_counts(addresses: &BTreeSet<u32>) -> Vec<BTreeMap<u32, usize>> {
    let mut counts = vec![BTreeMap::new(); 33];
    for &address in addresses {
        for prefix_length in 0..=32_u8 {
            let prefix = prefix_of(address, prefix_length);
            *counts[usize::from(prefix_length)]
                .entry(prefix)
                .or_default() += 1;
        }
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

fn first_atomic_length(counts: &[BTreeMap<u32, usize>]) -> u8 {
    (1..=32)
        .find(|&length| counts[length].values().any(|&count| count == 1))
        .map_or(33, |length| length as u8)
}

fn first_spillover_length(counts: &[BTreeMap<u32, usize>]) -> u8 {
    (1..=32)
        .find(|&length| {
            let capacity = 2_f64.powi(32 - length as i32);
            counts[length]
                .values()
                .any(|&count| count as f64 / capacity >= 1.0 - SPILLOVER_THRESHOLD)
        })
        .map_or(33, |length| length as u8)
}

fn prepare_moments(counts: &[BTreeMap<u32, usize>], prefix_lengths: &[u8]) -> Vec<PreparedMoment> {
    prefix_lengths
        .iter()
        .map(|&prefix_length| {
            // A /32 has no child prefix. Treating it as an empty moment extends the
            // estimator safely to small sparse sets, where both legacy ports indexed /33.
            let Some(children) = counts.get(usize::from(prefix_length) + 1) else {
                return PreparedMoment {
                    parent_counts: Vec::new(),
                    child_counts: Vec::new(),
                };
            };
            let mut parent_counts = Vec::new();
            let mut child_counts = Vec::new();
            for (&prefix, &count) in &counts[usize::from(prefix_length)] {
                if count <= 1 {
                    continue;
                }
                let child_counts_for_parent: Vec<_> = [prefix << 1, (prefix << 1) | 1]
                    .into_iter()
                    .filter_map(|child| children.get(&child).copied())
                    .map(|child_count| child_count as f64)
                    .collect();
                if !child_counts_for_parent.is_empty() {
                    parent_counts.push(count as f64);
                    child_counts.push(child_counts_for_parent);
                }
            }
            PreparedMoment {
                parent_counts,
                child_counts,
            }
        })
        .collect()
}

fn one_moment(prepared: &PreparedMoment, q: f64) -> (f64, f64) {
    if prepared.parent_counts.is_empty() {
        return (0.0, 0.0);
    }
    let parent_powers: Vec<_> = prepared
        .parent_counts
        .iter()
        .map(|count| count.powf(q))
        .collect();
    let child_power_sums: Vec<_> = prepared
        .child_counts
        .iter()
        .map(|children| children.iter().map(|count| count.powf(q)).sum::<f64>())
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

fn compute_structure(prepared: &[PreparedMoment]) -> Vec<StructureRow> {
    (0..=((MAX_Q - MIN_Q) / DELTA_Q) as usize)
        .map(|index| {
            let q = MIN_Q + index as f64 * DELTA_Q;
            let (tau_sum, d2_sum) = prepared
                .iter()
                .map(|moment| one_moment(moment, q))
                .fold((0.0, 0.0), |(tau_sum, d2_sum), (tau, d2)| {
                    (tau_sum + tau, d2_sum + d2)
                });
            let count = prepared.len() as f64;
            StructureRow {
                q,
                tau_tilde: tau_sum / count,
                sd: (d2_sum / count).sqrt(),
            }
        })
        .collect()
}

fn compute_spectrum(structure: &[StructureRow]) -> Vec<SpectrumRow> {
    let alphas: Vec<_> = structure
        .windows(3)
        .map(|rows| {
            let row = rows[1];
            let alpha = (rows[2].tau_tilde - rows[0].tau_tilde) / (2.0 * DELTA_Q);
            SpectrumRow {
                alpha,
                f: row.q * alpha - row.tau_tilde,
            }
        })
        .collect();
    let mut rows = Vec::new();
    let mut started = false;
    for pair in alphas.windows(2) {
        let decreasing = pair[0].alpha > pair[1].alpha;
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
    counts: &[BTreeMap<u32, usize>],
    prefix_lengths: &[u8],
    structure: &[StructureRow],
    total_addresses: usize,
) -> Vec<DimensionRow> {
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
    counts: &[BTreeMap<u32, usize>],
    prefix_lengths: &[u8],
    total_addresses: usize,
) -> f64 {
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

    fn close(left: f64, right: f64) {
        assert!((left - right).abs() <= 1e-12, "{left} != {right}");
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
        assert_eq!(result.metadata.min_prefix_length, Some(1));
        assert_eq!(result.metadata.max_prefix_length, Some(23));
        assert_eq!(result.structure.len(), 33);
        close(result.structure[0].q, -0.5);
        close(result.structure[0].tau_tilde, -0.06521739130434782);
        close(result.structure[16].q, 1.5);
        close(result.structure[32].tau_tilde, 0.10869565217391304);
        assert_eq!(result.spectrum.len(), 1);
        close(result.spectrum[0].alpha, 0.043478260869565216);
        close(result.spectrum[0].f, 0.043478260869565216);
        assert_eq!(result.dimensions.len(), 3);
        close(result.dimensions[0].q, 1.0);
        close(result.dimensions[0].dim, 0.0);
        close(result.dimensions[1].q, 0.0);
        close(result.dimensions[1].dim, 0.043478260869565216);
        close(result.dimensions[2].q, 2.0);
        close(result.dimensions[2].dim, 0.043478260869565216);
    }

    #[test]
    fn sparse_two_address_set_is_safe_at_the_32_bit_boundary() {
        let result = compute([Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)]);

        assert_eq!(result.metadata.min_prefix_length, Some(31));
        assert_eq!(result.metadata.max_prefix_length, Some(32));
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
            "{\"schemaVersion\":1,\"metadata\":{\"input\":\"-\",\"minPrefixLength\":null,\"maxPrefixLength\":null,\"totalAddrs\":0},\"structure\":[],\"spectrum\":[],\"dimensions\":[]}\n"
        );
    }
}
