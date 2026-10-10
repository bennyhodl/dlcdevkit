//! The transactions this node broadcast recently.
//!
//! A wallet sync tells esplora which unconfirmed transactions it expects
//! (bdk's `expected_spk_txids`) and reads a missing one as evicted from
//! the mempool. That is right for a transaction the chain once showed and
//! wrong for one this node sent a second ago: electrs indexes its mempool
//! on its own clock, behind the node's. Evicting an own broadcast frees
//! its coins, and the next selection double-spends them.

use bdk_chain::TxUpdate;
use bitcoin::Txid;
use std::collections::HashMap;
use std::sync::Mutex;

/// How long, in seconds, esplora may not know a transaction this node
/// broadcast before a sync that misses it counts as a real drop. electrs
/// polls the node's mempool every few seconds and indexes new entries
/// after that; a minute covers a busy mempool with room to spare and is
/// still short next to how long a real drop takes to matter.
const OWN_BROADCAST_GRACE_SECS: u64 = 60;

/// When this node broadcast each recent transaction, in unix seconds. In
/// memory: a restart takes longer than the lag this covers.
#[derive(Debug, Default)]
pub(crate) struct RecentBroadcasts {
    broadcasts: Mutex<HashMap<Txid, u64>>,
}

impl RecentBroadcasts {
    /// Remembers that `txid` was broadcast at `at`, and forgets the
    /// broadcasts whose grace ended before it.
    pub(crate) fn record(&self, txid: Txid, at: u64) {
        let mut broadcasts = self.lock();
        broadcasts
            .retain(|_, broadcast_at| at.saturating_sub(*broadcast_at) <= OWN_BROADCAST_GRACE_SECS);
        broadcasts.insert(txid, at);
    }

    /// Drops from `tx_update` the evictions of transactions broadcast
    /// within [`OWN_BROADCAST_GRACE_SECS`] before the sync that reported
    /// them. An eviction is stamped with its sync's start time, so the
    /// comparison needs no clock: a sync that started before or just after
    /// the broadcast cannot tell a lagging index from a drop, and a later
    /// one can.
    pub(crate) fn drop_lagging_evictions<A>(&self, tx_update: &mut TxUpdate<A>) {
        let broadcasts = self.lock();
        tx_update.evicted_ats.retain(|(txid, evicted_at)| {
            broadcasts.get(txid).is_none_or(|broadcast_at| {
                evicted_at.saturating_sub(*broadcast_at) > OWN_BROADCAST_GRACE_SECS
            })
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Txid, u64>> {
        // The map is valid after any panic: every write is one insert or
        // retain.
        self.broadcasts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    fn txid(byte: u8) -> Txid {
        Txid::from_byte_array([byte; 32])
    }

    #[test]
    fn evictions_of_own_broadcasts_wait_out_the_grace() {
        let broadcasts = RecentBroadcasts::default();
        let own = txid(1);
        let foreign = txid(2);
        let broadcast_at = 1_000;
        broadcasts.record(own, broadcast_at);

        let evictions_at = |evicted_at: u64| {
            let mut tx_update = TxUpdate::<()>::default();
            tx_update.evicted_ats.insert((own, evicted_at));
            tx_update.evicted_ats.insert((foreign, evicted_at));
            broadcasts.drop_lagging_evictions(&mut tx_update);
            tx_update.evicted_ats
        };

        // A sync a second after the broadcast misses it because esplora is
        // behind the node: only the foreign eviction applies.
        assert_eq!(
            evictions_at(broadcast_at + 1),
            [(foreign, broadcast_at + 1)].into()
        );
        // A sync that started before the broadcast cannot have seen it.
        assert_eq!(
            evictions_at(broadcast_at - 1),
            [(foreign, broadcast_at - 1)].into()
        );
        // Past the grace, a miss is a real drop.
        let later = broadcast_at + OWN_BROADCAST_GRACE_SECS + 1;
        assert_eq!(evictions_at(later), [(own, later), (foreign, later)].into());
    }
}
