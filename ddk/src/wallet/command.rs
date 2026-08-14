use super::WalletStorage;
use crate::error::WalletError;
use crate::logger::{log_debug, WriteLog};
use crate::{chain::EsploraClient, logger::Logger};
use bdk_chain::spk_client::FullScanRequest;
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::coin_selection::{
    BranchAndBoundCoinSelection, CoinSelectionAlgorithm, SingleRandomDraw,
};
use bdk_wallet::{KeychainKind, PersistedWallet, Update, Utxo, WeightedUtxo};
use bitcoin::key::rand::thread_rng;
use bitcoin::{Address, Amount, FeeRate, ScriptBuf};
use std::sync::Arc;

type Result<T> = std::result::Result<T, WalletError>;

/// The number of consecutive unused script pubkeys to scan before a full scan
/// stops. A restored wallet with gaps larger than this does not find all funds.
const STOP_GAP: usize = 50;
/// The number of esplora requests a scan makes in parallel.
const PARALLEL_REQUESTS: usize = 5;

#[tracing::instrument(skip_all)]
pub async fn sync(
    wallet: &mut PersistedWallet<WalletStorage>,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
    logger: Arc<Logger>,
) -> Result<()> {
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
    wallet.apply_update(sync_result)?;

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

    wallet
        .persist_async(storage)
        .await
        .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    Ok(())
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

    let selected = BranchAndBoundCoinSelection::new(super::MIN_CHANGE_SIZE, SingleRandomDraw)
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
