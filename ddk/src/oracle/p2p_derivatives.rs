//! rust-dlc <https://github.com/p2pderivatives/rust-dlc/blob/master/p2pd-oracle-client/src/lib.rs> (2024)

//! # cg-oracle-client
//! Http client wrapper for the Crypto Garage DLC oracle

use crate::{error::OracleError, Oracle};
use chrono::{DateTime, SecondsFormat, Utc};
use ddk_manager::error::Error as DlcManagerError;
use dlc::secp256k1_zkp::{schnorr::Signature, XOnlyPublicKey};
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};

/// Enables interacting with a DLC oracle.
pub struct P2PDOracleClient {
    host: String,
    public_key: XOnlyPublicKey,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicKeyResponse {
    public_key: XOnlyPublicKey,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AttestationResponse {
    event_id: String,
    signatures: Vec<Signature>,
    values: Vec<String>,
}

async fn get<T>(path: &str) -> Result<T, OracleError>
where
    T: serde::de::DeserializeOwned,
{
    reqwest::get(path)
        .await
        .map_err(|x| OracleError::Reqwest(x))?
        .json::<T>()
        .await
        .map_err(|e| OracleError::Reqwest(e))
}

fn pubkey_path(host: &str) -> String {
    format!("{}{}", host, "oracle/publickey")
}

fn announcement_path(host: &str, asset_id: &str, date_time: &DateTime<Utc>) -> String {
    format!(
        "{}asset/{}/announcement/{}",
        host,
        asset_id,
        date_time.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

fn attestation_path(host: &str, asset_id: &str, date_time: &DateTime<Utc>) -> String {
    format!(
        "{}asset/{}/attestation/{}",
        host,
        asset_id,
        date_time.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

impl P2PDOracleClient {
    /// Try to create an instance of an oracle client connecting to the provided
    /// host. Returns an error if the host could not be reached. Panics if the
    /// oracle uses an incompatible format.
    pub async fn new(host: &str) -> Result<P2PDOracleClient, OracleError> {
        if host.is_empty() {
            return Err(OracleError::Init("Invalid host".to_string()));
        }
        let host = if !host.ends_with('/') {
            format!("{}{}", host, "/")
        } else {
            host.to_string()
        };

        let public_key = get::<PublicKeyResponse>(&pubkey_path(&host))
            .await?
            .public_key;

        Ok(P2PDOracleClient { host, public_key })
    }
}

fn parse_event_id(event_id: &str) -> Result<(String, DateTime<Utc>), OracleError> {
    let asset_id = &event_id[..6];
    let timestamp_str = &event_id[6..];
    let timestamp: i64 = timestamp_str
        .parse()
        .map_err(|_| OracleError::Custom("Invalid timestamp format".to_string()))?;
    let naive_date_time = DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| OracleError::Custom(format!("Invalid timestamp {} in event id", timestamp)))?
        .naive_utc();
    let date_time = DateTime::from_naive_utc_and_offset(naive_date_time, Utc);
    Ok((asset_id.to_string(), date_time))
}

#[async_trait::async_trait]
impl ddk_manager::Oracle for P2PDOracleClient {
    fn get_public_key(&self) -> XOnlyPublicKey {
        self.public_key
    }

    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, DlcManagerError> {
        let (asset_id, date_time) =
            parse_event_id(event_id).map_err(|e| DlcManagerError::OracleError(e.to_string()))?;
        let path = announcement_path(&self.host, &asset_id, &date_time);
        let announcement = get(&path)
            .await
            .map_err(|e| DlcManagerError::OracleError(e.to_string()))?;
        Ok(announcement)
    }

    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, ddk_manager::error::Error> {
        let (asset_id, date_time) =
            parse_event_id(event_id).map_err(|e| DlcManagerError::OracleError(e.to_string()))?;
        let path = attestation_path(&self.host, &asset_id, &date_time);
        let AttestationResponse {
            event_id,
            signatures,
            values,
        } = get::<AttestationResponse>(&path)
            .await
            .map_err(|e| DlcManagerError::OracleError(e.to_string()))?;

        Ok(OracleAttestation {
            event_id,
            oracle_public_key: self.public_key,
            signatures,
            outcomes: values,
        })
    }
}

impl Oracle for P2PDOracleClient {
    fn name(&self) -> String {
        "p2pderivatives".into()
    }
}
