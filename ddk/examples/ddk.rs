use anyhow::Result;
use ddk::config::DdkConfig;
use ddk::builder::DdkBuilder;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use std::env::current_dir;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn main() -> Result<()> {
    let mut config = DdkConfig::default();
    config.storage_path = current_dir()?;

    let transport = Arc::new(LightningTransport::new(&config.seed_config, config.network)?);
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().expect("No storage."),
    )?);

    let oracle_client = Arc::new(P2PDOracleClient::new(ddk::ORACLE_HOST).expect("no oracle"));

    let mut builder = DdkBuilder::new();
    builder.set_config(config);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().expect("couldn't start ddk");

    loop {}
}
