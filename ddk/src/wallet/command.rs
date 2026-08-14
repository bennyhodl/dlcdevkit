use super::WalletStorage;
use crate::error::WalletError;
use crate::logger::{log_debug, WriteLog};
use crate::{chain::EsploraClient, logger::Logger};
use bdk_chain::spk_client::FullScanRequest;
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::{KeychainKind, PersistedWallet, Update};
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
    wallet
        .persist_async(storage)
        .await
        .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    Ok(())
}
