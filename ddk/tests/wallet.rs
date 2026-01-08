mod test_util;

use ddk::chain::EsploraClient;
use ddk::error::WalletError;
use ddk::logger::{LogLevel, Logger};
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk::Storage;
use bitcoin::Network;
use std::sync::Arc;

/// Test to trigger the descriptor mismatch error at line 262 in wallet/mod.rs
///
/// This test creates a wallet with one seed, then tries to load it with a different seed.
/// Since the descriptors won't match, it will trigger the error at line 262.
#[tokio::test]
async fn descriptor_mismatch_error() {
    dotenv::dotenv().ok();
    
    // Setup logger
    let logger = Arc::new(Logger::console(
        "descriptor_mismatch_test".to_string(),
        LogLevel::Info,
    ));

    // Setup Esplora client
    let esplora_host = std::env::var("ESPLORA_HOST").expect("ESPLORA_HOST must be set");
    let esplora = Arc::new(
        EsploraClient::new(&esplora_host, Network::Regtest, logger.clone())
            .expect("Failed to create Esplora client"),
    );

    // Create shared storage
    let storage = Arc::new(MemoryStorage::new()) as Arc<dyn Storage>;

    // First seed - this will create and persist a wallet
    let mut seed1 = [0u8; 64];
    seed1[0..8].copy_from_slice(b"seed_one");
    
    let _wallet1 = DlcDevKitWallet::new(
        &seed1,
        esplora.clone(),
        Network::Regtest,
        storage.clone(),
        None,
        logger.clone(),
    )
    .await
    .expect("Failed to create first wallet");

    // Second seed - this will try to load the existing wallet but with different descriptors
    // This should fail at line 262 because the descriptors won't match
    let mut seed2 = [0u8; 64];
    seed2[0..8].copy_from_slice(b"seed_two");
    
    let result = DlcDevKitWallet::new(
        &seed2,
        esplora.clone(),
        Network::Regtest,
        storage.clone(), // Same storage, but different seed
        None,
        logger.clone(),
    )
    .await;

    // Verify we got the expected error
    match result {
        Ok(_) => panic!("Expected WalletPersistanceError but wallet loaded successfully"),
        Err(WalletError::WalletPersistanceError(e)) => {

            println!("\n{}", "=".repeat(70));
            println!("   {:?}", WalletError::WalletPersistanceError(e.clone()));
            println!("\n{}", "=".repeat(70));
            assert!(
                e.len() > 0,
                "Error message should not be empty"
            );
        }
        Err(e) => {
            println!("\n{}", "=".repeat(70));
            println!("UNEXPECTED ERROR TYPE");
            println!("{}", "=".repeat(70));
            println!("Expected WalletPersistanceError, but got: {:?}", e);
            println!("Full error Debug: {:?}", e);
            println!("{}", "=".repeat(70));
            panic!("Expected WalletPersistanceError, got: {:?}", e);
        }
    }
}
