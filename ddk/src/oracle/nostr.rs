use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::error::OracleError;
use crate::logger::Logger;
use crate::logger::{log_debug, log_error, log_info, log_warn, WriteLog};
use bitcoin::XOnlyPublicKey;
use ddk_manager::error::Error as ManagerError;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use ddk_messages::TlvRecord;
use nostr_database::MemoryDatabase;
use nostr_database::NostrDatabase;
use nostr_rs::event::EventId;
use nostr_rs::event::Kind;
use nostr_rs::key::PublicKey as NostrPublicKey;
use nostr_rs::types::{Timestamp, TryIntoUrl};
use nostr_sdk::Client;
use nostr_sdk::Filter;
use nostr_sdk::RelayPoolNotification;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// A Nostr-based oracle implementation for DLCs that listens for and processes
/// oracle announcements and attestations over the Nostr network.
///
/// This oracle maintains connections to Nostr relays, processes incoming events,
/// and stores them in a memory database for quick retrieval.
///
/// # Components
/// * `client` - Nostr client for connecting to relays and handling messages
/// * `db` - In-memory database for caching oracle events
/// * `xonly_oracle_pubkey` - Bitcoin-compatible x-only public key for DLC operations
/// * `nostr_oracle_pubkey` - Nostr-format public key for message verification
#[derive(Debug)]
pub struct NostrOracle {
    /// Nostr client for connecting to relays and handling message subscriptions
    client: Client,
    /// In-memory database for storing oracle events (announcements and attestations)
    db: nostr_database::MemoryDatabase,
    /// Bitcoin x-only public key used for DLC operations
    xonly_oracle_pubkey: XOnlyPublicKey,
    /// Nostr public key for message verification and routing
    nostr_oracle_pubkey: NostrPublicKey,
    /// [`ddk::logger::Logger`] instance for logging
    logger: Arc<Logger>,
}

impl NostrOracle {
    /// Creates a new NostrOracle instance and establishes connections to the specified relays.
    ///
    /// This function:
    /// 1. Converts the Nostr public key to Bitcoin format for DLC operations
    /// 2. Initializes a Nostr client and connects to all provided relays
    /// 3. Sets up a subscription for oracle announcements (kind 88) and attestations (kind 89)
    /// 4. Initializes an in-memory database for event caching
    ///
    /// # Arguments
    /// * `relays` - List of Nostr relay URLs to connect to
    /// * `since` - Optional timestamp to filter events from (defaults to now if None)
    /// * `nostr_oracle_pubkey` - The oracle's Nostr public key for message verification
    /// * `logger` - [`ddk::logger::Logger`] instance for logging
    ///
    /// # Returns
    /// * `Ok(NostrOracle)` - Successfully initialized oracle
    /// * `Err(OracleError)` - If initialization fails (invalid key, connection issues, etc.)
    pub async fn new<U: TryIntoUrl>(
        relays: Vec<U>,
        since: Option<Timestamp>,
        nostr_oracle_pubkey: NostrPublicKey,
        logger: Arc<Logger>,
    ) -> Result<Self, OracleError> {
        let xonly_oracle_pubkey = XOnlyPublicKey::from_slice(nostr_oracle_pubkey.as_bytes())
            .map_err(|_| {
                OracleError::Init(
                    "Failed to convert Nostr public key to XOnlyPublicKey.".to_string(),
                )
            })?;

        let client = Client::default();

        for relay in relays {
            if let Ok(url) = relay.try_into_url() {
                client.add_relay(url).await.unwrap();
            } else {
                log_error!(logger, "Invalid relay URL.");
            }
        }

        client.connect().await;

        let since = since.unwrap_or(Timestamp::now());
        let filter = crate::nostr::messages::create_oracle_message_filter(since);

        client
            .subscribe(filter, None)
            .await
            .map_err(|_| OracleError::Init("Failed to make subscription.".to_string()))?;

        let db = MemoryDatabase::new();

        Ok(Self {
            client,
            db,
            xonly_oracle_pubkey,
            nostr_oracle_pubkey,
            logger,
        })
    }

    /// Starts the oracle's event processing loop in a separate task.
    ///
    /// This function spawns a new tokio task that:
    /// 1. Listens for incoming Nostr events (announcements and attestations)
    /// 2. Decodes and validates the events
    /// 3. Stores valid events in the in-memory database
    /// 4. Can be gracefully shut down using the stop signal
    ///
    /// # Arguments
    /// * `stop_signal` - Watch channel receiver for graceful shutdown
    ///
    /// # Returns
    /// * `JoinHandle` - Handle to the spawned task
    ///
    /// The task will run until the stop signal is set to true, at which point it will:
    /// 1. Disconnect from all relays
    /// 2. Stop processing events
    /// 3. Clean up resources
    ///
    /// # Event Processing
    /// - Kind 88: Oracle announcements (future events)
    /// - Kind 89: Oracle attestations (event outcomes)
    pub fn start(
        &self,
        mut stop_signal: watch::Receiver<bool>,
    ) -> JoinHandle<Result<(), OracleError>> {
        log_info!(
            self.logger,
            "Starting Nostr Oracle listener. pubkey={}",
            self.nostr_oracle_pubkey.to_string()
        );
        let nostr_client = self.client.clone();
        let db = self.db.clone();
        let logger = self.logger.clone();
        tokio::spawn(async move {
            log_info!(logger, "Listening for Oracle messages.");
            let mut notifications = nostr_client.notifications();
            loop {
                tokio::select! {
                    _ = stop_signal.changed() => {
                        if *stop_signal.borrow() {
                            log_warn!(logger, "Stopping nostr oracle subscription.");
                            nostr_client.disconnect().await;
                            break;
                        }
                    },
                    Ok(notification) = notifications.recv() => {
                        log_info!(logger, "Received notification {:?}", notification);
                        match notification {
                            RelayPoolNotification::Event {
                                relay_url: _,
                                subscription_id: _,
                                event,
                            } => {

                                match event.kind {
                                    Kind::Custom(88) => {
                                        if let Ok(announcement) = decode_base64::<OracleAnnouncement>(&event.content) {
                                            log_info!(logger, "Received announcement event: {}", announcement.oracle_event.event_id);
                                            let _ = db.save_event(&event).await;
                                        }
                                    }
                                    Kind::Custom(89) => {
                                        if let Ok(attestation) = decode_base64::<OracleAttestation>(&event.content) {
                                            log_info!(logger, "Received attestation event: {}", attestation.event_id);
                                            let _ =db.save_event(&event).await;
                                        }
                                    }
                                    _ => ()
                                }
                            }
                            _ => ()
                        }
                    }
                }
            }
            Ok::<_, OracleError>(())
        })
    }
}

impl crate::Oracle for NostrOracle {
    fn name(&self) -> String {
        "nostr".to_string()
    }
}

#[async_trait::async_trait]
impl ddk_manager::Oracle for NostrOracle {
    fn get_public_key(&self) -> XOnlyPublicKey {
        self.xonly_oracle_pubkey
    }

    #[tracing::instrument(skip(self))]
    async fn get_announcement(&self, event_id: &str) -> Result<OracleAnnouncement, ManagerError> {
        let event_id = EventId::from_str(event_id)
            .map_err(|_| ManagerError::OracleError(format!("Invalid event id: {}", event_id)))?;

        if let Ok(event) = self.db.event_by_id(&event_id).await {
            log_debug!(
                self.logger,
                "Event found in nostr database. event_id={}",
                event_id
            );
            if let Some(event) = event {
                return Ok(decode_base64::<OracleAnnouncement>(&event.content).unwrap());
            }
        }

        let event = self
            .client
            .fetch_events(
                Filter::new().event(event_id).since(Timestamp::zero()),
                Duration::from_secs(10),
            )
            .await
            .map_err(|_| {
                ManagerError::OracleError(format!("Failed to fetch event: {}", event_id))
            })?;

        if let Some(event) = event.first() {
            let announcement = serde_json::from_str(&event.content).unwrap();
            let _ = self.db.save_event(event).await;
            return Ok(announcement);
        }

        Err(ddk_manager::error::Error::OracleError(
            "No event found".to_string(),
        ))
    }

    #[tracing::instrument(skip(self))]
    async fn get_attestation(&self, event_id: &str) -> Result<OracleAttestation, ManagerError> {
        let event_id = EventId::from_str(event_id)
            .map_err(|_| ManagerError::OracleError(format!("Invalid event id: {}", event_id)))?;
        if let Ok(event) = self.db.event_by_id(&event_id).await {
            if let Some(event) = event {
                return Ok(decode_base64::<OracleAttestation>(&event.content).unwrap());
            }
        }

        let event = self
            .client
            .fetch_events(Filter::new().event(event_id), Duration::from_secs(10))
            .await
            .map_err(|_| {
                ManagerError::OracleError(format!("Failed to fetch event: {}", event_id))
            })?;

        if let Some(event) = event.first() {
            let attestation = serde_json::from_str(&event.content).unwrap();
            let _ = self.db.save_event(event).await;
            return Ok(attestation);
        }

        Err(ManagerError::OracleError("No event found".to_string()))
    }
}

/// Decodes an oracle message from the base64 content of a nostr event.
///
/// Events carry the message in its standalone TLV form. Events published before that
/// was settled carry the TLV body alone, and a relay keeps them forever, so both forms
/// are accepted here.
fn decode_base64<T: TlvRecord>(content: &str) -> Result<T, OracleError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content)
        .map_err(|_| OracleError::Custom("Failed to decode base64.".to_string()))?;
    T::from_tlv_bytes_or_legacy(&bytes)
        .map_err(|_| OracleError::Custom("Failed to read event.".to_string()))
}

#[cfg(test)]
mod tests {
    use bitcoin::bip32::Xpriv;
    use nostr_rs::event::Event;

    use super::*;

    const REAL_ANNOUNCEMENT_HEX: &str = "fdd824fd012a73740e61118e5d1c2c223c986b859a42c2cca56cb621d13ff8880ea856caf24aa29bc36a1e0d5c471b6a2baa68f98a413362c4e6a97f13c04b186395e9e0d8fcc3d07289c2ade25405c1c421b38c9322cd73fb2c89f42ce0730a35fae1f8875dfdd822c60001529fadc9958e1e1cf8ea29f05a25d67e04a0c21dcbfcb99b8b24d32f274795eb68e81101fdd8064e0004086e6f742d70616964067265706169641d6c6971756964617465642d62792d6d617475726174696f6e2d646174651d6c6971756964617465642d62792d70726963652d7468726573686f6c644d6c6f616e2d6d6174757265642d38313233313935633631653439376631323465623764336266626531323232613530326233306162343139363766306466323036306133656533366635623063";

    /// Announcements published to a relay before this crate wrote the TLV form
    /// carry the bare body. A relay keeps them forever, so a client upgrading to
    /// this version must still be able to read what is already out there.
    #[test]
    fn legacy_body_form_nostr_events_still_decode() {
        use base64::Engine as _;
        use lightning::util::ser::Writeable;

        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX).unwrap();

        let legacy = base64::engine::general_purpose::STANDARD.encode(announcement.encode());
        let current = base64::engine::general_purpose::STANDARD.encode(announcement.to_tlv_bytes());
        assert_ne!(legacy, current, "the two forms must actually differ");

        assert_eq!(
            decode_base64::<OracleAnnouncement>(&legacy).unwrap(),
            announcement
        );
        assert_eq!(
            decode_base64::<OracleAnnouncement>(&current).unwrap(),
            announcement
        );
    }

    async fn test_send_announcement(
        key: nostr_rs::key::Keys,
        relay_url: &str,
    ) -> (OracleAnnouncement, Event) {
        let xpriv =
            Xpriv::new_master(bitcoin::Network::Regtest, &key.secret_key().secret_bytes()).unwrap();
        let storage = kormir::storage::MemoryStorage::default();
        let oracle = kormir::Oracle::new(storage, xpriv.private_key, xpriv);
        let announcement = oracle
            .create_numeric_event(
                "nostr-oracle-test".to_string(),
                20,
                false,
                2,
                "btc/usd".to_string(),
                std::time::SystemTime::now()
                    .checked_add(std::time::Duration::from_secs(10))
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as u32,
            )
            .await
            .unwrap();
        let ann_event =
            kormir::nostr_events::create_announcement_event(&oracle.nostr_keys(), &announcement)
                .unwrap();

        let nostr_client = nostr_sdk::Client::new(key);
        nostr_client.add_relay(relay_url).await.unwrap();
        nostr_client.connect().await;
        nostr_client.send_event(&ann_event).await.unwrap();
        (announcement, ann_event)
    }

    #[tokio::test]
    async fn handle_oracle_announcement_test() {
        // Held for the duration of the test: the relay shuts down when dropped.
        let relay = ddk_testenv::nostr::TestRelay::start().await;
        let nostr_keys = nostr_rs::key::Keys::generate();
        let (announcement, event) = test_send_announcement(nostr_keys, relay.url()).await;
        let decoded = decode_base64::<OracleAnnouncement>(&event.content).unwrap();
        assert_eq!(announcement, decoded);
    }
}
