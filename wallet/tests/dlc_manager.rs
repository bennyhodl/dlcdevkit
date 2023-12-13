include!("./util.rs");

#[test]
fn dlc_manager_does_not_fail() {
    let manager = OneWalletTest::setup_bitcoind_and_electrsd_and_ernest("dlc_manager");

    let check = manager.ernest.manager.lock().unwrap().periodic_check(false);

    assert_eq!(check.is_ok(), true)
}
