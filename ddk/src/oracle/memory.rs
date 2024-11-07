use bitcoin::bip32::Xpriv;
use bitcoin::key::rand::Rng;
use kormir::storage::{MemoryStorage, Storage};
use kormir::Oracle as Kormir;
use dlc_messages::oracle_msgs::{OracleAttestation, OracleAnnouncement};

use crate::Oracle;

pub struct MemoryOracle {
    pub oracle: Kormir<MemoryStorage>,
}

impl MemoryOracle {
    pub fn new() -> Self {
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

impl dlc_manager::Oracle for MemoryOracle {
    fn get_public_key(&self) -> bitcoin::XOnlyPublicKey {
        self.oracle.public_key()
    }

    async fn get_announcement(
        &self,
        event_id: &str,
    ) -> Result<OracleAnnouncement, dlc_manager::error::Error> {
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
    ) -> Result<OracleAttestation, dlc_manager::error::Error> {
        let signatures = self
            .oracle
            .storage
            .get_event(event_id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .signatures;

        Ok(OracleAttestation {
            oracle_public_key: self.oracle.public_key(),
            signatures: signatures.values().cloned().collect::<Vec<_>>(),
            outcomes: signatures.keys().cloned().collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeDelta};
    use dlc_manager::Oracle;

    use super::*;

    #[tokio::test]
    async fn get_and_sign() {
        let oracle = MemoryOracle::new();
        let expiry = TimeDelta::seconds(15);
        let timestamp: u32 = Local::now()
            .checked_add_signed(expiry)
            .unwrap()
            .timestamp()
            .try_into()
            .unwrap();
        let (id, announcement) = oracle
            .oracle
            .create_enum_event(
                "event_id".into(),
                vec!["rust".into(), "go".into()],
                timestamp,
            )
            .await
            .unwrap();

        let ann = oracle.get_announcement(&format!("{id}")).await.unwrap();

        assert_eq!(ann, announcement);

        let sign = oracle
            .oracle
            .sign_enum_event(id, "rust".to_string())
            .await
            .unwrap();

        let att = oracle.get_attestation(&format!("{id}")).await.unwrap();

        assert_eq!(sign, att);
    }
}
