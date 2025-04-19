mod test_util;

use std::sync::Arc;

use bitcoin::key::Secp256k1;
use bitcoin::Amount;
use bitcoincore_rpc::RpcApi;
use ddk::oracle::memory::MemoryOracle;
use ddk_manager::contract::Contract;
use ddk_manager::contract::{ser::Serializable, PreClosedContract};
use ddk_manager::Storage;
use lightning::io::Cursor;
use test_util::generate_blocks;

#[tokio::test]
async fn contract_balance() {
    let contract_bytes = include_bytes!("../tests/data/dlc_storage/PreClosed");
    let mut cursor = Cursor::new(contract_bytes);
    let preclosed = PreClosedContract::deserialize(&mut cursor).unwrap();
    let secp = Secp256k1::new();
    let oracle = Arc::new(MemoryOracle::default());
    let bob = test_util::TestSuite::new(&secp, "balance", oracle).await;

    bob.ddk
        .storage
        .update_contract(&Contract::PreClosed(preclosed.clone()))
        .await
        .unwrap();

    let address = bob.ddk.wallet.new_external_address().await.unwrap().address;

    let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let client = bitcoincore_rpc::Client::new("http://127.0.0.1:18443", auth).unwrap();
    client
        .send_to_address(
            &address,
            Amount::ONE_BTC,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    bob.ddk.wallet.sync().await.unwrap();
    let balance = bob.ddk.balance().await.unwrap();
    assert_eq!(balance.foreign_unconfirmed, Amount::ONE_BTC);
    assert_eq!(balance.contract_pnl, -11000000);

    generate_blocks(2);

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    bob.ddk.wallet.sync().await.unwrap();
    let balance = bob.ddk.balance().await.unwrap();
    assert_eq!(balance.confirmed, Amount::ONE_BTC);
    assert_eq!(balance.foreign_unconfirmed, Amount::ZERO);
    assert_eq!(balance.contract_pnl, -11000000);
}
