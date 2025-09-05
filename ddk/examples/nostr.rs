use bitcoin::key::rand::Fill;
use bitcoin::Network;
use ddk::builder::{Builder, SeedConfig};
use ddk::logger::{LogLevel, Logger};
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::transport::nostr::NostrDlc;
use std::sync::Arc;

type NostrDdk = ddk::DlcDevKit<NostrDlc, MemoryStorage, MemoryOracle>;

#[tokio::main]
async fn main() -> Result<(), ddk::error::Error> {
    let mut seed_bytes = [0u8; 64];
    seed_bytes
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();
    let logger = Arc::new(Logger::console("nostr_example".to_string(), LogLevel::Info));

    let transport = Arc::new(
        NostrDlc::new(
            &seed_bytes,
            "wss://nostr.dlcdevkit.com",
            Network::Regtest,
            logger.clone(),
        )
        .await?,
    );
    let storage = Arc::new(MemoryStorage::new());
    let oracle_client = Arc::new(MemoryOracle::default());

    let mut builder = Builder::new();
    builder.set_seed_bytes(SeedConfig::Bytes(seed_bytes))?;
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());
    builder.set_logger(logger.clone());

    let ddk: NostrDdk = builder.finish().await?;

    ddk.start().expect("couldn't start ddk");

    loop {}
}
