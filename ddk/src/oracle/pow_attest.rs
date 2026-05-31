//! HTTP client for the pow-attest oracle at <https://attest.powforge.dev>.
//!
//! pow-attest is a PoW-gated Schnorr attestation oracle implementing the
//! dlcspecs OracleAnnouncement (type 55332) and OracleAttestation (type 55400)
//! TLV formats. Bytes are read directly through
//! `ddk_messages::oracle_msgs::OracleAnnouncement::read` / `OracleAttestation::read`
//! with no JSON translation layer.
//!
//! See discussion at <https://github.com/bennyhodl/dlcdevkit/issues/158>.

use std::io::Cursor;
use std::sync::Arc;

use crate::error::OracleError;
use crate::logger::{log_error, log_info, Logger, WriteLog};
use bitcoin::key::XOnlyPublicKey;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::util::ser::Readable;
use serde::Deserialize;

/// Client for the pow-attest oracle.
///
/// The oracle exposes binary TLV endpoints that match the dlcspecs format
/// verbatim, so deserialization uses `lightning::util::ser::Readable` on the
/// response payload after stripping the outer TLV header.
#[derive(Debug)]
pub struct PowAttestOracleClient {
    host: String,
    pubkey: XOnlyPublicKey,
    client: reqwest::Client,
    logger: Arc<Logger>,
}

#[derive(Debug, Deserialize)]
struct InfoResponse {
    oracle_pubkey: XOnlyPublicKey,
}

impl PowAttestOracleClient {
    /// Connects to a pow-attest oracle at `host` (e.g. `https://attest.powforge.dev`).
    ///
    /// Fetches `/api/v1/info` to learn the oracle's x-only public key.
    pub async fn new(host: &str, logger: Arc<Logger>) -> Result<Self, OracleError> {
        if host.is_empty() {
            return Err(OracleError::Init("Invalid host".to_string()));
        }
        let host = host.trim_end_matches('/').to_string();
        let client = reqwest::Client::new();
        let info: InfoResponse = client
            .get(format!("{host}/api/v1/info"))
            .send()
            .await
            .map_err(|e| {
                OracleError::Init(format!("Could not reach pow-attest: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                OracleError::Init(format!("Could not parse /api/v1/info: {e}"))
            })?;
        log_info!(
            logger,
            "Connected to pow-attest oracle. host={} pubkey={}",
            host,
            info.oracle_pubkey
        );
        Ok(Self {
            host,
            pubkey: info.oracle_pubkey,
            client,
            logger,
        })
    }
}

/// Strips the outer TLV header (BigSize type + BigSize length) from the
/// response body and reads the inner payload.
///
/// The pow-attest endpoints return the full TLV wire format including the
/// 3-byte BigSize type (`fdd824` for announcements, `fdd868` for attestations)
/// and the 1-byte BigSize length prefix. `OracleAnnouncement::read` and
/// `OracleAttestation::read` expect only the payload, so the leading 4 bytes
/// are skipped here.
fn read_tlv_payload<T: Readable>(bytes: &[u8]) -> Result<T, lightning::ln::msgs::DecodeError> {
    // Outer type+len wrapper is 4 bytes for the message sizes pow-attest emits
    // (3-byte BigSize type + 1-byte BigSize length <= 252). If the server ever
    // grows a message past 252 bytes of payload, the length prefix becomes
    // multi-byte and this offset will need to follow the BigSize length rules
    // in dlcspecs.
    let payload = if bytes.len() > 4 { &bytes[4..] } else { bytes };
    let mut cur = Cursor::new(payload);
    T::read(&mut cur)
}

#[async_trait::async_trait]
impl ddk_manager::Oracle for PowAttestOracleClient {
    fn get_public_key(&self) -> XOnlyPublicKey {
        self.pubkey
    }

    #[tracing::instrument(skip(self))]
    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, ddk_manager::error::Error> {
        let url = format!(
            "{}/api/v1/bounty/{}/announcement.tlv",
            self.host, event_id
        );
        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                log_error!(
                    self.logger,
                    "Could not fetch pow-attest announcement. error={}",
                    e
                );
                ddk_manager::error::Error::OracleError(format!(
                    "Could not fetch announcement: {e}"
                ))
            })?
            .bytes()
            .await
            .map_err(|e| {
                ddk_manager::error::Error::OracleError(format!(
                    "Could not read announcement body: {e}"
                ))
            })?;
        let announcement = read_tlv_payload::<OracleAnnouncement>(&bytes).map_err(|e| {
            log_error!(
                self.logger,
                "Could not decode pow-attest announcement TLV. error={:?}",
                e
            );
            ddk_manager::error::Error::OracleError(format!("Could not decode announcement: {e:?}"))
        })?;
        log_info!(
            self.logger,
            "pow-attest announcement. event_id={} nonces={}",
            event_id,
            announcement.oracle_event.oracle_nonces.len()
        );
        Ok(announcement)
    }

    #[tracing::instrument(skip(self))]
    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, ddk_manager::error::Error> {
        let url = format!(
            "{}/api/v1/bounty/{}/attestation.tlv",
            self.host, event_id
        );
        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                log_error!(
                    self.logger,
                    "Could not fetch pow-attest attestation. error={}",
                    e
                );
                ddk_manager::error::Error::OracleError(format!(
                    "Could not fetch attestation: {e}"
                ))
            })?
            .bytes()
            .await
            .map_err(|e| {
                ddk_manager::error::Error::OracleError(format!(
                    "Could not read attestation body: {e}"
                ))
            })?;
        let attestation = read_tlv_payload::<OracleAttestation>(&bytes).map_err(|e| {
            log_error!(
                self.logger,
                "Could not decode pow-attest attestation TLV. error={:?}",
                e
            );
            ddk_manager::error::Error::OracleError(format!("Could not decode attestation: {e:?}"))
        })?;
        log_info!(
            self.logger,
            "pow-attest attestation. event_id={} outcomes={:?}",
            event_id,
            attestation.outcomes
        );
        Ok(attestation)
    }
}

impl crate::Oracle for PowAttestOracleClient {
    fn name(&self) -> String {
        "pow-attest".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured OracleAnnouncement TLV from
    /// `https://attest.powforge.dev/api/v1/bounty/<static-id>/announcement.tlv`.
    ///
    /// The 4-byte outer header is `fdd824 c9`:
    ///   - `fdd824` = 3-byte BigSize for type `55332` (OracleAnnouncement)
    ///   - `c9`     = 1-byte BigSize for length `201`
    const STATIC_ANNOUNCEMENT_TLV_HEX: &str = "fdd824c9711cd782ddf632840c17b934e646785eb5418ec1b104436cce98eff8a4ea1557cd5d2e93316d300aa758cefebf02dd23f9a0fdfe08ce807e9b54ac241c80243def6218b2e12d74ffafa1b6e5217cc4592848c321c28109869903ff88989db23bfdd8226500013e0c2dad9737a8fc69f09298317fae26276c6319f65f0c589e57973abf48fbd967352480fdd806150002000852454c4541534544000750454e44494e47002436626137623831302d396461642d313164312d383062342d303063303466643433306338";

    #[test]
    fn roundtrips_static_announcement() {
        let bytes = hex::decode(STATIC_ANNOUNCEMENT_TLV_HEX).expect("hex");
        let ann = read_tlv_payload::<OracleAnnouncement>(&bytes)
            .expect("OracleAnnouncement::read failed on captured pow-attest TLV");
        assert_eq!(ann.oracle_event.oracle_nonces.len(), 1);
        assert!(ann.oracle_event.event_id.contains("6ba7b810"));
    }
}
