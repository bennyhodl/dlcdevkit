use anyhow::anyhow;
use bitcoin::key::XOnlyPublicKey;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use kormir::storage::OracleEventData;
use lightning::{io::Cursor, util::ser::Readable};
use serde::Serialize;
use std::str::FromStr;
use uuid::Uuid;

async fn get<T>(host: &str, path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}/{}", host, path);
    let request = reqwest::get(url).await?.json::<T>().await?;

    Ok(request)
}

#[derive(Serialize)]
pub struct CreateEnumEvent {
    pub event_id: String,
    pub outcomes: Vec<String>,
    pub event_maturity_epoch: u32,
}

#[derive(Serialize)]
struct SignEnumEvent {
    pub event_id: String,
    pub outcome: String,
}

/// Kormir oracle client.
///
/// Allows the creation of enum announcements and signing as well.
#[derive(Debug)]
pub struct KormirOracleClient {
    pubkey: XOnlyPublicKey,
    client: reqwest::Client,
    host: String,
}

impl KormirOracleClient {
    pub async fn new(host: &str) -> anyhow::Result<KormirOracleClient> {
        let request: String = get(host, "pubkey").await?;
        let pubkey = XOnlyPublicKey::from_str(&request)?;
        let client = reqwest::Client::new();
        tracing::info!(
            host,
            pubkey = pubkey.to_string(),
            "Connected to Kormir client."
        );

        Ok(KormirOracleClient {
            pubkey,
            client,
            host: host.to_string(),
        })
    }

    pub async fn get_pubkey(&self) -> anyhow::Result<XOnlyPublicKey> {
        Ok(self.pubkey)
    }

    /// List all events stored with the connected Kormir server.
    ///
    /// Kormir events includes announcements info, nonce index, signatures
    /// if announcement has been signed, and nostr information.
    pub async fn list_events(&self) -> anyhow::Result<Vec<OracleEventData>> {
        get(&self.host, "list-events").await.map_err(|e| {
            tracing::error!(error = e.to_string(), "Error getting all kormir events.");
            anyhow!("List events")
        })
    }

    /// Creates an enum oracle announcament.
    ///
    /// Maturity should be the UNIX timestamp of contract maturity.
    pub async fn create_event(
        &self,
        outcomes: Vec<String>,
        maturity: u32,
    ) -> anyhow::Result<OracleAnnouncement> {
        let event_id = Uuid::new_v4().to_string();

        let create_event_request = CreateEnumEvent {
            event_id: event_id.clone(),
            outcomes,
            event_maturity_epoch: maturity,
        };

        let announcement = self
            .client
            .post(format!("{}/create-enum", self.host))
            .json(&create_event_request)
            .send()
            .await?
            .text()
            .await?
            .trim_matches('"')
            .to_string();

        let announcement_bytes = hex::decode(&announcement)?;
        let mut cursor = Cursor::new(&announcement_bytes);
        let announcement: OracleAnnouncement = Readable::read(&mut cursor)
            .map_err(|_| anyhow!("Can't read bytes for attestation."))?;

        Ok(announcement)
    }

    /// Requests for Kormir to sign an announcement with a given outcome.
    pub async fn sign_event(
        &self,
        event_id: String,
        outcome: String,
    ) -> anyhow::Result<OracleAttestation> {
        tracing::info!("Signing event. event_id={} outcome={}", event_id, outcome);

        let event = SignEnumEvent { event_id, outcome };

        let hex = self
            .client
            .post(format!("{}/sign-enum", &self.host))
            .json(&event)
            .send()
            .await?
            .text()
            .await?
            .trim_matches('"')
            .to_string();

        let attestion_buffer = hex::decode(hex)?;

        let mut cursor = lightning::io::Cursor::new(attestion_buffer);
        let attestation: OracleAttestation = Readable::read(&mut cursor)
            .map_err(|_| anyhow!("Can't read bytes for attestation."))?;

        tracing::info!("Signed Kormir oracle event.");

        Ok(attestation)
    }
}

#[async_trait::async_trait]
impl ddk_manager::Oracle for KormirOracleClient {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        self.pubkey
    }

    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, ddk_manager::error::Error> {
        tracing::info!(event_id, "Getting attestation to close contract.");
        let attestation = get::<OracleAttestation>(&self.host, &format!("attestation/{event_id}"))
            .await
            .map_err(|e| {
                tracing::error!(error=?e, "Could not get attestation.");
                ddk_manager::error::Error::OracleError("Could not get attestation".into())
            })?;
        tracing::info!(event_id, attestation =? attestation, "Kormir attestation.");
        Ok(attestation)
    }

    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, ddk_manager::error::Error> {
        tracing::info!(event_id, "Getting oracle announcement.");
        let announcement =
            get::<OracleAnnouncement>(&self.host, &format!("announcement/{event_id}"))
                .await
                .map_err(|e| {
                    tracing::error!(error =? e, "Could not get announcement.");
                    ddk_manager::error::Error::OracleError("Could not get announcement".into())
                })?;
        tracing::info!(event_id, announcement=?announcement, "Kormir announcement.");
        Ok(announcement)
    }
}

impl crate::Oracle for KormirOracleClient {
    fn name(&self) -> String {
        "kormir".into()
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta};

    use super::*;

    async fn create_kormir() -> KormirOracleClient {
        KormirOracleClient::new("https://kormir.dlcdevkit.com")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn kormir() {
        let kormir = create_kormir().await;

        let expiry = TimeDelta::seconds(30);
        let timestamp: u32 = Local::now()
            .checked_add_signed(expiry)
            .unwrap()
            .timestamp()
            .try_into()
            .unwrap();

        let announcement = kormir
            .create_event(vec!["rust".to_string(), "go".to_string()], timestamp)
            .await;

        assert!(announcement.is_ok());

        let sign_enum = kormir
            .sign_event(
                announcement.unwrap().oracle_event.event_id,
                "rust".to_string(),
            )
            .await;

        assert!(sign_enum.is_ok())
    }
}
