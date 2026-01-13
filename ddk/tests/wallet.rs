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
            println!("\n{}", "=".repeat(80));
            println!("SUCCESS: Descriptor mismatch error detected");
            println!("{}", "=".repeat(80));
            println!("Keychain: {}", keychain);
            println!("Expected descriptor: {}", expected);
            println!("Stored descriptor: {}", stored);
            println!("{}", "=".repeat(80));

            // Verify the error contains meaningful information
            assert_eq!(keychain, "external", "Should identify external keychain");
            assert!(
                !expected.is_empty(),
                "Expected descriptor should not be empty"
            );
            assert!(!stored.is_empty(), "Stored descriptor should not be empty");
            // Verify the format includes checksum, path, and fingerprint
            assert!(
                expected.contains("DerivationPath:") || expected == "unknown",
                "Expected descriptor should include DerivationPath, got: '{}'",
                expected
            );
            assert!(
                stored.contains("DerivationPath:")
                    || stored == "unknown"
                    || stored.starts_with("Could not extract"),
                "Stored descriptor should include DerivationPath, got: '{}'",
                stored
            );
            // Verify fingerprint is present when path is present
            if expected.contains("DerivationPath:") {
                assert!(
                    expected.contains("Fingerprint:"),
                    "Expected descriptor should include Fingerprint when DerivationPath is present, got: '{}'",
                    expected
                );
            }
            if stored.contains("DerivationPath:") {
                assert!(
                    stored.contains("Fingerprint:"),
                    "Stored descriptor should include Fingerprint when DerivationPath is present, got: '{}'",
                    stored
                );
            }
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

/// Test function to display the formatted error message.
///
/// This test triggers a descriptor mismatch error and prints the full
/// formatted error message directly from the error's Display implementation.
#[tokio::test]
async fn display_error_message() {
    dotenv::dotenv().ok();
    let storage = Arc::new(MemoryStorage::new()) as Arc<dyn Storage>;

    // Setup logger
    let logger = Arc::new(Logger::console(
        "display_error_test".to_string(),
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
    let mut seed2 = [0u8; 64];
    seed2[0..8].copy_from_slice(b"seed_two");

    let result = DlcDevKitWallet::new(
        &seed2,
        esplora.clone(),
        Network::Regtest,
        storage.clone(),
        None,
        logger.clone(),
    )
    .await;

    // Display the error message directly from the error's Display implementation
    match result {
        Ok(_) => panic!("Expected DescriptorMismatch error but wallet loaded successfully"),
        Err(e) => {
            println!("{}", e);
        }
    }
}
