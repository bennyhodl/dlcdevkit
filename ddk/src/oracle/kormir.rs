use bitcoin::key::XOnlyPublicKey;
use dlc_manager::error::Error;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use kormir::storage::OracleEventData;
use serde::Serialize;
use std::str::FromStr;
use uuid::Uuid;

fn get<T>(host: &str, path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}{}", host, path);
    let request = reqwest::blocking::get(url)?.json::<T>()?;

    Ok(request)
}

#[derive(Serialize)]
pub struct CreateEnumEvent {
    pub event_id: String,
    pub outcomes: Vec<String>,
    pub event_maturity_epoch: u32,
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

    pub async fn list_events(&self) -> anyhow::Result<Vec<OracleAnnouncement>> {
        let oracle_events: Vec<OracleEventData> =
            reqwest::get(format!("{}/list-events", self.host))
                .await?
                .json()
                .await?;
        println!("oracle_events: {:?}", oracle_events);

        Ok(oracle_events
            .iter()
            .map(|event| event.announcement.clone())
            .collect::<Vec<OracleAnnouncement>>())
    }

    pub async fn create_event(
        &self,
        outcomes: Vec<String>,
        maturity: u32,
    ) -> anyhow::Result<OracleAnnouncement> {
        let event_id = Uuid::new_v4().to_string();

        let create_event_request = CreateEnumEvent {
            event_id,
            outcomes,
            event_maturity_epoch: maturity,
        };
        let announcement: OracleAnnouncement = self
            .client
            .post(format!("{}/create-enum", self.host))
            .json(&create_event_request)
            .send()
            .await?
            .json()
            .await?;

        Ok(announcement)
    }
}

impl dlc_manager::Oracle for KormirOracleClient {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        self.pubkey
    }

    fn get_attestation(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        get::<OracleAttestation>(&self.host, "attestation")
            .map_err(|_| dlc_manager::error::Error::OracleError("Could not get attestation".into()))
    }

    fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        get::<OracleAnnouncement>(&self.host, "announcement").map_err(|_| {
            dlc_manager::error::Error::OracleError("Could not get announcement".into())
        })
    }
}

#[async_trait::async_trait]
impl crate::DdkOracle for KormirOracleClient {
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
}
