include!("./util.rs");

#[tokio::test]
async fn dlc_manager_does_not_fail() {
    let manager = OneWalletTest::setup_bitcoind_and_electrsd_and_ernest("dlc_manager").await;

    let check = manager.ernest.manager.lock().unwrap().periodic_check(false);

    assert_eq!(check.is_ok(), true)
}
