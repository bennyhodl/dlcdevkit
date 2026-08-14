//! Chain-truth tracking of DLC funding outputs.
//!
//! The 2-of-2 funding output of a contract is invisible to the BDK wallet:
//! it pays a P2WSH script that no wallet descriptor expresses. The tracker
//! indexes each contract's funding script over its own transaction graph
//! with the `bdk_chain` primitives, sharing the wallet's local chain view.
//! Confirmation of the funding transaction and any spend of the funding
//! output (CET, refund, or counterparty close) then come from the chain
//! instead of collateral math.

use bdk_chain::indexed_tx_graph::IndexedTxGraph;
use bdk_chain::indexer::spk_txout::SpkTxOutIndex;
use bdk_chain::local_chain::LocalChain;
use bdk_chain::{
    Balance, CanonicalizationParams, ChainPosition, ConfirmationBlockTime, Merge, TxUpdate,
};
use bitcoin::{OutPoint, ScriptBuf, Txid};
use ddk_manager::ContractId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A contract funding output with its chain state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractUtxo {
    /// The contract this funding output belongs to
    pub contract_id: ContractId,
    /// The funding outpoint
    pub outpoint: OutPoint,
    /// The funding output
    pub txout: bitcoin::TxOut,
    /// Whether the funding transaction is confirmed
    pub confirmed: bool,
    /// The transaction that spent the funding output, when the chain
    /// knows one (CET, refund, or counterparty close)
    pub spent_by: Option<Txid>,
}

/// The serde changeset of the tracker, persisted through the `Storage`
/// trait next to the wallet changeset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// Funding script registrations by contract. Serialized as a list of
    /// pairs: a JSON map cannot key on a byte array.
    #[serde(with = "spk_map_serde")]
    pub spks: BTreeMap<ContractId, ScriptBuf>,
    /// The transaction graph changes
    pub tx_graph: bdk_chain::tx_graph::ChangeSet<ConfirmationBlockTime>,
}

mod spk_map_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<ContractId, ScriptBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&map.iter().collect::<Vec<_>>(), serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<ContractId, ScriptBuf>, D::Error> {
        let entries: Vec<(ContractId, ScriptBuf)> = serde::Deserialize::deserialize(deserializer)?;
        Ok(entries.into_iter().collect())
    }
}

impl Merge for ChangeSet {
    fn merge(&mut self, other: Self) {
        self.spks.extend(other.spks);
        self.tx_graph.merge(other.tx_graph);
    }

    fn is_empty(&self) -> bool {
        self.spks.is_empty() && self.tx_graph.is_empty()
    }
}

/// Tracks contract funding outputs beside the wallet.
///
/// The tracker owns a transaction graph indexed by contract id and shares
/// the wallet's [`LocalChain`] view, which the caller passes into the query
/// methods. Changes stage into a [`ChangeSet`] the caller persists.
pub struct ContractUtxoTracker {
    graph: IndexedTxGraph<ConfirmationBlockTime, SpkTxOutIndex<ContractId>>,
    stage: ChangeSet,
}

impl Default for ContractUtxoTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ContractUtxoTracker {
    /// Creates an empty tracker.
    pub fn new() -> Self {
        Self {
            graph: IndexedTxGraph::new(SpkTxOutIndex::default()),
            stage: ChangeSet::default(),
        }
    }

    /// Rebuilds a tracker from a persisted changeset.
    pub fn from_changeset(changeset: ChangeSet) -> Self {
        let mut index = SpkTxOutIndex::default();
        for (contract_id, spk) in &changeset.spks {
            index.insert_spk(*contract_id, spk.clone());
        }
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_changeset(bdk_chain::indexed_tx_graph::ChangeSet {
            tx_graph: changeset.tx_graph,
            indexer: (),
        });
        Self {
            graph,
            stage: ChangeSet::default(),
        }
    }

    /// Registers a contract's funding script. Idempotent; returns whether
    /// the script was new. Transactions already in the graph are
    /// re-indexed so a late registration still finds its output.
    pub fn register(&mut self, contract_id: ContractId, spk: ScriptBuf) -> bool {
        if self.graph.index.spk_at_index(&contract_id) == Some(spk.clone()) {
            return false;
        }
        self.graph.index.insert_spk(contract_id, spk.clone());
        let _ = self.graph.reindex();
        self.stage.spks.insert(contract_id, spk);
        true
    }

    /// Whether the contract's funding script is registered.
    pub fn contains(&self, contract_id: &ContractId) -> bool {
        self.graph.index.spk_at_index(contract_id).is_some()
    }

    /// The funding scripts to carry in a targeted sync request.
    pub fn sync_spks(&self) -> Vec<ScriptBuf> {
        self.graph.index.all_spks().values().cloned().collect()
    }

    /// The known funding outpoints to carry in a targeted sync request,
    /// so esplora resolves their spend status.
    pub fn sync_outpoints(&self) -> Vec<OutPoint> {
        self.graph
            .index
            .outpoints()
            .iter()
            .map(|(_, outpoint)| *outpoint)
            .collect()
    }

    /// Applies a transaction update from a sync and stages the change.
    pub fn apply_update(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        let changeset = self.graph.apply_update(update);
        self.stage.merge(ChangeSet {
            spks: BTreeMap::new(),
            tx_graph: changeset.tx_graph,
        });
    }

    /// The balance over the tracked funding outputs, from chain truth.
    /// All tracked outputs are trusted: both parties signed the funding
    /// transaction.
    pub fn balance(&self, chain: &LocalChain) -> Balance {
        self.graph.graph().balance(
            chain,
            chain.tip().block_id(),
            CanonicalizationParams::default(),
            self.graph.index.outpoints().iter().cloned(),
            |_, _| true,
        )
    }

    /// The tracked funding outputs with their chain state, including the
    /// spent ones so a caller can observe a close.
    pub fn utxos(&self, chain: &LocalChain) -> Vec<ContractUtxo> {
        self.graph
            .graph()
            .filter_chain_txouts(
                chain,
                chain.tip().block_id(),
                CanonicalizationParams::default(),
                self.graph.index.outpoints().iter().cloned(),
            )
            .map(|(contract_id, full_txout)| ContractUtxo {
                contract_id,
                outpoint: full_txout.outpoint,
                txout: full_txout.txout,
                confirmed: matches!(full_txout.chain_position, ChainPosition::Confirmed { .. }),
                spent_by: full_txout.spent_by.map(|(_, txid)| txid),
            })
            .collect()
    }

    /// Takes the staged changeset for persistence, if any.
    pub fn take_staged(&mut self) -> Option<ChangeSet> {
        if self.stage.is_empty() {
            None
        } else {
            Some(core::mem::take(&mut self.stage))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk_chain::BlockId;
    use bitcoin::hashes::Hash;
    use bitcoin::{
        absolute::LockTime, transaction::Version, Amount, BlockHash, OutPoint, Sequence,
        Transaction, TxIn, TxOut, Witness,
    };

    fn funding_spk() -> ScriptBuf {
        // Any script works for the index; a P2WSH-shaped one keeps the
        // test honest.
        ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([0xCD; 32]))
    }

    fn chain_with_blocks(blocks: u32) -> LocalChain {
        let (mut chain, _) = LocalChain::from_genesis_hash(BlockHash::from_byte_array([0; 32]));
        for height in 1..=blocks {
            let mut hash = [0u8; 32];
            hash[..4].copy_from_slice(&height.to_be_bytes());
            let _ = chain
                .insert_block(BlockId {
                    height,
                    hash: BlockHash::from_byte_array(hash),
                })
                .unwrap();
        }
        chain
    }

    fn funding_tx(spk: ScriptBuf) -> Transaction {
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([0xAA; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(200_000),
                script_pubkey: spk,
            }],
        }
    }

    fn anchor_at(chain: &LocalChain, height: u32) -> ConfirmationBlockTime {
        ConfirmationBlockTime {
            block_id: chain.get(height).unwrap().block_id(),
            confirmation_time: 100,
        }
    }

    #[test]
    fn tracks_funding_confirmation_and_spend() {
        let chain = chain_with_blocks(3);
        let contract_id = [0x11; 32];
        let spk = funding_spk();

        let mut tracker = ContractUtxoTracker::new();
        assert!(tracker.register(contract_id, spk.clone()));
        assert!(!tracker.register(contract_id, spk.clone()));

        // Unconfirmed funding transaction: a pending balance.
        let fund = funding_tx(spk.clone());
        let fund_txid = fund.compute_txid();
        let mut update = TxUpdate::default();
        update.txs.push(fund.clone().into());
        update.seen_ats.insert((fund_txid, 100));
        tracker.apply_update(update);

        let balance = tracker.balance(&chain);
        assert_eq!(balance.trusted_pending, Amount::from_sat(200_000));
        assert_eq!(balance.confirmed, Amount::ZERO);

        // Confirmation moves the balance and marks the utxo confirmed.
        let mut update = TxUpdate::default();
        update.anchors.insert((anchor_at(&chain, 2), fund_txid));
        tracker.apply_update(update);

        let balance = tracker.balance(&chain);
        assert_eq!(balance.confirmed, Amount::from_sat(200_000));
        let utxos = tracker.utxos(&chain);
        assert_eq!(utxos.len(), 1);
        assert!(utxos[0].confirmed);
        assert_eq!(utxos[0].spent_by, None);
        assert_eq!(utxos[0].contract_id, contract_id);

        // A spend of the funding output (a CET) is observed.
        let cet = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: fund_txid,
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(199_000),
                script_pubkey: ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array(
                    [0xEE; 32],
                )),
            }],
        };
        let cet_txid = cet.compute_txid();
        let mut update = TxUpdate::default();
        update.txs.push(cet.into());
        update.anchors.insert((anchor_at(&chain, 3), cet_txid));
        tracker.apply_update(update);

        let balance = tracker.balance(&chain);
        assert_eq!(balance.confirmed, Amount::ZERO);
        let utxos = tracker.utxos(&chain);
        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].spent_by, Some(cet_txid));
    }

    #[test]
    fn changeset_round_trips() {
        let chain = chain_with_blocks(2);
        let contract_id = [0x22; 32];
        let spk = funding_spk();

        let mut tracker = ContractUtxoTracker::new();
        tracker.register(contract_id, spk.clone());
        let fund = funding_tx(spk);
        let fund_txid = fund.compute_txid();
        let mut update = TxUpdate::default();
        update.txs.push(fund.into());
        update.anchors.insert((anchor_at(&chain, 2), fund_txid));
        tracker.apply_update(update);

        let staged = tracker.take_staged().unwrap();
        assert!(tracker.take_staged().is_none());

        let restored = ContractUtxoTracker::from_changeset(staged);
        assert!(restored.contains(&contract_id));
        assert_eq!(
            restored.balance(&chain).confirmed,
            Amount::from_sat(200_000)
        );
        assert_eq!(restored.utxos(&chain), tracker.utxos(&chain));
    }

    #[test]
    fn late_registration_finds_existing_outputs() {
        let chain = chain_with_blocks(2);
        let spk = funding_spk();

        // The funding transaction lands in the graph before the contract
        // is registered (restore path).
        let mut tracker = ContractUtxoTracker::new();
        tracker.register(
            [0x01; 32],
            ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([0x0F; 32])),
        );
        let fund = funding_tx(spk.clone());
        let fund_txid = fund.compute_txid();
        let mut update = TxUpdate::default();
        update.txs.push(fund.into());
        update.anchors.insert((anchor_at(&chain, 2), fund_txid));
        tracker.apply_update(update);
        assert_eq!(tracker.balance(&chain).confirmed, Amount::ZERO);

        tracker.register([0x33; 32], spk);
        assert_eq!(tracker.balance(&chain).confirmed, Amount::from_sat(200_000));
    }
}
