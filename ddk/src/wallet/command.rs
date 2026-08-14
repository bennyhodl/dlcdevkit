use super::contract_tracker::ContractUtxoTracker;
use super::WalletStorage;
use crate::error::WalletError;
use crate::logger::{log_debug, log_error, WriteLog};
use crate::{chain::EsploraClient, logger::Logger};
use bdk_chain::spk_client::{FullScanRequest, SyncRequest};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::coin_selection::{
    BranchAndBoundCoinSelection, CoinSelectionAlgorithm, SingleRandomDraw,
};
use bdk_wallet::{KeychainKind, PersistedWallet, Update, Utxo, WalletEvent, WeightedUtxo};
use bitcoin::key::rand::thread_rng;
use bitcoin::{Address, Amount, FeeRate, ScriptBuf};
use ddk_manager::contract::Contract;
use ddk_manager::ContractId;
use std::sync::Arc;
use tokio::sync::broadcast;

type Result<T> = std::result::Result<T, WalletError>;

/// The number of consecutive unused script pubkeys to scan before a full scan
/// stops. A restored wallet with gaps larger than this does not find all funds.
const STOP_GAP: usize = 50;
/// The number of esplora requests a scan makes in parallel.
const PARALLEL_REQUESTS: usize = 5;

#[tracing::instrument(skip_all)]
pub async fn sync(
    wallet: &mut PersistedWallet<WalletStorage>,
    tracker: &mut ContractUtxoTracker,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    events: &broadcast::Sender<WalletEvent>,
    logger: Arc<Logger>,
) -> Result<()> {
    // Keep the fee rate cache fresh; a failed fetch keeps the cached
    // rates and never fails the sync.
    blockchain.refresh_fee_estimates().await;

    let block_height = blockchain
        .async_client
        .get_height()
        .await
        .map_err(|e| WalletError::Esplora(e.to_string()))?;
    let prev_tip = wallet.latest_checkpoint();

    log_debug!(
        logger,
        "Syncing wallet with latest known height. height={} wallet_height={}",
        block_height,
        prev_tip.height()
    );
    let sync_result = if prev_tip.height() == 0 {
        log_debug!(logger, "Performing a full chain scan.");
        let mut spk_iters = wallet.all_unbounded_spk_iters();
        let external_spks = spk_iters
            .remove(&KeychainKind::External)
            .expect("wallet has an external keychain");
        let internal_spks = spk_iters
            .remove(&KeychainKind::Internal)
            .expect("wallet has an internal keychain");
        let chain = FullScanRequest::builder()
            .spks_for_keychain(KeychainKind::External, external_spks)
            .spks_for_keychain(KeychainKind::Internal, internal_spks)
            .chain_tip(prev_tip)
            .build();
        let sync = blockchain
            .async_client
            .full_scan(chain, STOP_GAP, PARALLEL_REQUESTS)
            .await
            .map_err(|e| WalletError::Esplora(e.to_string()))?;
        Update::from(sync)
    } else {
        // Tell esplora which txids we expect under our SPKs. Expected txids
        // that no longer show up come back stamped as evicted, which drops
        // replaced or evicted transactions from the canonical view.
        let expected_spk_txids = wallet
            .tx_graph()
            .list_expected_spk_txids(
                wallet.local_chain(),
                prev_tip.block_id(),
                wallet.spk_index(),
                ..,
            )
            .collect::<Vec<_>>();
        let spks = wallet
            .start_sync_with_revealed_spks()
            .expected_spk_txids(expected_spk_txids)
            .chain_tip(prev_tip)
            .build();
        // A sync (not a full scan) must not set last-active indices: the
        // update is built from the sync result alone.
        let sync = blockchain
            .async_client
            .sync(spks, PARALLEL_REQUESTS)
            .await
            .map_err(|e| WalletError::Esplora(e.to_string()))?;
        Update::from(sync)
    };
    forward_events(events, wallet.apply_update_events(sync_result)?);

    // A lock on an outpoint that a confirmed transaction spent is dead
    // weight: release it. Unconfirmed spends keep their locks, because an
    // evicted transaction would return the coin to the spendable set.
    let confirmed_spends = wallet
        .transactions()
        .filter(|canonical_tx| canonical_tx.chain_position.is_confirmed())
        .flat_map(|canonical_tx| {
            canonical_tx
                .tx_node
                .tx
                .input
                .iter()
                .map(|input| input.previous_output)
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::HashSet<_>>();
    let spent_locks = wallet
        .list_locked_outpoints()
        .filter(|outpoint| confirmed_spends.contains(outpoint))
        .collect::<Vec<_>>();
    for outpoint in spent_locks {
        wallet.unlock_outpoint(outpoint);
    }

    sync_contract_utxos(wallet, tracker, blockchain, storage, events, logger).await?;

    wallet
        .persist_async(storage)
        .await
        .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    if let Some(changeset) = tracker.take_staged() {
        storage.0.persist_contract_tracker(&changeset).await?;
    }
    Ok(())
}

/// Runs a second, targeted sync over the contract funding SPKs and
/// outpoints. Esplora resolves outpoint spend status, so one round trip
/// learns both confirmation of the funding transaction and any spend of the
/// funding output (CET, refund, or counterparty close).
async fn sync_contract_utxos(
    wallet: &mut PersistedWallet<WalletStorage>,
    tracker: &mut ContractUtxoTracker,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    events: &broadcast::Sender<WalletEvent>,
    logger: Arc<Logger>,
) -> Result<()> {
    // Contracts learn their funding transaction at accept time; register
    // every funding script the storage knows about. Registration and a
    // close also write BIP-329 labels: the funding transaction and its
    // outpoint get the contract id, the CET gets the attested outcome.
    // Label writes are upserts, so repeating them is harmless, and a
    // failed write never fails the sync.
    match storage.0.get_contracts().await {
        Ok(contracts) => {
            for contract in &contracts {
                if let Some((contract_id, spk, outpoint)) = contract_funding_info(contract) {
                    if tracker.register(contract_id, spk) {
                        let funding_tx_label =
                            bip329::Label::Transaction(bip329::TransactionRecord {
                                ref_: outpoint.txid,
                                label: Some(format!("DLC funding {}", hex::encode(contract_id))),
                                origin: None,
                            });
                        let funding_output_label = bip329::Label::Output(bip329::OutputRecord {
                            ref_: outpoint,
                            label: Some(hex::encode(contract_id)),
                            spendable: Some(false),
                        });
                        for label in [funding_tx_label, funding_output_label] {
                            if let Err(e) = storage.0.persist_label(&label).await {
                                log_error!(logger, "Could not write a contract label. error={}", e);
                            }
                        }
                    }
                }
                if let Some((cet_txid, close_label)) = contract_close_label(contract) {
                    let label = bip329::Label::Transaction(bip329::TransactionRecord {
                        ref_: cet_txid,
                        label: Some(close_label),
                        origin: None,
                    });
                    if let Err(e) = storage.0.persist_label(&label).await {
                        log_error!(logger, "Could not write a close label. error={}", e);
                    }
                }
            }
        }
        Err(e) => log_error!(
            logger,
            "Could not load contracts for the funding tracker. error={}",
            e
        ),
    }

    let spks = tracker.sync_spks();
    if spks.is_empty() {
        return Ok(());
    }

    let request = SyncRequest::builder()
        .chain_tip(wallet.latest_checkpoint())
        .spks(spks)
        .outpoints(tracker.sync_outpoints())
        .build();
    let response = blockchain
        .async_client
        .sync(request, PARALLEL_REQUESTS)
        .await
        .map_err(|e| WalletError::Esplora(e.to_string()))?;

    // The tracker shares the wallet's chain view: connect the chain update
    // to the wallet so the tracker's anchors resolve.
    if let Some(chain_update) = response.chain_update {
        forward_events(
            events,
            wallet.apply_update_events(Update {
                chain: Some(chain_update),
                ..Default::default()
            })?,
        );
    }
    tracker.apply_update(response.tx_update);

    Ok(())
}

/// Forwards wallet events to the subscribers. A send fails only when no
/// subscriber exists, which is fine.
fn forward_events(events: &broadcast::Sender<WalletEvent>, wallet_events: Vec<WalletEvent>) {
    for event in wallet_events {
        let _ = events.send(event);
    }
}

/// The funding script and outpoint of a contract that has a funding
/// transaction, with the contract's id. Offered contracts do not have one
/// yet: the funding transaction needs the accept party's inputs.
pub(super) fn contract_funding_info(
    contract: &Contract,
) -> Option<(ContractId, ScriptBuf, bitcoin::OutPoint)> {
    let accepted = match contract {
        Contract::Accepted(accepted) => accepted,
        Contract::Signed(signed) | Contract::Confirmed(signed) => &signed.accepted_contract,
        Contract::PreClosed(preclosed) => &preclosed.signed_contract.accepted_contract,
        _ => return None,
    };
    let transactions = &accepted.dlc_transactions;
    let fund_output = transactions.get_fund_output();
    let outpoint = bitcoin::OutPoint {
        txid: transactions.fund.compute_txid(),
        vout: transactions.get_fund_output_index() as u32,
    };
    Some((
        accepted.get_contract_id(),
        fund_output.script_pubkey.clone(),
        outpoint,
    ))
}

/// The broadcast CET of a closed or pre-closed contract with the label it
/// gets: the contract id and the attested outcome.
pub(super) fn contract_close_label(contract: &Contract) -> Option<(bitcoin::Txid, String)> {
    let (cet, attestations, contract_id) = match contract {
        Contract::PreClosed(preclosed) => (
            Some(&preclosed.signed_cet),
            preclosed.attestations.as_ref(),
            preclosed
                .signed_contract
                .accepted_contract
                .get_contract_id(),
        ),
        Contract::Closed(closed) => (
            closed.signed_cet.as_ref(),
            closed.attestations.as_ref(),
            closed.contract_id,
        ),
        _ => return None,
    };
    let txid = cet?.compute_txid();
    let outcomes = attestations
        .map(|attestations| {
            attestations
                .iter()
                .flat_map(|attestation| attestation.outcomes.clone())
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|outcomes| !outcomes.is_empty());
    let label = match outcomes {
        Some(outcomes) => format!("DLC close {}: {}", hex::encode(contract_id), outcomes),
        None => format!("DLC close {}", hex::encode(contract_id)),
    };
    Some((txid, label))
}

/// What a send spends: a specific amount, or the whole wallet.
#[derive(Debug)]
pub enum Spend {
    /// Send this amount to the destination
    Amount(Amount),
    /// Drain every spendable coin to the destination
    All,
}

/// Coin control for a send: restrict which UTXOs fund the transaction.
#[derive(Debug, Default, Clone)]
pub struct CoinControl {
    /// Fund the transaction with exactly these UTXOs and no others
    pub selected_utxos: Vec<bitcoin::OutPoint>,
    /// Never spend these outpoints
    pub unspendable: Vec<bitcoin::OutPoint>,
    /// Only spend coins with at least this many confirmations
    pub min_confirmations: Option<u32>,
}

/// Builds, signs, and broadcasts a spend to `address`, then persists the
/// wallet so the revealed change index survives a restart.
#[tracing::instrument(skip(wallet, blockchain, storage))]
pub async fn send(
    wallet: &mut PersistedWallet<WalletStorage>,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    address: Address,
    spend: Spend,
    fee_rate: FeeRate,
    coin_control: CoinControl,
) -> Result<bitcoin::Txid> {
    let mut builder = wallet.build_tx();
    builder.version(2).fee_rate(fee_rate);
    match spend {
        Spend::Amount(amount) => {
            builder.add_recipient(address.script_pubkey(), amount);
        }
        Spend::All => {
            if coin_control.selected_utxos.is_empty() {
                builder.drain_wallet();
            }
            builder.drain_to(address.script_pubkey());
        }
    }
    if !coin_control.selected_utxos.is_empty() {
        builder.add_utxos(&coin_control.selected_utxos)?;
        builder.manually_selected_only();
    }
    builder.unspendable(coin_control.unspendable);
    if let Some(min_confirmations) = coin_control.min_confirmations {
        builder.exclude_below_confirmations(min_confirmations);
    }
    let psbt = builder.finish()?;
    sign_and_broadcast(wallet, blockchain, storage, psbt).await
}

/// Builds, signs, and broadcasts an RBF replacement of `txid` at the
/// higher `fee_rate`. RBF is on by default for wallet transactions.
#[tracing::instrument(skip(wallet, blockchain, storage))]
pub async fn bump_fee(
    wallet: &mut PersistedWallet<WalletStorage>,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    txid: bitcoin::Txid,
    fee_rate: FeeRate,
) -> Result<bitcoin::Txid> {
    let mut builder = wallet.build_fee_bump(txid)?;
    builder.fee_rate(fee_rate);
    let psbt = builder.finish()?;
    sign_and_broadcast(wallet, blockchain, storage, psbt).await
}

/// Signs a built PSBT, broadcasts the transaction, and persists the
/// wallet so the revealed change index survives a restart.
async fn sign_and_broadcast(
    wallet: &mut PersistedWallet<WalletStorage>,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    mut psbt: bitcoin::Psbt,
) -> Result<bitcoin::Txid> {
    wallet.sign(&mut psbt, bdk_wallet::SignOptions::default())?;
    let tx = psbt.extract_tx().map_err(|_| WalletError::ExtractTx)?;
    let txid = tx.compute_txid();
    blockchain
        .async_client
        .broadcast(&tx)
        .await
        .map_err(|e| WalletError::Esplora(e.to_string()))?;
    wallet
        .persist_async(storage)
        .await
        .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    Ok(txid)
}

/// Selects UTXOs that cover `amount` at `fee_rate`, ignoring locked
/// outpoints. When `lock_utxos` is set, the selected outpoints are locked and
/// the locks are persisted, so a concurrent selection cannot pick the same
/// coins and the locks survive a restart.
#[tracing::instrument(skip(wallet, storage))]
pub async fn select_utxos(
    wallet: &mut PersistedWallet<WalletStorage>,
    storage: &mut WalletStorage,
    amount: Amount,
    fee_rate: u64,
    lock_utxos: bool,
    min_change_size: u64,
) -> Result<Vec<ddk_manager::Utxo>> {
    let candidates = wallet
        .list_unspent()
        .filter(|utxo| !wallet.is_outpoint_locked(utxo.outpoint))
        .map(|utxo| WeightedUtxo {
            satisfaction_weight: utxo.txout.weight(),
            utxo: Utxo::Local(utxo),
        })
        .collect::<Vec<WeightedUtxo>>();

    let fee_rate = FeeRate::from_sat_per_vb(fee_rate)
        .ok_or_else(|| WalletError::Esplora(format!("Invalid fee rate: {fee_rate}")))?;

    let selected = BranchAndBoundCoinSelection::new(min_change_size, SingleRandomDraw)
        .coin_select(
            vec![],
            candidates,
            fee_rate,
            amount,
            ScriptBuf::new().as_script(),
            &mut thread_rng(),
        )?;

    let network = wallet.network();
    let utxos = selected
        .selected
        .iter()
        .map(|utxo| {
            let address = Address::from_script(&utxo.txout().script_pubkey, network)
                .expect("wallet outputs have addressable script pubkeys");
            ddk_manager::Utxo {
                tx_out: utxo.txout().clone(),
                outpoint: utxo.outpoint(),
                address,
                redeem_script: ScriptBuf::new(),
                reserved: lock_utxos,
            }
        })
        .collect::<Vec<_>>();

    if lock_utxos {
        for utxo in &utxos {
            wallet.lock_outpoint(utxo.outpoint);
        }
        wallet
            .persist_async(storage)
            .await
            .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    }

    Ok(utxos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::ser::deserialize_contract;

    #[test]
    fn funding_spk_follows_the_contract_state() {
        let offered = deserialize_contract(
            &include_bytes!("../../../testconfig/contract_binaries/Offered").to_vec(),
        )
        .unwrap();
        assert!(contract_funding_info(&offered).is_none());

        let accepted = deserialize_contract(
            &include_bytes!("../../../testconfig/contract_binaries/Accepted").to_vec(),
        )
        .unwrap();
        let (accepted_id, accepted_spk, accepted_outpoint) =
            contract_funding_info(&accepted).unwrap();
        assert!(accepted_spk.is_p2wsh());

        // The same contract keeps the same funding script through its
        // lifecycle.
        for binary in [
            include_bytes!("../../../testconfig/contract_binaries/Signed").to_vec(),
            include_bytes!("../../../testconfig/contract_binaries/Confirmed").to_vec(),
            include_bytes!("../../../testconfig/contract_binaries/PreClosed").to_vec(),
        ] {
            let contract = deserialize_contract(&binary).unwrap();
            let (contract_id, spk, outpoint) = contract_funding_info(&contract).unwrap();
            assert_eq!(contract_id, accepted_id);
            assert_eq!(spk, accepted_spk);
            assert_eq!(outpoint, accepted_outpoint);
        }
    }

    #[test]
    fn close_labels_carry_the_outcome() {
        let confirmed = deserialize_contract(
            &include_bytes!("../../../testconfig/contract_binaries/Confirmed").to_vec(),
        )
        .unwrap();
        assert!(contract_close_label(&confirmed).is_none());

        for binary in [
            include_bytes!("../../../testconfig/contract_binaries/PreClosed").to_vec(),
            include_bytes!("../../../testconfig/contract_binaries/Closed").to_vec(),
        ] {
            let contract = deserialize_contract(&binary).unwrap();
            let (_txid, label) = contract_close_label(&contract).unwrap();
            assert!(label.starts_with("DLC close "));
        }
    }
}
