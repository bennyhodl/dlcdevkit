use std::collections::HashMap;
use bitcoin::key::XOnlyPublicKey;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use std::str::FromStr;
use kormir::{storage::OracleEventData, Oracle};
use serde::Serialize;
use uuid::Uuid;

const KORMIR_URL: &str = "http://localhost:8082";

fn get<T>(path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}{}", KORMIR_URL, path);
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
}

impl KormirOracleClient {
    pub fn new() -> anyhow::Result<KormirOracleClient> {
        tracing::info!(host=KORMIR_URL, "Connecting to Kormir oracle client.");
        let request: String = get("/pubkey")?;
        let pubkey = XOnlyPublicKey::from_str(&request)?;

        let client = reqwest::Client::new();
        tracing::info!(pubkey=pubkey.to_string(), "Connected to Kormir client.");

        Ok(KormirOracleClient { pubkey, client })
    }

    pub async fn get_pubkey(&self) -> anyhow::Result<XOnlyPublicKey> {
        let request = reqwest::get(format!("{}/pubkey", KORMIR_URL)).await?.json::<String>().await?;
        Ok(XOnlyPublicKey::from_str(&request)?)
    }

    // pub async fn list_events(&self) -> anyhow::Result<Vec<OracleAnnouncement>> {
    //     let oracle_events: Vec<OracleEventData> = reqwest::get(format!("{}/list-events", KORMIR_URL)).await?.json().await?;
    //     println!("oracle_events: {:?}", oracle_events);
    //
    //     Ok(oracle_events.iter().map(|event| event.announcement.clone()).collect::<Vec<OracleAnnouncement>>())
    // }

    pub async fn create_event(&self, outcomes: Vec<String>, maturity: u32) -> anyhow::Result<()> {
        let event_id = Uuid::new_v4().to_string();

        let create_event_request = CreateEnumEvent {
            event_id,
            outcomes,
            event_maturity_epoch: maturity,
        };
        self.client.post(format!("{}/create-event", KORMIR_URL))
            .json(&create_event_request)
            .send().await?;

        Ok(())
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
        get::<OracleAttestation>("attestation").map_err(|_| dlc_manager::error::Error::OracleError("Could not get attestation".into()))
    }

    fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        get::<OracleAnnouncement>("announcement").map_err(|_| {
            dlc_manager::error::Error::OracleError("Could not get announcement".into())
        })
    }
}

impl crate::DdkOracle for KormirOracleClient {
    fn name(&self) -> String {
        "kormir".into()
    }
}
