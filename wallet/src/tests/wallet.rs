use crate::tests::util::{generate_blocks_and_wait, TestSuite};
use bitcoin::Amount;
use electrsd::bitcoind::bitcoincore_rpc::{bitcoincore_rpc_json::AddressType, RpcApi};

#[test]
fn receive() {
    let test = TestSuite::setup_bitcoind_and_electrsd_and_ernest("receive");

    generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 150);

    let address = test.ernest.wallet.new_external_address().unwrap();

    test.bitcoind
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

    generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

    let balance = test.ernest.wallet.get_balance().unwrap();

    assert_eq!(balance.confirmed, 100_000_000)
}

#[test]
fn send() {
    let test = TestSuite::setup_bitcoind_and_electrsd_and_ernest("send");

    generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 150);

    let address = test.ernest.wallet.new_external_address().unwrap();

    test.bitcoind
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

    generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

    let wallet_balance = test.ernest.wallet.get_balance().unwrap();

    assert_eq!(wallet_balance.confirmed, 100_000_000);

    let bitcoind_addr = test.bitcoind
        .client
        .get_new_address(None, Some(AddressType::Bech32))
        .unwrap();

    let txn = test.ernest.wallet
        .send_to_address(bitcoind_addr.clone(), 50_000_000, 1.0)
        
        .unwrap();

    generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 10);

    let txn_seen = test.bitcoind.client.get_transaction(&txn, None).unwrap();

    assert_eq!(txn_seen.info.txid, txn);
}
