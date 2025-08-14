use bitcoin::bip32::Xpriv;
use bitcoin::key::rand::Rng;
use bitcoin::secp256k1::schnorr::Signature;
use ddk_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use kormir::storage::{MemoryStorage, Storage};
use kormir::Oracle as Kormir;

use crate::Oracle;

#[derive(Debug, Clone)]
pub struct MemoryOracle {
    pub oracle: Kormir<MemoryStorage>,
}

impl Default for MemoryOracle {
    fn default() -> Self {
        let mut seed: [u8; 64] = [0; 64];
        bitcoin::key::rand::thread_rng().fill(&mut seed);
        let xpriv = Xpriv::new_master(bitcoin::Network::Regtest, &seed).unwrap();
        let oracle = Kormir::from_xpriv(MemoryStorage::default(), xpriv).unwrap();
        Self { oracle }
    }
}

impl Oracle for MemoryOracle {
    fn name(&self) -> String {
        "kormir".to_string()
    }
}

#[async_trait::async_trait]
impl ddk_manager::Oracle for MemoryOracle {
    fn get_public_key(&self) -> bitcoin::XOnlyPublicKey {
        self.oracle.public_key()
    }

    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, ddk_manager::error::Error> {
        Ok(self
            .oracle
            .storage
            .get_event(event_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .announcement)
    }

    async fn get_attestation(
        &self,
        event_id: &str,
    ) -> Result<OracleAttestation, ddk_manager::error::Error> {
        let event = self
            .oracle
            .storage
            .get_event(event_id.to_string())
            .await
            .unwrap()
            .unwrap();

        let sigs = event
            .signatures
            .iter()
            .map(|sig| sig.1)
            .collect::<Vec<Signature>>();

        let outcomes = event
            .signatures
            .iter()
            .map(|outcome| outcome.0.clone())
            .collect::<Vec<String>>();

        Ok(OracleAttestation {
            event_id: event.announcement.oracle_event.event_id,
            oracle_public_key: self.oracle.public_key(),
            signatures: sigs,
            outcomes,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta};
    use ddk_manager::Oracle;

    use super::*;

    #[tokio::test]
    async fn get_and_sign() {
        let oracle = MemoryOracle::default();
        let expiry = TimeDelta::seconds(15);
        let timestamp: u32 = Local::now()
            .checked_add_signed(expiry)
            .unwrap()
            .timestamp()
            .try_into()
            .unwrap();
        let announcement = oracle
            .oracle
            .create_enum_event(
                "event_id".into(),
                vec!["rust".into(), "go".into()],
                timestamp,
            )
            .await
            .unwrap();

        let ann = oracle
            .get_announcement(&announcement.oracle_event.event_id)
            .await
            .unwrap();

        assert_eq!(ann, announcement);

        let sign = oracle
            .oracle
            .sign_enum_event(
                announcement.oracle_event.event_id.clone(),
                "rust".to_string(),
            )
            .await
            .unwrap();

        let att = oracle
            .get_attestation(&announcement.oracle_event.event_id)
            .await
            .unwrap();

        assert_eq!(sign, att);
    }
}
