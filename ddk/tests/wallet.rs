mod test_util;

use bitcoin::Network;
use ddk::chain::EsploraClient;
use ddk::error::WalletError;
use ddk::logger::{LogLevel, Logger};
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk::Storage;
use std::sync::Arc;

/// Helper function to test descriptor mismatch error across different storage backends.
///
/// This function creates a wallet with one seed, persists it, then tries to load it
/// with a different seed. It verifies that the error message correctly shows checksums
/// instead of full descriptors.
async fn test_descriptor_mismatch_error_with_storage(storage: Arc<dyn Storage>) {
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
        Ok(_) => panic!("Expected DescriptorMismatch error but wallet loaded successfully"),
        Err(WalletError::DescriptorMismatch {
            keychain,
            expected,
            stored,
        }) => {
            println!("\n{}", "=".repeat(70));
            println!("SUCCESS: Descriptor mismatch error detected");
            println!("{}", "=".repeat(70));
            println!("Keychain: {}", keychain);
            println!("Expected descriptor checksum: {}", expected);
            println!("Stored descriptor checksum: {}", stored);
            println!("{}", "=".repeat(70));

            // Verify the error contains meaningful information
            assert_eq!(keychain, "external", "Should identify external keychain");
            assert!(
                expected.len() > 0,
                "Expected descriptor checksum should not be empty"
            );
            assert!(
                stored.len() > 0,
                "Stored descriptor checksum should not be empty"
            );
            // Verify checksums are valid (8 alphanumeric characters or "unknown")
            // Checksums should be 8 characters (typical) or "unknown" if extraction failed
            assert!(
                expected.len() == 8 || expected == "unknown",
                "Expected checksum should be 8 characters or 'unknown', got: '{}' (length: {})",
                expected,
                expected.len()
            );
            assert!(
                stored.len() == 8 || stored == "unknown" || stored.starts_with("Could not extract"),
                "Stored checksum should be 8 characters, 'unknown', or an error message, got: '{}' (length: {})",
                stored,
                stored.len()
            );
        }
        Err(e) => {
            println!("\n{}", "=".repeat(70));
            println!("UNEXPECTED ERROR TYPE");
            println!("{}", "=".repeat(70));
            println!("Expected DescriptorMismatch, but got: {:?}", e);
            println!("Full error Debug: {:?}", e);
            println!("{}", "=".repeat(70));
            panic!("Expected DescriptorMismatch, got: {:?}", e);
        }
    }
}

/// Test descriptor mismatch error with MemoryStorage backend.
///
/// This test verifies that the descriptor mismatch error message works correctly
/// with the in-memory storage backend.
#[tokio::test]
async fn descriptor_mismatch_error_memory() {
    dotenv::dotenv().ok();
    let storage = Arc::new(MemoryStorage::new()) as Arc<dyn Storage>;
    test_descriptor_mismatch_error_with_storage(storage).await;
}

/// Test descriptor mismatch error with SledStorage backend.
///
/// This test verifies that the descriptor mismatch error message works correctly
/// with the Sled embedded database storage backend.
#[cfg(feature = "sled")]
#[tokio::test]
async fn descriptor_mismatch_error_sled() {
    use ddk::storage::sled::SledStorage;
    use uuid;

    dotenv::dotenv().ok();
    let logger = Arc::new(Logger::console(
        "descriptor_mismatch_sled_test".to_string(),
        LogLevel::Info,
    ));

    // Create a temporary directory for the sled database
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("ddk_test_sled_{}", uuid::Uuid::new_v4()));

    let storage = Arc::new(
        SledStorage::new(db_path.to_str().unwrap(), logger).expect("Failed to create SledStorage"),
    ) as Arc<dyn Storage>;

    test_descriptor_mismatch_error_with_storage(storage).await;

    // Cleanup: remove the temporary database
    if db_path.exists() {
        std::fs::remove_dir_all(&db_path).ok();
    }
}

/// Test descriptor mismatch error with PostgresStorage backend.
///
/// This test verifies that the descriptor mismatch error message works correctly
/// with the PostgreSQL storage backend.
///
/// Note: Requires DATABASE_URL environment variable to be set.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn descriptor_mismatch_error_postgres() {
    use ddk::storage::postgres::PostgresStore;
    use uuid;

    dotenv::dotenv().ok();
    let postgres_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for postgres tests");

    let logger = Arc::new(Logger::console(
        "descriptor_mismatch_postgres_test".to_string(),
        LogLevel::Info,
    ));

    // Create a unique wallet name for this test to avoid conflicts
    let wallet_name = format!("test_wallet_{}", uuid::Uuid::new_v4());

    let storage = Arc::new(
        PostgresStore::new(&postgres_url, true, logger, wallet_name.clone())
            .await
            .expect("Failed to create PostgresStore"),
    ) as Arc<dyn Storage>;

    test_descriptor_mismatch_error_with_storage(storage).await;
}
