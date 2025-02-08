use std::str::FromStr;
use std::time::Duration;

use bitcoin::XOnlyPublicKey;
use kormir::{OracleAnnouncement, OracleAttestation};
use nostr_database::MemoryDatabase;
use nostr_database::NostrEventsDatabase;
use nostr_rs::event::EventId;
use nostr_rs::key::PublicKey as NostrPublicKey;
use nostr_rs::types::{Timestamp, TryIntoUrl};
use nostr_sdk::Client;
use nostr_sdk::Filter;

#[derive(Debug, thiserror::Error)]
pub enum NostrOracleError {
    #[error("Failed to make subscription.")]
    FailedToMakeSubscription,
    #[error("Failed to convert Nostr public key to XOnlyPublicKey.")]
    XonlyConversionError,
}

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
    ) -> Result<Self, NostrOracleError> {
        let xonly_oracle_pubkey = XOnlyPublicKey::from_slice(nostr_oracle_pubkey.as_bytes())
            .map_err(|_| NostrOracleError::XonlyConversionError)?;

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
            .map_err(|_| NostrOracleError::FailedToMakeSubscription)?;

        let db = MemoryDatabase::new();

        Ok(Self {
            client,
            db,
            xonly_oracle_pubkey,
            nostr_oracle_pubkey,
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

    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, ddk_manager::error::Error> {
        let event_id = EventId::from_str(event_id).map_err(|_| {
            ddk_manager::error::Error::OracleError(format!("Invalid event id: {}", event_id))
        })?;
        if let Ok(event) = self.db.event_by_id(&event_id).await {
            if let Some(event) = event {
                return Ok(serde_json::from_str(&event.content).unwrap());
            }
        }

        let event = self
            .client
            .fetch_events(Filter::new().event(event_id), Duration::from_secs(10))
            .await
            .map_err(|_| {
                ddk_manager::error::Error::OracleError(format!(
                    "Failed to fetch event: {}",
                    event_id
                ))
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

    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, ddk_manager::error::Error> {
        let event_id = EventId::from_str(event_id).map_err(|_| {
            ddk_manager::error::Error::OracleError(format!("Invalid event id: {}", event_id))
        })?;
        if let Ok(event) = self.db.event_by_id(&event_id).await {
            if let Some(event) = event {
                return Ok(serde_json::from_str(&event.content).unwrap());
            }
        }

        let event = self
            .client
            .fetch_events(Filter::new().event(event_id), Duration::from_secs(10))
            .await
            .map_err(|_| {
                ddk_manager::error::Error::OracleError(format!(
                    "Failed to fetch event: {}",
                    event_id
                ))
            })?;

        if let Some(event) = event.first() {
            let attestation = serde_json::from_str(&event.content).unwrap();
            let _ = self.db.save_event(event).await;
            return Ok(attestation);
        }

        Err(ddk_manager::error::Error::OracleError(
            "No event found".to_string(),
        ))
    }
}
