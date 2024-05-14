// include!("./util.rs");

// #[tokio::test]
// async fn wallet_receive_bitcoin() {
//     let test = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit("receive").await;

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 150);

//     let address = test.dlc_dev_kit.wallet.new_external_address().unwrap();

//     test.bitcoind
//         .client
//         .send_to_address(
//             &address.address,
//             Amount::from_sat(100_000_000),
//             None,
//             None,
//             None,
//             None,
//             None,
//             None,
//         )
//         .unwrap();

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

//     let balance = test.dlc_dev_kit.wallet.get_balance().unwrap();

//     assert_eq!(balance.confirmed, 100_000_000)
// }

// #[tokio::test]
// async fn wallet_send_bitcoin() {
//     let test = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit("send").await;

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 150);

//     let address = test.dlc_dev_kit.wallet.new_external_address().unwrap();

//     test.bitcoind
//         .client
//         .send_to_address(
//             &address.address,
//             Amount::from_sat(100_000_000),
//             None,
//             None,
//             Some(false),
//             None,
//             None,
//             None,
//         )
//         .unwrap();

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

//     let wallet_balance = test.dlc_dev_kit.wallet.get_balance().unwrap();

//     assert_eq!(wallet_balance.confirmed, 100_000_000);

//     let bitcoind_addr = test
//         .bitcoind
//         .client
//         .get_new_address(None, Some(AddressType::Bech32))
//         .unwrap();

//     let txn = test
//         .dlc_dev_kit
//         .wallet
//         .send_to_address(bitcoind_addr.clone(), 50_000_000, 1.0)
//         .unwrap();

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 10);

//     let txn_seen = test.bitcoind.client.get_transaction(&txn, None).unwrap();

//     assert_eq!(txn_seen.info.txid, txn);
// }

// #[tokio::test]
// async fn dlc_dev_kit_wallet_sending_to_dlc_dev_kit_wallet() {
//     let test = TwoWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit(
//         "two_dlc_dev_kit_one",
//         "two_dlc_dev_kit_two",
//     )
//     .await;

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 150);

//     let funding_address = test.dlc_dev_kit_one.wallet.new_external_address().unwrap();

//     test.bitcoind
//         .client
//         .send_to_address(
//             &funding_address.address,
//             Amount::from_sat(100_000_000),
//             None,
//             None,
//             Some(false),
//             None,
//             None,
//             None,
//         )
//         .unwrap();

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

//     test.dlc_dev_kit_one.wallet.sync().unwrap();

//     let send_to_two = test.dlc_dev_kit_two.wallet.new_external_address().unwrap();

//     test.dlc_dev_kit_one
//         .wallet
//         .send_to_address(send_to_two.address, 50_000_000, 1.0)
//         .unwrap();

//     generate_blocks_and_wait(&test.bitcoind, &test.electrsd, 6);

//     let balance_one = test.dlc_dev_kit_one.wallet.get_balance().unwrap();

//     let balance_two = test.dlc_dev_kit_two.wallet.get_balance().unwrap();

//     test.dlc_dev_kit_one.wallet.sync().unwrap();
//     test.dlc_dev_kit_two.wallet.sync().unwrap();

//     assert!(balance_one.confirmed > 1);
//     assert_eq!(balance_two.confirmed, 50_000_000);
// }
