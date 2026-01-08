//! Example to trigger the descriptor mismatch error at line 262 in wallet/mod.rs
//!
//! This example creates a wallet with one seed, then tries to load it with a different seed.
//! Since the descriptors won't match, it will trigger the error at line 262.

use ddk::chain::EsploraClient;
use ddk::error::WalletError;
use ddk::logger::{LogLevel, Logger};
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use bitcoin::Network;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup logger
    let logger = Arc::new(Logger::console(
        "descriptor_mismatch_example".to_string(),
        LogLevel::Info,
    ));

    // Setup Esplora client (using regtest as default)
    let esplora_host = std::env::var("ESPLORA_HOST")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());
    let esplora = Arc::new(
        EsploraClient::new(&esplora_host, Network::Regtest, logger.clone())
            .map_err(|e| format!("Failed to create Esplora client: {}", e))?,
    );

    // Create shared storage
    let storage = Arc::new(MemoryStorage::new());

    // First seed - this will create and persist a wallet
    let mut seed1 = [0u8; 64];
    seed1[0..8].copy_from_slice(b"seed_one");
    println!("Creating wallet with seed1...");
    
    let _wallet1 = DlcDevKitWallet::new(
        &seed1,
        esplora.clone(),
        Network::Regtest,
        storage.clone(),
        None,
        logger.clone(),
    )
    .await?;
    
    println!("✓ Wallet 1 created successfully");

    // Second seed - this will try to load the existing wallet but with different descriptors
    // This should fail at line 262 because the descriptors won't match
    let mut seed2 = [0u8; 64];
    seed2[0..8].copy_from_slice(b"seed_two");
    println!("\nAttempting to load wallet with seed2 (different descriptors)...");
    
    match DlcDevKitWallet::new(
        &seed2,
        esplora.clone(),
        Network::Regtest,
        storage.clone(), // Same storage, but different seed
        None,
        logger.clone(),
    )
    .await
    {
        Ok(_) => {
            println!("✗ Unexpected: Wallet loaded successfully (this shouldn't happen)");
            Err("Expected error but wallet loaded successfully".into())
        }
        Err(WalletError::WalletPersistanceError(e)) => {
            println!("✓ Successfully triggered the error at line 262!");
            println!("  Error message: {}", e);
            Ok(())
        }
        Err(e) => {
            println!("✗ Got a different error than expected:");
            println!("  Error: {:?}", e);
            Err(format!("Expected WalletPersistanceError, got: {:?}", e).into())
        }
    }
}
