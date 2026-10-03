use crate::maad::{MIN_MAAD_ADDRESSES, PrefixBits};

#[derive(Clone, Debug, PartialEq)]
pub struct ConcentrationResult {
    pub entry_count: usize,
    pub weight_total: f64,
    pub values: Option<ConcentrationValues>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConcentrationValues {
    pub hhi: f64,
    pub top1_share: f64,
    pub top10_share: f64,
    pub top100_share: f64,
    pub entropy: [f64; 4],
}

pub(crate) fn compute_sorted<B: PrefixBits>(
    entries: impl Iterator<Item = (B, f64)> + Clone,
    uniform: bool,
) -> ConcentrationResult {
    let mut entry_count = 0;
    let mut weight_total = 0.0;
    for (_, weight) in entries.clone().filter(|(_, weight)| *weight > 0.0) {
        entry_count += 1;
        weight_total += weight;
    }
    let values = (entry_count >= MIN_MAAD_ADDRESSES).then(|| {
        let (hhi, top) = if uniform {
            (
                1.0 / weight_total,
                [1, 10, 100].map(|k| k.min(entry_count) as f64 / weight_total),
            )
        } else {
            let mut weights = Vec::with_capacity(entry_count);
            let mut hhi = 0.0;
            for (_, weight) in entries.clone().filter(|(_, weight)| *weight > 0.0) {
                let share = weight / weight_total;
                hhi += share * share;
                weights.push(weight);
            }
            if weights.len() > 100 {
                weights.select_nth_unstable_by(99, |a, b| b.total_cmp(a));
                weights.truncate(100);
            }
            weights.sort_unstable_by(|a, b| b.total_cmp(a));
            (
                hhi.min(1.0),
                [1, 10, 100].map(|k| {
                    if k >= entry_count {
                        1.0
                    } else {
                        (weights.iter().take(k).sum::<f64>() / weight_total).min(1.0)
                    }
                }),
            )
        };
        let prefix_lengths = if B::WIDTH == 32 {
            [8, 16, 24, 32]
        } else {
            [32, 48, 64, 128]
        };
        let entropy = prefix_lengths.map(|length| {
            let mut current = None;
            let mut mass = 0.0;
            let mut entropy = 0.0;
            let contribution = |mass: f64| {
                let share = mass / weight_total;
                if share > 0.0 {
                    -share * share.log2()
                } else {
                    0.0
                }
            };
            for (address, weight) in entries.clone().filter(|(_, weight)| *weight > 0.0) {
                let prefix = address.prefix(length);
                if current.is_some_and(|current| current != prefix) {
                    entropy += contribution(mass);
                    mass = 0.0;
                }
                current = Some(prefix);
                mass += weight;
            }
            entropy + contribution(mass)
        });
        ConcentrationValues {
            hhi,
            top1_share: top[0],
            top10_share: top[1],
            top100_share: top[2],
            entropy,
        }
    });
    ConcentrationResult {
        entry_count,
        weight_total,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn ipv4_small_set_matches_hand_computed_values() {
        let entries = [
            (u32::from(Ipv4Addr::new(10, 1, 1, 1)), 1.0),
            (u32::from(Ipv4Addr::new(10, 1, 1, 2)), 1.0),
            (u32::from(Ipv4Addr::new(10, 1, 2, 1)), 2.0),
            (u32::from(Ipv4Addr::new(11, 1, 1, 1)), 4.0),
        ];
        let result = compute_sorted(entries.into_iter(), false);
        assert_eq!(result.entry_count, 4);
        close(result.weight_total, 8.0);
        let values = result.values.unwrap();
        close(values.hhi, 22.0 / 64.0);
        close(values.top1_share, 0.5);
        close(values.top10_share, 1.0);
        close(values.top100_share, 1.0);
        for (actual, expected) in values.entropy.into_iter().zip([1.0, 1.0, 1.5, 1.75]) {
            close(actual, expected);
        }
        let uniform = compute_sorted(entries.into_iter().map(|(a, _)| (a, 1.0)), true)
            .values
            .unwrap();
        close(uniform.hhi, 0.25);
        close(uniform.top1_share, 0.25);
        for (actual, expected) in
            uniform
                .entropy
                .into_iter()
                .zip([0.8112781244591328, 0.8112781244591328, 1.5, 2.0])
        {
            close(actual, expected);
        }
    }

    #[test]
    fn ipv6_fixed_prefixes_include_full_address_entropy() {
        let entries = [
            ("2001:db8:1:1::1", 1.0),
            ("2001:db8:1:1::2", 1.0),
            ("2001:db8:1:2::1", 2.0),
            ("2001:db9:1:1::1", 4.0),
        ]
        .map(|(a, w)| (u128::from(a.parse::<Ipv6Addr>().unwrap()), w));
        let result = compute_sorted(entries.into_iter(), false);
        let values = result.values.unwrap();
        close(values.hhi, 22.0 / 64.0);
        for (actual, expected) in values.entropy.into_iter().zip([1.0, 1.0, 1.5, 1.75]) {
            close(actual, expected);
        }
    }

    #[test]
    fn zero_weights_are_excluded_and_small_sets_keep_audit_fields() {
        let values = compute_sorted([(0u32, 0.0), (1, 2.0), (2, 2.0)].into_iter(), false);
        assert_eq!(values.entry_count, 2);
        close(values.weight_total, 4.0);
        close(values.values.unwrap().hhi, 0.5);
        for entries in [vec![], vec![(0u32, 0.0)], vec![(0u32, 0.0), (1, 9.0)]] {
            let result = compute_sorted(entries.iter().copied(), false);
            assert!(result.values.is_none());
            assert_eq!(result.entry_count, usize::from(result.weight_total > 0.0));
        }
    }

    #[test]
    fn partial_selection_finds_top_k_without_sorting_all_weights() {
        let entries = (1u32..=200).map(|a| (a, a as f64)).collect::<Vec<_>>();
        let values = compute_sorted(entries.iter().copied(), false)
            .values
            .unwrap();
        close(values.top1_share, 200.0 / 20100.0);
        close(values.top10_share, 1955.0 / 20100.0);
        close(values.top100_share, 15050.0 / 20100.0);
        close(values.hhi, 2686700.0 / (20100.0 * 20100.0));
    }
}
