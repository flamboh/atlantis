//! Capture coverage for a canonical source and time bucket.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The completeness of capture evidence for a bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageState {
    Complete,
    Partial,
    Unknown,
}

/// Additive counts of expected, observed, and rejected source-bucket units.
///
/// A unit is one physical capture member for native input, or one resolved
/// logical source for CSV input, over one five-minute interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketCoverage {
    expected_units: u64,
    observed_units: u64,
    rejected_units: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CoverageError {
    #[error("bucket coverage must have at least one expected unit")]
    NoExpectedUnits,
    #[error("observed coverage units cannot exceed expected units")]
    TooManyObservedUnits,
    #[error("rejected coverage units cannot exceed expected units")]
    TooManyRejectedUnits,
    #[error("coverage unit count overflow")]
    UnitOverflow,
}

impl BucketCoverage {
    pub fn new(
        expected_units: u64,
        observed_units: u64,
        rejected_units: u64,
    ) -> Result<Self, CoverageError> {
        let coverage = Self {
            expected_units,
            observed_units,
            rejected_units,
        };
        coverage.validate()?;
        Ok(coverage)
    }

    /// Identity value used while constructing an aggregate bucket.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            expected_units: 0,
            observed_units: 0,
            rejected_units: 0,
        }
    }

    #[must_use]
    pub const fn complete_unit() -> Self {
        Self {
            expected_units: 1,
            observed_units: 1,
            rejected_units: 0,
        }
    }

    #[must_use]
    pub const fn expected_units(self) -> u64 {
        self.expected_units
    }

    #[must_use]
    pub const fn observed_units(self) -> u64 {
        self.observed_units
    }

    #[must_use]
    pub const fn rejected_units(self) -> u64 {
        self.rejected_units
    }

    #[must_use]
    pub const fn state(self) -> CoverageState {
        if self.expected_units == 0 {
            CoverageState::Unknown
        } else if self.observed_units == self.expected_units && self.rejected_units == 0 {
            CoverageState::Complete
        } else if self.observed_units == 0 && self.rejected_units == 0 {
            CoverageState::Unknown
        } else {
            CoverageState::Partial
        }
    }

    pub fn include(&mut self, child: Self) -> Result<(), CoverageError> {
        let expected_units = self
            .expected_units
            .checked_add(child.expected_units)
            .ok_or(CoverageError::UnitOverflow)?;
        let observed_units = self
            .observed_units
            .checked_add(child.observed_units)
            .ok_or(CoverageError::UnitOverflow)?;
        let rejected_units = self
            .rejected_units
            .checked_add(child.rejected_units)
            .ok_or(CoverageError::UnitOverflow)?;
        self.expected_units = expected_units;
        self.observed_units = observed_units;
        self.rejected_units = rejected_units;
        Ok(())
    }

    fn validate(self) -> Result<(), CoverageError> {
        if self.expected_units == 0 {
            return Err(CoverageError::NoExpectedUnits);
        }
        if self.observed_units > self.expected_units {
            return Err(CoverageError::TooManyObservedUnits);
        }
        if self.rejected_units > self.expected_units {
            return Err(CoverageError::TooManyRejectedUnits);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BucketCoverage, CoverageState};

    #[test]
    fn coverage_state_distinguishes_unknown_partial_and_complete() {
        assert_eq!(
            BucketCoverage::new(2, 0, 0).unwrap().state(),
            CoverageState::Unknown
        );
        assert_eq!(
            BucketCoverage::new(2, 1, 0).unwrap().state(),
            CoverageState::Partial
        );
        assert_eq!(
            BucketCoverage::new(2, 2, 1).unwrap().state(),
            CoverageState::Partial
        );
        assert_eq!(
            BucketCoverage::new(2, 2, 0).unwrap().state(),
            CoverageState::Complete
        );
    }

    #[test]
    fn coverage_rollups_add_source_bucket_units() {
        let mut coverage = BucketCoverage::empty();
        coverage
            .include(BucketCoverage::new(2, 1, 0).unwrap())
            .unwrap();
        coverage
            .include(BucketCoverage::new(2, 2, 1).unwrap())
            .unwrap();

        assert_eq!(coverage, BucketCoverage::new(4, 3, 1).unwrap());
        assert_eq!(coverage.state(), CoverageState::Partial);
    }

    #[test]
    fn coverage_rejects_impossible_counts() {
        assert!(BucketCoverage::new(0, 0, 0).is_err());
        assert!(BucketCoverage::new(1, 2, 0).is_err());
        assert!(BucketCoverage::new(1, 0, 2).is_err());
    }
}
