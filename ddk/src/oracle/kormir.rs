use anyhow::anyhow;
use bitcoin::key::XOnlyPublicKey;
use dlc_manager::error::Error;
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
    pub id: u32,
    pub outcome: String,
}

#[derive(Debug)]
pub struct KormirOracleClient {
    pubkey: XOnlyPublicKey,
    client: reqwest::Client,
    host: String,
}

impl KormirOracleClient {
    pub async fn new(host: &str) -> anyhow::Result<KormirOracleClient> {
        tracing::info!(host, "Connecting to Kormir oracle client.");
        let request: String = reqwest::get(format!("{host}/pubkey")).await?.json().await?;
        let pubkey = XOnlyPublicKey::from_str(&request)?;
        let client = reqwest::Client::new();
        tracing::info!(pubkey = pubkey.to_string(), "Connected to Kormir client.");

        Ok(KormirOracleClient {
            pubkey,
            client,
            host: host.to_string(),
        })
    }

    pub async fn get_pubkey(&self) -> anyhow::Result<XOnlyPublicKey> {
        let request = reqwest::get(format!("{}/pubkey", self.host))
            .await?
            .json::<String>()
            .await?;
        Ok(XOnlyPublicKey::from_str(&request)?)
    }

    pub async fn list_events(&self) -> anyhow::Result<Vec<OracleEventData>> {
        let oracle_events: Vec<OracleEventData> =
            reqwest::get(format!("{}/list-events", self.host))
                .await?
                .json()
                .await?;

        Ok(oracle_events)
    }

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

    pub async fn sign_event(
        &self,
        announcement: OracleAnnouncement,
        outcome: String,
    ) -> anyhow::Result<OracleAttestation> {
        let event_id = match self.list_events().await?.iter().find(|event| {
            event.announcement.oracle_event.event_id == announcement.oracle_event.event_id
        }) {
            Some(ann) => ann.id,
            None => return Err(anyhow!("Announcement not found.")),
        };

        let id = match event_id {
            Some(id) => id,
            None => return Err(anyhow!("No id in kormir oracle event data.")),
        };

        let event = SignEnumEvent { id, outcome };

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

impl dlc_manager::Oracle for KormirOracleClient {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        self.pubkey
    }

    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        tracing::info!(event_id, "Getting attestation to close contract.");
        let attestation = get::<OracleAttestation>(&self.host, &format!("attestation/{event_id}"))
            .await
            .map_err(|e| {
                tracing::error!("Attestation: {:?}", e);
                dlc_manager::error::Error::OracleError("Could not get attestation".into())
            })?;
        tracing::info!(event_id, attestation =? attestation, "Attestation");
        Ok(attestation)
    }

    async fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        get::<OracleAnnouncement>(&self.host, "announcement")
            .await
            .map_err(|_| {
                dlc_manager::error::Error::OracleError("Could not get announcement".into())
            })
    }
}

#[async_trait::async_trait]
impl crate::Oracle for KormirOracleClient {
    fn name(&self) -> String {
        "kormir".into()
    }

    async fn get_public_key_async(&self) -> Result<XOnlyPublicKey, dlc_manager::error::Error> {
        Ok(self.pubkey)
    }

    async fn get_announcement_async(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, dlc_manager::error::Error> {
        let announcements = reqwest::get(format!("{}/list-events", &self.host))
            .await
            .map_err(|_| Error::OracleError("Could not get announcements async.".into()))?
            .json::<Vec<OracleEventData>>()
            .await
            .map_err(|_| Error::OracleError("Could not get announcements async.".into()))?;

        let event = announcements
            .iter()
            .find(|event| event.announcement.oracle_event.event_id == event_id);

        match event {
            Some(event_data) => Ok(event_data.announcement.to_owned()),
            None => return Err(Error::OracleError("No event found".to_string())),
        }
    }

    async fn get_attestation_async(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, dlc_manager::error::Error> {
        let attestation = reqwest::get(format!("{}/attestation/{}", &self.host, event_id))
            .await
            .map_err(|_| Error::OracleError("Could not get attestation async.".into()))?
            .json::<OracleAttestation>()
            .await
            .map_err(|_| Error::OracleError("Could not get attestation async.".into()))?;

        Ok(attestation)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta};

    use super::*;
    use crate::test_util::create_oracle_announcement;

    async fn create_kormir() -> KormirOracleClient {
        KormirOracleClient::new("http://127.0.0.1:8082")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_announcement() {
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

        assert!(announcement.is_ok())
    }

    #[tokio::test]
    async fn sign_enum() {
        let kormir = create_kormir().await;

        let announcement = create_oracle_announcement().await;

        let sign_enum = kormir.sign_event(announcement, "rust".to_string()).await;

        assert!(sign_enum.is_ok())
    }
}
