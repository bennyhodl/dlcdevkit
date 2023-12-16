include!("./util.rs");
use std::str::FromStr;
use bdk::bitcoin::XOnlyPublicKey;

#[test]
fn oracle_pubkey() {
    let oracle = ernest_wallet::oracle::ErnestOracle::new().unwrap();
    let pubkey = oracle.get_pubkey().unwrap();
    let test_pubkey = XOnlyPublicKey::from_str("7a95aa7f15c283f510cd1d8f3cf8e60dd95770b5a0f6c406bf73d35c7a7c9c59").unwrap();

    assert_eq!(pubkey, test_pubkey)
}
