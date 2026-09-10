use crate::error::Error;
use bitcoin::secp256k1::schnorr::Signature;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Persistence for an [`Oracle`](crate::Oracle).
///
/// The oracle signs each event with a nonce derived from its signing key and
/// the event id. Signing two different outcomes with one nonce reveals the
/// signing key, so a storage implementation carries two safety obligations:
///
/// - `save_announcement` must refuse an event id that is already stored. An
///   event id is single-use per signing key, forever: a second announcement
///   under the same id would repeat the nonce.
/// - `save_signatures` must refuse to sign an event twice. Re-check the stored
///   signatures inside the same lock or transaction as the write, so that two
///   concurrent sign requests cannot both pass an earlier read.
pub trait Storage {
    /// Nonce indexes are no longer used. Nonces are derived from the event id.
    #[deprecated(
        since = "2.0.0",
        note = "nonces are derived from the event id; the oracle no longer requests indexes"
    )]
    async fn get_next_nonce_indexes(&self, _num: usize) -> Result<Vec<u32>, Error> {
        Ok(Vec::new())
    }

    /// Save the announcement and return the identifier
    /// for the announcement.
    ///
    /// Must return [`Error::EventAlreadyExists`] when an event with the same id
    /// is already stored. Overwriting an announcement would discard its
    /// recorded signatures and repeat its nonce. See the trait documentation.
    async fn save_announcement(&self, announcement: OracleAnnouncement) -> Result<String, Error>;

    /// Save signatures and outcomes for a given event.
    ///
    /// Must return [`Error::EventAlreadySigned`] when signatures are already
    /// stored, checked atomically with the write. See the trait documentation.
    async fn save_signatures(
        &self,
        event_id: String,
        sigs: Vec<(String, Signature)>,
    ) -> Result<OracleEventData, Error>;

    /// Get the announcement data for the given id
    async fn get_event(&self, event_id: String) -> Result<Option<OracleEventData>, Error>;
}

/// Data saved for an oracle announcement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleEventData {
    pub event_id: String,
    pub announcement: OracleAnnouncement,
    pub signatures: Vec<(String, Signature)>,
    #[cfg(feature = "nostr")]
    pub announcement_event_id: Option<String>,
    #[cfg(feature = "nostr")]
    pub attestation_event_id: Option<String>,
}

impl OracleEventData {
    pub fn attestation(&self) -> Option<OracleAttestation> {
        if self.signatures.is_empty() {
            None
        } else {
            Some(OracleAttestation {
                event_id: self.announcement.oracle_event.event_id.clone(),
                oracle_public_key: self.announcement.oracle_public_key,
                signatures: self.signatures.iter().map(|x| x.1).collect(),
                outcomes: self.signatures.iter().map(|x| x.0.clone()).collect(),
            })
        }
    }
}

/// In-memory [`Storage`] for tests and local development.
///
/// Nothing is persisted. A fresh `MemoryStorage` has no record of the event
/// ids an earlier instance announced, so it cannot refuse a repeated id. Use
/// a durable storage for any oracle whose attestations are published.
#[derive(Debug, Clone)]
pub struct MemoryStorage {
    data: Arc<RwLock<HashMap<String, OracleEventData>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn list_events(&self) -> Result<Vec<OracleEventData>, Error> {
        let Ok(guard) = self.data.try_read() else {
            return Err(Error::Internal);
        };

        Ok(guard.values().cloned().collect())
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage for MemoryStorage {
    async fn save_announcement(&self, announcement: OracleAnnouncement) -> Result<String, Error> {
        let event_id = announcement.oracle_event.event_id.clone();
        let event = OracleEventData {
            event_id: event_id.clone(),
            announcement,
            signatures: Default::default(),
            #[cfg(feature = "nostr")]
            announcement_event_id: None,
            #[cfg(feature = "nostr")]
            attestation_event_id: None,
        };

        let mut data = self.data.try_write().unwrap();
        if data.contains_key(&event_id) {
            return Err(Error::EventAlreadyExists);
        }
        data.insert(event_id.clone(), event);

        Ok(event_id)
    }

    async fn save_signatures(
        &self,
        id: String,
        sigs: Vec<(String, Signature)>,
    ) -> Result<OracleEventData, Error> {
        let mut data = self.data.try_write().unwrap();
        let Some(mut event) = data.get(&id).cloned() else {
            return Err(Error::NotFound);
        };

        if !event.signatures.is_empty() {
            return Err(Error::EventAlreadySigned);
        }

        event.signatures = sigs;
        data.insert(id, event.clone());

        Ok(event)
    }

    async fn get_event(&self, event_id: String) -> Result<Option<OracleEventData>, Error> {
        let data = self.data.try_read().unwrap();
        Ok(data.get(&event_id).cloned())
    }
}
