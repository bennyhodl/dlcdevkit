//! Connects to the live pow-attest oracle and prints a decoded
//! `OracleAnnouncement`.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example pow_attest --features pow-attest
//! ```
//!
//! The default event id below is the static bounty kept on the pow-attest
//! server for downstream-test purposes. Override with the `EVENT_ID`
//! environment variable to point at any registered bounty.

use std::sync::Arc;

use ddk::error::Error;
use ddk::logger::{LogLevel, Logger};
use ddk::oracle::pow_attest::PowAttestOracleClient;

const DEFAULT_HOST: &str = "https://attest.powforge.dev";
const DEFAULT_EVENT_ID: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let logger = Arc::new(Logger::console(
        "pow_attest_example".to_string(),
        LogLevel::Info,
    ));

    let host = std::env::var("POW_ATTEST_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let event_id =
        std::env::var("EVENT_ID").unwrap_or_else(|_| DEFAULT_EVENT_ID.to_string());

    let client = PowAttestOracleClient::new(&host, logger).await?;

    // ddk_manager::Oracle is the trait that exposes get_announcement /
    // get_attestation. Bring it into scope so the methods resolve.
    use ddk_manager::Oracle as _;

    let announcement = client.get_announcement(&event_id).await?;
    println!("oracle_pubkey:        {}", announcement.oracle_public_key);
    println!("event_id:             {}", announcement.oracle_event.event_id);
    println!("nonces:               {}", announcement.oracle_event.oracle_nonces.len());
    println!(
        "maturity_epoch:       {}",
        announcement.oracle_event.event_maturity_epoch
    );

    Ok(())
}
