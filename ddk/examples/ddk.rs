use ddk::builder::DdkBuilder;
use ddk::{DdkConfig, SeedConfig};
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::oracle::P2PDOracleClient;
use ddk::Network;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

#[tokio::main]
async fn main() {
    let name = "dlcdevkit-example";
    let dir = std::env::current_dir().unwrap().join(name);
    let seed = SeedConfig::File(dir.to_str().unwrap().to_string());
    let config = DdkConfig {
        storage_path: std::env::current_dir().unwrap().join(name).to_str().unwrap().to_string(),
        seed
    };
    let transport = Arc::new(LightningTransport::new("peer_manager", Network::Regtest));
    let storage = Arc::new(SledStorageProvider::new(std::env::current_dir().unwrap().join(name).to_str().unwrap()).unwrap());
    let oracle_client = tokio::task::spawn_blocking(move || Arc::new(P2PDOracleClient::new("http://127.0.0.1:8080").unwrap())).await.unwrap();

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name(name)
        .set_config(config)
        .set_esplora_url(ddk::ESPLORA_HOST)
        .set_network(bitcoin::Network::Regtest)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .await
        .unwrap();

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().await.expect("nope");
}
