//! Per-address Singularity scoring, a port of MAAD's `Singularities.hs`.
//!
//! For each distinct IPv4 address `x`, `alpha(x)` is the OLS slope of
//! `-log2(mu_l(x) / n)` against prefix length `l`, where `mu_l(x)` counts the
//! distinct addresses sharing `x`'s `/l` prefix and `n` is the total distinct
//! address count. Prefix levels stop at the first isolated prefix
//! (`mu == 1`), matching the reference in `vendor/maad/Singularities.hs`.
//!
//! High alpha marks an address in a sparse, isolated region of address
//! space; low alpha marks one that stays inside a dense cluster across many
//! prefix levels. Both tails are anomalous.

use std::io;
use std::net::Ipv4Addr;

/// Fitted singularity exponent for one distinct address.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AddressScore {
    pub address: Ipv4Addr,
    pub alpha: f64,
    pub intercept: f64,
    pub r_squared: f64,
    /// Number of prefix levels used in the regression.
    pub prefix_levels: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct RunningOls {
    count: f64,
    sum_x: f64,
    sum_y: f64,
    sum_xx: f64,
    sum_xy: f64,
    sum_yy: f64,
}

impl RunningOls {
    fn add(self, x: f64, y: f64) -> Self {
        Self {
            count: self.count + 1.0,
            sum_x: self.sum_x + x,
            sum_y: self.sum_y + y,
            sum_xx: self.sum_xx + x * x,
            sum_xy: self.sum_xy + x * y,
            sum_yy: self.sum_yy + y * y,
        }
    }

    /// Fits the accumulated points without branches for degenerate inputs.
    ///
    /// Zero or one point, and a constant ordinate across two or more points,
    /// leave at least one zero denominator. IEEE-754 division therefore
    /// produces `NaN`, matching MAAD's rank-deficient OLS behavior. These
    /// values are intentional and must not be replaced with fallback scores.
    fn fit(self) -> Regression {
        let mean_x = self.sum_x / self.count;
        let mean_y = self.sum_y / self.count;
        let sxx = self.sum_xx - self.count * mean_x * mean_x;
        let sxy = self.sum_xy - self.count * mean_x * mean_y;
        let syy = self.sum_yy - self.count * mean_y * mean_y;
        let alpha = sxy / sxx;

        Regression {
            alpha,
            intercept: mean_y - alpha * mean_x,
            r_squared: (sxy * sxy) / (sxx * syy),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Regression {
    alpha: f64,
    intercept: f64,
    r_squared: f64,
}

/// Score every distinct address in `addresses` (duplicates are ignored).
/// Returns scores sorted by ascending alpha, ties broken by address, matching
/// the reference ordering.
pub fn score(addresses: Vec<Ipv4Addr>) -> Vec<AddressScore> {
    let mut addresses: Vec<u32> = addresses.into_iter().map(u32::from).collect();
    addresses.sort_unstable();
    addresses.dedup();

    if addresses.is_empty() {
        return Vec::new();
    }

    let mut scores = Vec::with_capacity(addresses.len());
    visit_prefix(
        &addresses,
        0,
        RunningOls::default(),
        &mut scores,
        addresses.len() as f64,
    );
    scores.sort_by(|left, right| {
        left.alpha
            .total_cmp(&right.alpha)
            .then_with(|| left.address.cmp(&right.address))
    });
    scores
}

fn visit_prefix(
    addresses: &[u32],
    level: u8,
    running: RunningOls,
    scores: &mut Vec<AddressScore>,
    total: f64,
) {
    if addresses.len() == 1 {
        let regression = running.fit();
        scores.push(AddressScore {
            address: Ipv4Addr::from(addresses[0]),
            alpha: regression.alpha,
            intercept: regression.intercept,
            r_squared: regression.r_squared,
            prefix_levels: level,
        });
        return;
    }

    let ordinate = -((addresses.len() as f64) / total).log2();
    let running = running.add(f64::from(level), ordinate);

    if level < 32 {
        let bit = 31 - u32::from(level);
        let split = addresses.partition_point(|address| (address >> bit) & 1 == 0);

        if split > 0 {
            visit_prefix(&addresses[..split], level + 1, running, scores, total);
        }
        if split < addresses.len() {
            visit_prefix(&addresses[split..], level + 1, running, scores, total);
        }
    }
}

/// Write scores as CSV with an `addr,alpha,intercept,r2,n_levels` header.
pub fn write_csv(scores: &[AddressScore], mut output: impl io::Write) -> io::Result<()> {
    writeln!(output, "addr,alpha,intercept,r2,n_levels")?;
    for score in scores {
        writeln!(
            output,
            "{},{},{},{},{}",
            score.address, score.alpha, score.intercept, score.r_squared, score.prefix_levels
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-9;

    #[test]
    fn scores_hand_verified_prefix_fixture() {
        let scores = score(vec![
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(32, 0, 0, 0),
            Ipv4Addr::new(128, 0, 0, 0),
            Ipv4Addr::new(192, 0, 0, 0),
        ]);

        let expected = [
            (Ipv4Addr::new(0, 0, 0, 0), 0.5, 1.0 / 6.0, 0.75, 3),
            (Ipv4Addr::new(32, 0, 0, 0), 0.5, 1.0 / 6.0, 0.75, 3),
            (Ipv4Addr::new(128, 0, 0, 0), 1.0, 0.0, 1.0, 2),
            (Ipv4Addr::new(192, 0, 0, 0), 1.0, 0.0, 1.0, 2),
        ];

        assert_eq!(scores.len(), expected.len());
        for (actual, (address, alpha, intercept, r_squared, prefix_levels)) in
            scores.iter().zip(expected)
        {
            assert_eq!(actual.address, address);
            assert_close(actual.alpha, alpha);
            assert_close(actual.intercept, intercept);
            assert_close(actual.r_squared, r_squared);
            assert_eq!(actual.prefix_levels, prefix_levels);
        }
    }

    #[test]
    fn ignores_duplicate_addresses() {
        let address = Ipv4Addr::new(10, 0, 0, 1);
        let duplicate_only = score(vec![address, address]);
        assert_eq!(duplicate_only.len(), 1);
        assert_eq!(duplicate_only[0].address, address);

        let deduped = vec![
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(32, 0, 0, 0),
            Ipv4Addr::new(128, 0, 0, 0),
            Ipv4Addr::new(192, 0, 0, 0),
        ];
        let mut with_duplicate = deduped.clone();
        with_duplicate.insert(2, Ipv4Addr::new(32, 0, 0, 0));

        assert_eq!(score(with_duplicate), score(deduped));
    }

    #[test]
    fn empty_input_has_no_scores() {
        assert!(score(Vec::new()).is_empty());
    }

    #[test]
    fn single_address_has_degenerate_regression() {
        let address = Ipv4Addr::new(203, 0, 113, 7);
        let scores = score(vec![address]);

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].address, address);
        assert_eq!(scores[0].prefix_levels, 0);
        assert!(scores[0].alpha.is_nan());
        assert!(scores[0].intercept.is_nan());
        assert!(scores[0].r_squared.is_nan());
    }

    #[test]
    fn sorts_three_alpha_groups_before_breaking_ties_by_address() {
        let scores = score(vec![
            Ipv4Addr::new(192, 0, 0, 0),
            Ipv4Addr::new(16, 0, 0, 0),
            Ipv4Addr::new(128, 0, 0, 0),
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(32, 0, 0, 0),
        ]);

        assert_eq!(
            scores.iter().map(|entry| entry.address).collect::<Vec<_>>(),
            vec![
                Ipv4Addr::new(32, 0, 0, 0),
                Ipv4Addr::new(0, 0, 0, 0),
                Ipv4Addr::new(16, 0, 0, 0),
                Ipv4Addr::new(128, 0, 0, 0),
                Ipv4Addr::new(192, 0, 0, 0),
            ]
        );
        assert!(scores[0].alpha < scores[1].alpha);
        assert_close(scores[1].alpha, scores[2].alpha);
        assert!(scores[2].alpha < scores[3].alpha);
        assert_close(scores[3].alpha, scores[4].alpha);
    }

    #[test]
    fn writes_scores_as_csv() {
        let scores = score(vec![
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(32, 0, 0, 0),
            Ipv4Addr::new(128, 0, 0, 0),
            Ipv4Addr::new(192, 0, 0, 0),
        ]);
        let mut csv = Vec::new();

        write_csv(&scores, &mut csv).unwrap();

        let csv = String::from_utf8(csv).unwrap();
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some("addr,alpha,intercept,r2,n_levels"));

        let expected = [
            ("0.0.0.0", 0.5, 1.0 / 6.0, 0.75, 3),
            ("32.0.0.0", 0.5, 1.0 / 6.0, 0.75, 3),
            ("128.0.0.0", 1.0, 0.0, 1.0, 2),
            ("192.0.0.0", 1.0, 0.0, 1.0, 2),
        ];
        for (line, (address, alpha, intercept, r_squared, prefix_levels)) in
            lines.by_ref().zip(expected)
        {
            let fields: Vec<_> = line.split(',').collect();
            assert_eq!(fields.len(), 5);
            assert_eq!(fields[0], address);
            assert_close(fields[1].parse().unwrap(), alpha);
            assert_close(fields[2].parse().unwrap(), intercept);
            assert_close(fields[3].parse().unwrap(), r_squared);
            assert_eq!(fields[4].parse::<u8>().unwrap(), prefix_levels);
        }
        assert_eq!(lines.next(), None);
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= EPSILON,
            "actual={actual:?}, expected={expected:?}"
        );
    }
}
