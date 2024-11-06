use anyhow::Result;
use bitcoin::key::rand::Fill;
use ddk::builder::Builder;
use ddk::oracle::kormir::KormirOracleClient;
use ddk::storage::sled::SledStorage;
use ddk::transport::lightning::LightningTransport;
use std::env::current_dir;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[tokio::main]
async fn main() -> Result<()> {
    let transport = Arc::new(LightningTransport::new(&[0u8; 32], 1776)?);
    let storage = Arc::new(SledStorage::new(current_dir()?.to_str().unwrap())?);
    let oracle_client = Arc::new(KormirOracleClient::new("host").await?);

    let mut seed_bytes = [0u8; 32];
    seed_bytes.try_fill(&mut bitcoin::key::rand::thread_rng())?;

    let mut builder = Builder::new();
    builder.set_seed_bytes(seed_bytes);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;

    ddk.start().expect("couldn't start ddk");

    Ok(())
}
