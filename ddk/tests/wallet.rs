mod test_util;

use bitcoin::{key::rand::Fill, Network};
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
/// and derivation paths for both expected and stored descriptors for comparison.
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

    let mut seed1 = [0u8; 64];
    seed1
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .expect("Failed to generate random seed");

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

    let mut seed2 = [0u8; 64];
    seed2
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .expect("Failed to generate random seed");

    let result = DlcDevKitWallet::new(
        &seed2,
        esplora.clone(),
        Network::Regtest,
        storage.clone(),
        None,
        logger.clone(),
    )
    .await;

    match result {
        Ok(_) => panic!("Expected DescriptorMismatch error but wallet loaded successfully"),
        Err(WalletError::DescriptorMismatch {
            keychain,
            expected,
            stored,
        }) => {
            let error_msg = format!(
                "{}",
                WalletError::DescriptorMismatch {
                    keychain: keychain.clone(),
                    expected: expected.clone(),
                    stored: stored.clone(),
                }
            );
            println!("\n{}", "=".repeat(80));
            println!("{}", error_msg);
            println!("{}", "=".repeat(80));

            assert_eq!(keychain, "external", "Should identify external keychain");
            assert!(
                !expected.is_empty(),
                "Expected descriptor should not be empty"
            );
            assert!(!stored.is_empty(), "Stored descriptor should not be empty");

            assert!(
                expected.contains("DerivationPath:") || expected.contains("Checksum:"),
                "Expected descriptor should include DerivationPath or Checksum, got: '{}'",
                expected
            );

            assert!(
                expected.contains("Checksum:"),
                "Expected descriptor should include Checksum, got: '{}'",
                expected
            );

            assert!(
                stored.contains("DerivationPath:") || stored.contains("Checksum:"),
                "Stored descriptor should include DerivationPath or Checksum, got: '{}'",
                stored
            );

            assert!(
                stored.contains("Checksum:"),
                "Stored descriptor should include Checksum, got: '{}'",
                stored
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
#[tokio::test]
async fn descriptor_mismatch_error_memory() {
    dotenv::dotenv().ok();
    let storage = Arc::new(MemoryStorage::new()) as Arc<dyn Storage>;
    test_descriptor_mismatch_error_with_storage(storage).await;
}

/// Test descriptor mismatch error with SledStorage backend.
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
