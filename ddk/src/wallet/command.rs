use crate::chain::EsploraClient;
use crate::error::WalletError;
use bdk_chain::spk_client::FullScanRequest;
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::{KeychainKind, PersistedWallet, Update};
use std::collections::BTreeMap;

use super::WalletStorage;

type Result<T> = std::result::Result<T, WalletError>;

pub async fn sync(
    wallet: &mut PersistedWallet<WalletStorage>,
    blockchain: &EsploraClient,
    storage: &mut WalletStorage,
) -> Result<()> {
    let prev_tip = wallet.latest_checkpoint();
    tracing::debug!(
        height = prev_tip.height(),
        "Syncing wallet with latest known height."
    );
    let sync_result = if prev_tip.height() == 0 {
        tracing::info!("Performing a full chain scan.");
        let spks = wallet
            .all_unbounded_spk_iters()
            .get(&KeychainKind::External)
            .unwrap()
            .to_owned();
        let chain = FullScanRequest::builder()
            .spks_for_keychain(KeychainKind::External, spks.clone())
            .chain_tip(prev_tip)
            .build();
        let sync = blockchain
            .async_client
            .full_scan(chain, 10, 1)
            .await
            .map_err(|e| WalletError::Esplora(e.to_string()))?;
        Update {
            last_active_indices: sync.last_active_indices,
            tx_update: sync.tx_update,
            chain: sync.chain_update,
        }
    } else {
        let spks = wallet
            .start_sync_with_revealed_spks()
            .chain_tip(prev_tip)
            .build();
        let sync = blockchain
            .async_client
            .sync(spks, 1)
            .await
            .map_err(|e| WalletError::Esplora(e.to_string()))?;
        let indices = wallet.derivation_index(KeychainKind::External).unwrap_or(0);
        let internal_index = wallet.derivation_index(KeychainKind::Internal).unwrap_or(0);
        let mut last_active_indices = BTreeMap::new();
        last_active_indices.insert(KeychainKind::External, indices);
        last_active_indices.insert(KeychainKind::Internal, internal_index);
        Update {
            last_active_indices,
            tx_update: sync.tx_update,
            chain: sync.chain_update,
        }
    };
    wallet.apply_update(sync_result)?;
    wallet
        .persist_async(storage)
        .await
        .map_err(|e| WalletError::WalletPersistanceError(e.to_string()))?;
    Ok(())
}
