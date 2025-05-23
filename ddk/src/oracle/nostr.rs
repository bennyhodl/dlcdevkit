use std::str::FromStr;
use std::time::Duration;

use crate::error::OracleError;
use bitcoin::XOnlyPublicKey;
use ddk_manager::error::Error as ManagerError;
use kormir::{OracleAnnouncement, OracleAttestation};
use lightning::io::Cursor;
use lightning::util::ser::Readable;
use nostr_database::MemoryDatabase;
use nostr_database::NostrEventsDatabase;
use nostr_rs::event::EventId;
use nostr_rs::event::Kind;
use nostr_rs::key::PublicKey as NostrPublicKey;
use nostr_rs::types::{Timestamp, TryIntoUrl};
use nostr_sdk::Client;
use nostr_sdk::Filter;
use nostr_sdk::RelayPoolNotification;
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct NostrOracle {
    client: Client,
    db: nostr_database::MemoryDatabase,
    xonly_oracle_pubkey: XOnlyPublicKey,
    nostr_oracle_pubkey: NostrPublicKey,
}

impl NostrOracle {
    pub async fn new<U: TryIntoUrl>(
        relays: Vec<U>,
        since: Option<Timestamp>,
        nostr_oracle_pubkey: NostrPublicKey,
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
                tracing::error!("Invalid relay URL.");
            }
        }

        client.connect().await;

        let since = since.unwrap_or(Timestamp::now());
        let filter = crate::nostr::create_oracle_message_filter(since);

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
        })
    }

    pub fn start(
        &self,
        mut stop_signal: watch::Receiver<bool>,
    ) -> JoinHandle<Result<(), OracleError>> {
        tracing::info!(
            pubkey = self.nostr_oracle_pubkey.to_string(),
            "Starting Nostr Oracle listener."
        );
        let nostr_client = self.client.clone();
        let db = self.db.clone();
        tokio::spawn(async move {
            tracing::info!("Listening for Oracle messages.");
            let mut notifications = nostr_client.notifications();
            loop {
                tokio::select! {
                    _ = stop_signal.changed() => {
                        if *stop_signal.borrow() {
                            tracing::warn!("Stopping nostr oracle subscription.");
                            nostr_client.disconnect().await;
                            break;
                        }
                    },
                    Ok(notification) = notifications.recv() => {
                        tracing::info!("Received notification {:?}", notification);
                        match notification {
                            RelayPoolNotification::Event {
                                relay_url: _,
                                subscription_id: _,
                                event,
                            } => {

                                match event.kind {
                                    Kind::Custom(88) => {
                                        if let Ok(announcement) = decode_base64::<OracleAnnouncement>(&event.content) {
                                            tracing::info!("Received announcement event: {}", announcement.oracle_event.event_id);
                                            let _ = db.save_event(&event).await;
                                        }
                                    }
                                    Kind::Custom(89) => {
                                        if let Ok(attestation) = decode_base64::<OracleAttestation>(&event.content) {
                                            tracing::info!("Received attestation event: {}", attestation.event_id);
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

    async fn get_announcement(&self, event_id: &str) -> Result<OracleAnnouncement, ManagerError> {
        tracing::info!("Getting announcement for event id: {}", event_id);
        let event_id = EventId::from_str(event_id)
            .map_err(|_| ManagerError::OracleError(format!("Invalid event id: {}", event_id)))?;

        if let Ok(event) = self.db.event_by_id(&event_id).await {
            tracing::info!("Event found in db: {:?}", event);
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

fn decode_base64<T: Readable>(content: &str) -> Result<T, OracleError> {
    let bytes = base64::decode(content)
        .map_err(|_| OracleError::Custom("Failed to decode base64.".to_string()))?;
    let mut cursor = Cursor::new(bytes);
    T::read(&mut cursor).map_err(|_| OracleError::Custom("Failed to read event.".to_string()))
}

#[cfg(test)]
mod tests {
    use bitcoin::bip32::Xpriv;
    use ddk_manager::Oracle;
    use nostr_rs::event::Event;
    use std::sync::Arc;

    use super::*;

    async fn test_send_announcement(key: nostr_rs::key::Keys) -> (OracleAnnouncement, Event) {
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
        nostr_client.add_relay("ws://localhost:8081").await.unwrap();
        nostr_client.connect().await;
        nostr_client.send_event(&ann_event).await.unwrap();
        (announcement, ann_event)
    }

    #[tokio::test]
    async fn handle_oracle_announcement_test() {
        let nostr_keys = nostr_rs::key::Keys::generate();
        let (announcement, event) = test_send_announcement(nostr_keys).await;
        let decoded = decode_base64::<OracleAnnouncement>(&event.content).unwrap();
        assert_eq!(announcement, decoded);
    }

    #[test_log::test(tokio::test)]
    async fn nostr_oracle_receives_announcement() {
        let nostr_keys = nostr_rs::key::Keys::generate();
        let (stop_signal_sender, stop_signal_receiver) = watch::channel(false);
        let nostr_oracle = Arc::new(
            NostrOracle::new(
                vec!["ws://localhost:8081".to_string()],
                None,
                nostr_keys.public_key,
            )
            .await
            .unwrap(),
        );

        let nostr = nostr_oracle.clone();
        tokio::spawn(async move {
            let _ = nostr.start(stop_signal_receiver).await;
        });

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let announcement = test_send_announcement(nostr_keys).await;

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let received_announcement = nostr_oracle
            .get_announcement(&announcement.1.id.to_string())
            .await
            .unwrap();

        tracing::info!("Received announcement: {:?}", received_announcement);

        let _ = stop_signal_sender.send(true);
    }
}
