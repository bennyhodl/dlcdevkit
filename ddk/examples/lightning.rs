use bitcoin::key::rand::Fill;
use ddk::builder::{Builder, SeedConfig};
use ddk::logger::{LogLevel, Logger};
use ddk::oracle::kormir::KormirOracleClient;
use ddk::storage::sled::SledStorage;
use ddk::transport::lightning::LightningTransport;
use std::env::current_dir;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[tokio::main]
async fn main() -> Result<(), ddk::error::Error> {
    let logger = Arc::new(Logger::console(
        "lightning_example".to_string(),
        LogLevel::Info,
    ));
    // The transport seed is the node's identity key. Generate it once and
    // persist it; a fixed or all-zero seed gives every node the same identity.
    let mut transport_seed = [0u8; 32];
    transport_seed
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .expect("system randomness is available");
    let transport = Arc::new(LightningTransport::new(
        &transport_seed,
        1776,
        logger.clone(),
    )?);
    let storage = Arc::new(
        SledStorage::new(current_dir().unwrap().to_str().unwrap(), logger.clone()).unwrap(),
    );
    let oracle_client =
        Arc::new(KormirOracleClient::new("http://localhost:8080", None, logger.clone()).await?);

    let mut builder = Builder::new();
    builder.set_seed_bytes(SeedConfig::Random)?;
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());
    builder.set_logger(logger.clone());

    let ddk: ApplicationDdk = builder.finish().await?;

    ddk.start().expect("couldn't start ddk");

    Ok(())
}
