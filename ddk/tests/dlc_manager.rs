// include!("./util.rs");
//
// #[tokio::test]
// async fn dlc_manager_does_not_fail() {
//     let manager = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit("dlc_manager").await;
//
//     let check = manager.dlc_dev_kit.manager.lock().unwrap().periodic_check(false);
//
//     assert_eq!(check.is_ok(), true)
// }
