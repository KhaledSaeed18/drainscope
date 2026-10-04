//! Energy and power units.
//!
//! Hardware counters are integer microjoules ([`Microjoules`]); model math uses floating-point
//! joules ([`Joules`]) and watts ([`Watts`]). Durations are [`std::time::Duration`].

use std::iter::Sum;
use std::ops::{Add, AddAssign, Mul, Sub};
use std::time::Duration;

/// A raw energy counter value or delta, in microjoules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Microjoules(pub u64);

impl Microjoules {
    #[must_use]
    // A u64 delta only loses precision above 2^53 µJ (≈ 2.5 MWh), far beyond any interval.
    #[allow(clippy::cast_precision_loss)]
    pub fn to_joules(self) -> Joules {
        Joules(self.0 as f64 / 1e6)
    }
}

/// Energy in joules.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Joules(pub f64);

impl Joules {
    pub const ZERO: Self = Self(0.0);

    /// Average power if this energy was spent over `duration`. Zero for a zero duration.
    #[must_use]
    pub fn over(self, duration: Duration) -> Watts {
        let secs = duration.as_secs_f64();
        if secs > 0.0 {
            Watts(self.0 / secs)
        } else {
            Watts(0.0)
        }
    }

    /// `max(self, 0)`. Differences of independently read counters can dip below zero.
    #[must_use]
    pub fn non_negative(self) -> Self {
        Self(self.0.max(0.0))
    }
}

impl Add for Joules {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Joules {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Joules {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Mul<f64> for Joules {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self(self.0 * rhs)
    }
}

impl Sum for Joules {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

/// Power in watts.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Watts(pub f64);

impl Watts {
    /// Energy spent at this power over `duration`.
    #[must_use]
    pub fn for_duration(self, duration: Duration) -> Joules {
        Joules(self.0 * duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microjoules_convert_to_joules() {
        assert_eq!(Microjoules(2_500_000).to_joules(), Joules(2.5));
    }

    #[test]
    fn power_and_energy_round_trip() {
        let two_seconds = Duration::from_secs(2);
        assert_eq!(Joules(10.0).over(two_seconds), Watts(5.0));
        assert_eq!(Watts(5.0).for_duration(two_seconds), Joules(10.0));
    }

    #[test]
    fn zero_duration_gives_zero_power() {
        assert_eq!(Joules(10.0).over(Duration::ZERO), Watts(0.0));
    }

    #[test]
    fn joules_sum() {
        let total: Joules = [Joules(1.0), Joules(2.5)].into_iter().sum();
        assert_eq!(total, Joules(3.5));
    }
}
