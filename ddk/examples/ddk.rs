use anyhow::Result;
use ddk::builder::DdkBuilder;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorage;
use ddk::transport::lightning::LightningTransport;
use std::env::current_dir;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorage, P2PDOracleClient>;

#[tokio::main]
async fn main() -> Result<()> {
    let transport = Arc::new(LightningTransport::new(&[0u8; 32], 1776)?);
    let storage = Arc::new(SledStorage::new(current_dir()?.to_str().unwrap())?);

    let oracle_client = Arc::new(P2PDOracleClient::new("host").await?);

    let mut builder = DdkBuilder::new();
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;

    ddk.start().expect("couldn't start ddk");

    Ok(())
}
