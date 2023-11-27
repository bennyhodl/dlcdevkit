use crate::tests::util::{generate_blocks_and_wait, setup_bitcoind_and_electrsd_and_ernest_wallet};
use bitcoin::Amount;
use electrsd::bitcoind::bitcoincore_rpc::{bitcoincore_rpc_json::AddressType, RpcApi};

#[tokio::test]
async fn receive() {
    let (bitcoind, electrsd, wallet) = setup_bitcoind_and_electrsd_and_ernest_wallet();

    generate_blocks_and_wait(&bitcoind, &electrsd, 150);

    let address = wallet.new_external_address().unwrap();

    bitcoind
        .client
        .send_to_address(
            &address.address,
            Amount::from_sat(100_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    generate_blocks_and_wait(&bitcoind, &electrsd, 6);

    let balance = wallet.get_balance().await.unwrap();

    assert_eq!(balance.confirmed, 100_000_000)
}

#[tokio::test]
async fn send() {
    let (bitcoind, electrsd, wallet) = setup_bitcoind_and_electrsd_and_ernest_wallet();

    generate_blocks_and_wait(&bitcoind, &electrsd, 150);

    let address = wallet.new_external_address().unwrap();

    bitcoind
        .client
        .send_to_address(
            &address.address,
            Amount::from_sat(100_000_000),
            None,
            None,
            Some(false),
            None,
            None,
            None,
        )
        .unwrap();

    generate_blocks_and_wait(&bitcoind, &electrsd, 6);

    let wallet_balance = wallet.get_balance().await.unwrap();

    assert_eq!(wallet_balance.confirmed, 100_000_000);

    let bitcoind_addr = bitcoind
        .client
        .get_new_address(None, Some(AddressType::Bech32))
        .unwrap();

    let txn = wallet
        .send_to_address(bitcoind_addr.clone(), 50_000_000, 1.0)
        .await
        .unwrap();
    //
    generate_blocks_and_wait(&bitcoind, &electrsd, 10);

    let txn_seen = bitcoind.client.get_transaction(&txn, None).unwrap();

    assert_eq!(txn_seen.info.txid, txn);

    let wallet_balance = wallet.get_balance().await.unwrap();

    println!("Balance: {}", wallet_balance);
}
