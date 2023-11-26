use crate::tests::util::{generate_blocks_and_wait, setup_bitcoind_and_electrsd_and_ernest_wallet};
use bitcoin::Amount;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;

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
