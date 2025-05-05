use anyhow::anyhow;
use bitcoin::key::XOnlyPublicKey;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use hmac::{Hmac, Mac};
use kormir::storage::OracleEventData;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;
use uuid::Uuid;

async fn get<T>(host: &str, path: &str) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{host}/{path}");
    let request = reqwest::get(url).await?.json::<T>().await?;

    Ok(request)
}

#[derive(Debug, Serialize)]
pub struct CreateEnumEvent {
    pub event_id: String,
    pub outcomes: Vec<String>,
    pub event_maturity_epoch: u32,
}

#[derive(Debug, Serialize)]
struct SignEnumEvent {
    pub event_id: String,
    pub outcome: String,
}

#[derive(Debug, Serialize)]
pub struct CreateNumericEvent {
    pub event_id: String,
    pub num_digits: Option<u16>,
    pub is_signed: Option<bool>,
    pub precision: Option<i32>,
    pub unit: String,
    pub event_maturity_epoch: u32,
}

#[derive(Debug, Serialize)]
pub struct SignNumericEvent {
    pub event_id: String,
    pub outcome: i64,
}

/// Kormir oracle client.
///
/// Allows the creation of enum and numeric announcements as well as signing.
#[derive(Debug)]
pub struct KormirOracleClient {
    pubkey: XOnlyPublicKey,
    client: reqwest::Client,
    host: String,
    hmac_secret: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PubkeyResponse {
    pub pubkey: XOnlyPublicKey,
}

impl KormirOracleClient {
    pub async fn new(
        host: &str,
        hmac_secret: Option<Vec<u8>>,
    ) -> anyhow::Result<KormirOracleClient> {
        let pubkey: XOnlyPublicKey = get::<PubkeyResponse>(host, "pubkey").await?.pubkey;
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
            hmac_secret,
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

    /// Creates an enum oracle announcement.
    ///
    /// Maturity should be the UNIX timestamp of contract maturity.
    pub async fn create_enum_event(
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

        let (body, headers) = self.body_and_headers(&create_event_request)?;

        let announcement = self
            .client
            .post(format!("{}/create-enum", self.host))
            .body(body)
            .headers(headers)
            .send()
            .await?
            .json::<OracleAnnouncement>()
            .await?;

        tracing::info!(event_id, "Created Kormir oracle event.");

        Ok(announcement)
    }

    /// Requests for Kormir to sign an announcement with a given outcome.
    pub async fn sign_enum_event(
        &self,
        event_id: String,
        outcome: String,
    ) -> anyhow::Result<OracleAttestation> {
        tracing::info!("Signing event. event_id={} outcome={}", event_id, outcome);

        let event = SignEnumEvent {
            event_id: event_id.clone(),
            outcome: outcome.clone(),
        };

        let (body, headers) = self.body_and_headers(&event)?;

        let attestation = self
            .client
            .post(format!("{}/sign-enum", &self.host))
            .body(body)
            .headers(headers)
            .send()
            .await?
            .json::<OracleAttestation>()
            .await?;

        tracing::info!(event_id, outcome, "Signed Kormir oracle event.");

        Ok(attestation)
    }

    /// Creates a numeric oracle announcement.
    ///
    /// Kormir currently supports only numeric event with base 2.
    ///
    /// Maturity should be the UNIX timestamp of contract maturity.
    pub async fn create_numeric_event(
        &self,
        num_digits: Option<u16>,
        is_signed: Option<bool>,
        precision: Option<i32>,
        unit: String,
        maturity: u32,
    ) -> anyhow::Result<OracleAnnouncement> {
        let event_id = Uuid::new_v4().to_string();

        let create_event_request = CreateNumericEvent {
            event_id: event_id.clone(),
            num_digits,
            is_signed,
            precision,
            unit,
            event_maturity_epoch: maturity,
        };

        let (body, headers) = self.body_and_headers(&create_event_request)?;

        let announcement = self
            .client
            .post(format!("{}/create-numeric", self.host))
            .body(body)
            .headers(headers)
            .send()
            .await?
            .json::<OracleAnnouncement>()
            .await?;

        tracing::info!(event_id, "Created Kormir oracle event.");

        Ok(announcement)
    }

    /// Requests for Kormir to sign an announcement with a given outcome.
    pub async fn sign_numeric_event(
        &self,
        event_id: String,
        outcome: i64,
    ) -> anyhow::Result<OracleAttestation> {
        tracing::info!("Signing event. event_id={} outcome={}", event_id, outcome);

        let event = SignNumericEvent {
            event_id: event_id.clone(),
            outcome,
        };

        let (body, headers) = self.body_and_headers(&event)?;

        let attestation = self
            .client
            .post(format!("{}/sign-numeric", &self.host))
            .body(body)
            .headers(headers)
            .send()
            .await?
            .json::<OracleAttestation>()
            .await?;

        tracing::info!(event_id, outcome, "Signed Kormir oracle event.");

        Ok(attestation)
    }

    fn body_and_headers<T: Serialize + ?Sized>(
        &self,
        json: &T,
    ) -> anyhow::Result<(Vec<u8>, HeaderMap)> {
        let body = serde_json::to_vec(json)?;
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(secret) = &self.hmac_secret {
            let hmac = Self::calculate_hmac(&body, secret)?;
            headers.append("X-Signature", HeaderValue::from_bytes(hmac.as_bytes())?);
        }
        Ok((body, headers))
    }

    fn calculate_hmac(payload: &[u8], secret: &[u8]) -> anyhow::Result<String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)?;
        mac.update(payload);
        let result = mac.finalize().into_bytes();
        Ok(hex::encode(result))
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
        KormirOracleClient::new("https://kormir.dlcdevkit.com", None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn kormir_enum_events() {
        let kormir = create_kormir().await;

        let expiry = TimeDelta::seconds(30);
        let timestamp: u32 = Local::now()
            .checked_add_signed(expiry)
            .unwrap()
            .timestamp()
            .try_into()
            .unwrap();

        let announcement = kormir
            .create_enum_event(vec!["rust".to_string(), "go".to_string()], timestamp)
            .await;

        assert!(announcement.is_ok());

        let sign_enum = kormir
            .sign_enum_event(
                announcement.unwrap().oracle_event.event_id,
                "rust".to_string(),
            )
            .await;

        assert!(sign_enum.is_ok())
    }

    #[tokio::test]
    async fn kormir_numeric_events() {
        let kormir = create_kormir().await;

        let expiry = TimeDelta::seconds(30);
        let timestamp: u32 = Local::now()
            .checked_add_signed(expiry)
            .unwrap()
            .timestamp()
            .try_into()
            .unwrap();

        let announcement = kormir
            .create_numeric_event(Some(14), Some(true), Some(0), "m/s".to_string(), timestamp)
            .await;

        assert!(announcement.is_ok());

        let sign_numeric = kormir
            .sign_numeric_event(announcement.unwrap().oracle_event.event_id, -12345)
            .await;

        assert!(sign_numeric.is_ok());
    }
}
