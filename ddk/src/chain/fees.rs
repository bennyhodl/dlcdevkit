//! Live fee estimation with hardcoded floors.
//!
//! Each sync fetches the fee estimates from esplora and caches one rate per
//! [`ConfirmationTarget`]. The old hardcoded rates stay as floors: an
//! estimate never drops a rate below its floor, and the floors serve as the
//! fallback while no estimate has arrived.

use lightning::chain::chaininterface::ConfirmationTarget;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

/// The minimum fee rate in sats per 1000 weight units (1 sat/vB).
pub const MIN_FEERATE: u32 = 253;

/// Every confirmation target with the esplora block target its estimate
/// comes from and the floor (in sats per 1000 weight units) the cached rate
/// never goes below.
const TARGETS: [(ConfirmationTarget, u16, u32); 8] = [
    (ConfirmationTarget::MaximumFeeEstimate, 1, 5_000),
    (ConfirmationTarget::UrgentOnChainSweep, 1, 5_000),
    (ConfirmationTarget::OutputSpendingFee, 6, 2_000),
    (ConfirmationTarget::NonAnchorChannelFee, 6, 2_000),
    (ConfirmationTarget::AnchorChannelFee, 12, MIN_FEERATE),
    (ConfirmationTarget::ChannelCloseMinimum, 144, MIN_FEERATE),
    (
        ConfirmationTarget::MinAllowedAnchorChannelRemoteFee,
        1008,
        MIN_FEERATE,
    ),
    (
        ConfirmationTarget::MinAllowedNonAnchorChannelRemoteFee,
        1008,
        MIN_FEERATE,
    ),
];

/// A lock-free cache of fee rates per confirmation target.
#[derive(Debug)]
pub struct FeeRateCache {
    fees: HashMap<ConfirmationTarget, AtomicU32>,
}

impl Default for FeeRateCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FeeRateCache {
    /// Creates a cache seeded with the floor rates.
    pub fn new() -> Self {
        let fees = TARGETS
            .iter()
            .map(|(target, _, floor)| (*target, AtomicU32::new(*floor)))
            .collect();
        Self { fees }
    }

    /// Updates the cache from esplora fee estimates (block target to
    /// sat/vB). Targets without a usable estimate keep their last rate.
    pub fn update(&self, estimates: &HashMap<u16, f64>) {
        for (target, blocks, floor) in TARGETS.iter() {
            if let Some(sat_per_vb) = best_estimate(estimates, *blocks) {
                let sat_per_kw = ((sat_per_vb * 250.0).round() as u32).max(*floor);
                self.fees
                    .get(target)
                    .expect("every target is initialized")
                    .store(sat_per_kw, Ordering::Release);
            }
        }
    }

    /// The cached fee rate in sats per 1000 weight units.
    pub fn get(&self, confirmation_target: ConfirmationTarget) -> u32 {
        self.fees
            .get(&confirmation_target)
            .map(|rate| rate.load(Ordering::Acquire))
            .unwrap_or(MIN_FEERATE)
    }
}

/// The estimate at the largest block target at or below `blocks`, falling
/// back to the tightest (highest-fee) estimate available.
fn best_estimate(estimates: &HashMap<u16, f64>, blocks: u16) -> Option<f64> {
    estimates
        .iter()
        .filter(|(target, _)| **target <= blocks)
        .max_by_key(|(target, _)| **target)
        .or_else(|| estimates.iter().min_by_key(|(target, _)| **target))
        .map(|(_, sat_per_vb)| *sat_per_vb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_hold_without_estimates() {
        let cache = FeeRateCache::new();
        assert_eq!(cache.get(ConfirmationTarget::UrgentOnChainSweep), 5_000);
        assert_eq!(cache.get(ConfirmationTarget::MaximumFeeEstimate), 5_000);
        assert_eq!(cache.get(ConfirmationTarget::OutputSpendingFee), 2_000);
        assert_eq!(
            cache.get(ConfirmationTarget::ChannelCloseMinimum),
            MIN_FEERATE
        );
    }

    #[test]
    fn estimates_update_and_convert_to_sat_per_kw() {
        let cache = FeeRateCache::new();
        let estimates = HashMap::from([(1u16, 50.0), (6u16, 20.0), (144u16, 2.0)]);
        cache.update(&estimates);

        // 50 sat/vB = 12500 sat/kw for the one-block targets.
        assert_eq!(cache.get(ConfirmationTarget::UrgentOnChainSweep), 12_500);
        // 20 sat/vB = 5000 sat/kw for the six-block targets.
        assert_eq!(cache.get(ConfirmationTarget::NonAnchorChannelFee), 5_000);
        // The 12-block target falls back to the six-block estimate.
        assert_eq!(cache.get(ConfirmationTarget::AnchorChannelFee), 5_000);
        // 2 sat/vB = 500 sat/kw for the long targets.
        assert_eq!(cache.get(ConfirmationTarget::ChannelCloseMinimum), 500);
    }

    #[test]
    fn estimates_never_drop_below_the_floor() {
        let cache = FeeRateCache::new();
        let estimates = HashMap::from([(1u16, 0.5)]);
        cache.update(&estimates);
        assert_eq!(cache.get(ConfirmationTarget::UrgentOnChainSweep), 5_000);
        assert_eq!(
            cache.get(ConfirmationTarget::ChannelCloseMinimum),
            MIN_FEERATE
        );
    }
}
