use std::sync::Arc;

use crate::logger::{log_error, log_info, WriteLog};
use bitcoin::key::XOnlyPublicKey;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use hmac::{Hmac, Mac};
use kormir::storage::OracleEventData;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::error::OracleError;
use crate::logger::Logger;

async fn get<T>(host: &str, path: &str) -> Result<T, reqwest::Error>
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
    logger: Arc<Logger>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PubkeyResponse {
    pub pubkey: XOnlyPublicKey,
}

impl KormirOracleClient {
    pub async fn new(
        host: &str,
        hmac_secret: Option<Vec<u8>>,
        logger: Arc<Logger>,
    ) -> Result<KormirOracleClient, OracleError> {
        let pubkey: XOnlyPublicKey = get::<PubkeyResponse>(host, "pubkey")
            .await
            .map_err(|_| OracleError::Init("Could not get pubkey from Kormir.".to_string()))?
            .pubkey;
        let client = reqwest::Client::new();
        log_info!(
            logger,
            "Connected to Kormir client. host={} pubkey={}",
            host,
            pubkey.to_string()
        );

        Ok(KormirOracleClient {
            pubkey,
            client,
            host: host.to_string(),
            hmac_secret,
            logger,
        })
    }

    /// List all events stored with the connected Kormir server.
    ///
    /// Kormir events includes announcements info, nonce index, signatures
    /// if announcement has been signed, and nostr information.
    pub async fn list_events(&self) -> Result<Vec<OracleEventData>, OracleError> {
        get(&self.host, "list-events").await.map_err(|e| {
            log_error!(
                self.logger,
                "Error getting all kormir events. error={}",
                e.to_string()
            );
            OracleError::Announcement("Could not list events from Kormir.".to_string())
        })
    }

    /// Creates an enum oracle announcement.
    ///
    /// Maturity should be the UNIX timestamp of contract maturity.
    pub async fn create_enum_event(
        &self,
        outcomes: Vec<String>,
        maturity: u32,
    ) -> Result<OracleAnnouncement, OracleError> {
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

        log_info!(
            self.logger,
            "Created Kormir oracle event. event_id={}",
            event_id
        );

        Ok(announcement)
    }

    /// Requests for Kormir to sign an announcement with a given outcome.
    pub async fn sign_enum_event(
        &self,
        event_id: String,
        outcome: String,
    ) -> Result<OracleAttestation, OracleError> {
        log_info!(
            self.logger,
            "Signing event. event_id={} outcome={}",
            event_id,
            outcome
        );

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

        log_info!(
            self.logger,
            "Signed Kormir oracle event. event_id={} outcome={}",
            event_id,
            outcome
        );

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
    ) -> Result<OracleAnnouncement, OracleError> {
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

        log_info!(
            self.logger,
            "Created Kormir oracle event. event_id={}",
            event_id
        );

        Ok(announcement)
    }

    /// Requests for Kormir to sign an announcement with a given outcome.
    pub async fn sign_numeric_event(
        &self,
        event_id: String,
        outcome: i64,
    ) -> Result<OracleAttestation, OracleError> {
        log_info!(
            self.logger,
            "Signing event. event_id={} outcome={}",
            event_id,
            outcome
        );

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

        log_info!(
            self.logger,
            "Signed Kormir oracle event. event_id={} outcome={}",
            event_id,
            outcome
        );

        Ok(attestation)
    }

    fn body_and_headers<T: Serialize + ?Sized>(
        &self,
        json: &T,
    ) -> Result<(Vec<u8>, HeaderMap), OracleError> {
        let body = serde_json::to_vec(json).map_err(|e| OracleError::Custom(e.to_string()))?;
        let mut headers = HeaderMap::new();
        headers.append(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(secret) = &self.hmac_secret {
            let hmac = Self::calculate_hmac(&body, secret)?;
            headers.append(
                "X-Signature",
                HeaderValue::from_bytes(hmac.as_bytes())
                    .map_err(|e| OracleError::Custom(e.to_string()))?,
            );
        }
        Ok((body, headers))
    }

    fn calculate_hmac(payload: &[u8], secret: &[u8]) -> Result<String, OracleError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|e| OracleError::Custom(e.to_string()))?;
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

    #[tracing::instrument(skip(self))]
    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<ddk_messages::oracle_msgs::OracleAttestation, ddk_manager::error::Error> {
        let attestation = get::<OracleAttestation>(&self.host, &format!("attestation/{event_id}"))
            .await
            .map_err(|e| {
                log_error!(self.logger, "Could not get attestation. error={:?}", e);
                ddk_manager::error::Error::OracleError(format!("Could not get attestation: {e}"))
            })?;
        log_info!(
            self.logger,
            "Kormir attestation. event_id={} attestation={:?}",
            event_id,
            attestation
        );
        Ok(attestation)
    }

    #[tracing::instrument(skip(self))]
    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<ddk_messages::oracle_msgs::OracleAnnouncement, ddk_manager::error::Error> {
        let announcement =
            get::<OracleAnnouncement>(&self.host, &format!("announcement/{event_id}"))
                .await
                .map_err(|e| {
                    log_error!(
                        self.logger,
                        "Could not get announcement. error={}",
                        e.to_string()
                    );
                    ddk_manager::error::Error::OracleError(format!(
                        "Could not get announcement: {e}"
                    ))
                })?;
        log_info!(
            self.logger,
            "Kormir announcement. event_id={} announcement={:?}",
            event_id,
            announcement
        );
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
        KormirOracleClient::new(
            "https://kormir.dlcdevkit.com",
            None,
            Arc::new(Logger::disabled("kormir".to_string())),
        )
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
