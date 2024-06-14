use bdk::wallet::ChangeSet;
use bdk_file_store::Store;
use ddk::builder::DdkBuilder;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::{DdkConfig, SeedConfig};
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient, Store<ChangeSet>>;

#[tokio::main]
async fn main() {
    let name = "dlcdevkit-example";
    let dir = std::env::current_dir().unwrap().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let storage_path = dir.to_str().unwrap().to_string();
    let seed = SeedConfig::File(storage_path.clone());
    let config = DdkConfig {
        storage_path,
        network: bitcoin::Network::Regtest,
        esplora_host: ddk::ESPLORA_HOST.to_string(),
    };
    let transport = Arc::new(LightningTransport::new(&seed, config.network).unwrap());
    let storage =
        Arc::new(SledStorageProvider::new(dir.join("sled_db").to_str().unwrap()).unwrap());
    let oracle_client = tokio::task::spawn_blocking(move || {
        Arc::new(P2PDOracleClient::new(ddk::ORACLE_HOST).unwrap())
    })
    .await
    .unwrap();

    let wallet_storage = Store::<ChangeSet>::open_or_create_new("dlcdevkit".as_bytes(), dir.join("wallet_db")).unwrap();

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name(name)
        .set_config(config)
        .set_seed_config(seed)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_wallet_storage(wallet_storage)
        .set_oracle(oracle_client.clone())
        .finish()
        .await
        .unwrap();

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().await.expect("nope");

    let address = ddk.wallet.new_external_address().unwrap();

    println!("Address: {}", address);
}
