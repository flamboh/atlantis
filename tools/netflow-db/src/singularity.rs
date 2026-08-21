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

/// Score every distinct address in `addresses` (duplicates are ignored).
/// Returns scores sorted by ascending alpha, ties broken by address, matching
/// the reference ordering.
pub fn score(addresses: Vec<Ipv4Addr>) -> Vec<AddressScore> {
    let _ = addresses;
    todo!("singularity port: implemented by the scoring task")
}

/// Write scores as CSV with an `addr,alpha,intercept,r2,n_levels` header.
pub fn write_csv(scores: &[AddressScore], output: impl io::Write) -> io::Result<()> {
    let _ = (scores, output);
    todo!("singularity port: implemented by the scoring task")
}
