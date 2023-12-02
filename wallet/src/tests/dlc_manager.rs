use super::util::TestSuite;

#[test]
fn manager() {
    let manager = TestSuite::setup_bitcoind_and_electrsd_and_ernest("dlc_manager");

    let check = manager.ernest.manager.lock().unwrap().periodic_check(false);

    assert_eq!(check.is_ok(), true)
}
