use bitcoin::key::rand::Fill;
use ddk::builder::{Builder, SeedConfig};
use ddk::logger::{LogLevel, Logger};
use ddk::oracle::kormir::KormirOracleClient;
use ddk::storage::postgres::PostgresStore;
use ddk::transport::lightning::LightningTransport;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, PostgresStore, KormirOracleClient>;

#[tokio::main]
async fn main() -> Result<(), ddk::error::Error> {
    let logger = Arc::new(Logger::console(
        "console_logger".to_string(),
        LogLevel::Info,
    ));
    let transport = Arc::new(LightningTransport::new(&[0u8; 32], 1776, logger.clone())?);
    let storage = Arc::new(
        PostgresStore::new(
            &std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            false,
            logger.clone(),
            "test".to_string(),
        )
        .await?,
    );
    let oracle_client = Arc::new(
        KormirOracleClient::new("https://kormir.dlcdevkit.com", None, logger.clone()).await?,
    );

    let mut seed_bytes = [0u8; 64];
    seed_bytes
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();

    let mut builder = Builder::new();
    builder.set_seed_bytes(SeedConfig::Bytes(seed_bytes))?;
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());
    builder.set_logger(logger.clone());

    let ddk: ApplicationDdk = builder.finish().await?;

    ddk.start().expect("couldn't start ddk");

    Ok(())
}
