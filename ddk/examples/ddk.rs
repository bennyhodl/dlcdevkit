use anyhow::Result;
use ddk::builder::DdkBuilder;
use ddk::config::DdkConfig;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

#[tokio::main]
async fn main() -> Result<()> {
    let config = DdkConfig::default();

    let transport = Arc::new(LightningTransport::new(
        &config.seed_config,
        1776,
        config.network,
    )?);
    let storage = Arc::new(SledStorageProvider::new(
        config
            .storage_path
            .join("sled_db")
            .to_str()
            .expect("No storage."),
    )?);

    let oracle_client = Arc::new(P2PDOracleClient::new("host").await.expect("no oracle"));

    let mut builder = DdkBuilder::new();
    builder.set_config(config);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().expect("couldn't start ddk");

    Ok(())
}
