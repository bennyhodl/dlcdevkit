use crate::error::Error;
use bitcoin::secp256k1::schnorr::Signature;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

pub trait Storage {
    /// Get the next `num` nonce indexes
    async fn get_next_nonce_indexes(&self, num: usize) -> Result<Vec<u32>, Error>;

    /// Save the announcement and return the identifier
    /// for the announcement
    async fn save_announcement(
        &self,
        announcement: OracleAnnouncement,
        indexes: Vec<u32>,
    ) -> Result<String, Error>;

    /// Save signatures and outcomes for a given event
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
    pub indexes: Vec<u32>,
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

#[derive(Debug, Clone)]
pub struct MemoryStorage {
    current_index: Arc<AtomicU32>,
    data: Arc<RwLock<HashMap<String, OracleEventData>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            current_index: Arc::new(AtomicU32::new(0)),
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
    async fn get_next_nonce_indexes(&self, num: usize) -> Result<Vec<u32>, Error> {
        let mut current_index = self.current_index.fetch_add(num as u32, Ordering::Relaxed);
        let mut indexes = Vec::with_capacity(num);
        for _ in 0..num {
            indexes.push(current_index);
            current_index += 1;
        }
        Ok(indexes)
    }

    async fn save_announcement(
        &self,
        announcement: OracleAnnouncement,
        indexes: Vec<u32>,
    ) -> Result<String, Error> {
        let event_id = announcement.oracle_event.event_id.clone();
        let event = OracleEventData {
            event_id: event_id.clone(),
            announcement,
            indexes,
            signatures: Default::default(),
            #[cfg(feature = "nostr")]
            announcement_event_id: None,
            #[cfg(feature = "nostr")]
            attestation_event_id: None,
        };

        let mut data = self.data.try_write().unwrap();
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
