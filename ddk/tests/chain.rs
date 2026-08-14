use std::sync::Arc;

use bitcoin::hashes::Hash;
use bitcoin::{Amount, Network, Txid};
use bitcoincore_rpc::RpcApi;
use ddk::chain::EsploraClient;
use ddk::logger::Logger;
use ddk_manager::{Blockchain, ConfirmationStatus};

/// The esplora client must tell apart a transaction the network does not
/// know, a transaction in the mempool, and a confirmed transaction.
///
/// Before this split, a transaction that was evicted from the mempool and a
/// transaction with zero confirmations both reported zero confirmations.
/// With `NB_CONFIRMATIONS=0` the manager then moved contracts forward on
/// transactions that did not exist.
#[tokio::test]
async fn esplora_reports_confirmation_status() {
    let env = ddk_testenv::env();
    let logger = Arc::new(Logger::disabled("chain-test".to_string()));
    let esplora = EsploraClient::new(env.esplora_host(), Network::Regtest, logger).unwrap();

    // A transaction the network does not know is reported as not found.
    let unknown_txid = Txid::from_byte_array([7u8; 32]);
    let status = esplora
        .get_transaction_confirmations(&unknown_txid)
        .await
        .unwrap();
    assert_eq!(status, ConfirmationStatus::NotFound);

    // A transaction in the mempool is reported as in the mempool.
    let address = env
        .rpc()
        .get_new_address(None, None)
        .unwrap()
        .assume_checked();
    let txid = env.send_to_address(&address, Amount::from_btc(0.1).unwrap());
    env.wait_for_tx(&txid);
    let status = esplora.get_transaction_confirmations(&txid).await.unwrap();
    assert_eq!(status, ConfirmationStatus::InMempool);
    assert_eq!(status.confirmations(), 0);

    // A mined transaction is reported as confirmed.
    env.generate_blocks(1);
    let status = esplora.get_transaction_confirmations(&txid).await.unwrap();
    assert!(
        matches!(status, ConfirmationStatus::Confirmed(n) if n >= 1),
        "expected a confirmed status, got {:?}",
        status
    );
    assert!(status.confirmations() >= 1);
}
