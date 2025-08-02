#![allow(async_fn_in_trait)]

pub mod error;
#[cfg(feature = "nostr")]
pub mod nostr_events;
pub mod storage;

use crate::error::Error;
use crate::storage::Storage;
use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::key::XOnlyPublicKey;
use bitcoin::secp256k1::{All, Message, Secp256k1, SecretKey};
use bitcoin::Network;
use secp256k1_zkp::Keypair;
use std::str::FromStr;

pub use bitcoin;
pub use bitcoin::secp256k1::schnorr::Signature;
use dlc_messages::oracle_msgs::DigitDecompositionEventDescriptor;
pub use dlc_messages::oracle_msgs::{
    EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
};
pub use lightning;
pub use lightning::util::ser::{Readable, Writeable};
#[cfg(feature = "nostr")]
pub use nostr;

// first key for taproot address
const SIGNING_KEY_PATH: &str = "m/86'/0'/0'/0/0";

#[derive(Debug, Clone)]
pub struct Oracle<S: Storage> {
    pub storage: S,
    key_pair: Keypair,
    nonce_xpriv: Xpriv,
    secp: Secp256k1<All>,
}

impl<S: Storage> Oracle<S> {
    pub fn new(storage: S, signing_key: SecretKey, nonce_xpriv: Xpriv) -> Self {
        let secp = Secp256k1::new();
        Self {
            storage,
            key_pair: Keypair::from_secret_key(&secp, &signing_key),
            nonce_xpriv,
            secp,
        }
    }

    pub fn from_xpriv(storage: S, xpriv: Xpriv) -> Result<Self, Error> {
        let secp = Secp256k1::new();

        let signing_key = derive_signing_key(&secp, xpriv)?;
        Self::from_signing_key(storage, signing_key)
    }

    pub fn from_signing_key(storage: S, signing_key: SecretKey) -> Result<Self, Error> {
        let secp = Secp256k1::new();

        let xpriv_bytes = sha256::Hash::hash(&signing_key.secret_bytes()).to_byte_array();
        let nonce_xpriv =
            Xpriv::new_master(Network::Bitcoin, &xpriv_bytes).map_err(|_| Error::Internal)?;

        Ok(Self {
            storage,
            key_pair: Keypair::from_secret_key(&secp, &signing_key),
            nonce_xpriv,
            secp,
        })
    }

    pub fn public_key(&self) -> XOnlyPublicKey {
        self.key_pair.x_only_public_key().0
    }

    /// Returns the keys for the oracle, used for Nostr.
    #[cfg(feature = "nostr")]
    pub fn nostr_keys(&self) -> nostr::Keys {
        let sec = nostr::key::SecretKey::from_slice(&self.key_pair.secret_key().secret_bytes()[..])
            .expect("just converting types");
        nostr::Keys::new(sec)
    }

    fn get_nonce_key(&self, index: u32) -> SecretKey {
        self.nonce_xpriv
            .derive_priv(
                &self.secp,
                &[ChildNumber::from_hardened_idx(index).unwrap()],
            )
            .unwrap()
            .private_key
    }

    pub async fn create_enum_event(
        &self,
        event_id: String,
        outcomes: Vec<String>,
        event_maturity_epoch: u32,
    ) -> Result<OracleAnnouncement, Error> {
        let indexes = self.storage.get_next_nonce_indexes(1).await?;
        let oracle_nonces = indexes
            .iter()
            .map(|i| {
                let nonce_key = self.get_nonce_key(*i);
                nonce_key.x_only_public_key(&self.secp).0
            })
            .collect();
        let event_descriptor = EventDescriptor::EnumEvent(EnumEventDescriptor { outcomes });
        let oracle_event = OracleEvent {
            oracle_nonces,
            event_id,
            event_maturity_epoch,
            event_descriptor,
        };
        oracle_event.validate().map_err(|_| Error::Internal)?;

        // create signature
        let mut data = Vec::new();
        oracle_event.write(&mut data).map_err(|_| Error::Internal)?;
        let hash = sha256::Hash::hash(&data);
        let msg = Message::from_digest(hash.to_byte_array());
        let announcement_signature = self.secp.sign_schnorr_no_aux_rand(&msg, &self.key_pair);

        let ann = OracleAnnouncement {
            oracle_event,
            oracle_public_key: self.public_key(),
            announcement_signature,
        };
        ann.validate(&self.secp).map_err(|_| Error::Internal)?;

        let _ = self.storage.save_announcement(ann.clone(), indexes).await?;

        Ok(ann)
    }

    pub async fn sign_enum_event(
        &self,
        event_id: String,
        outcome: String,
    ) -> Result<OracleAttestation, Error> {
        let Some(data) = self.storage.get_event(event_id.clone()).await? else {
            return Err(Error::NotFound);
        };
        if !data.signatures.is_empty() {
            return Err(Error::EventAlreadySigned);
        }
        if data.indexes.len() != 1 {
            return Err(Error::Internal);
        }
        let descriptor = match &data.announcement.oracle_event.event_descriptor {
            EventDescriptor::EnumEvent(desc) => desc,
            _ => return Err(Error::Internal),
        };
        if !descriptor.outcomes.contains(&outcome) {
            return Err(Error::InvalidOutcome);
        }

        let nonce_index = data.indexes.first().expect("Already checked length");
        let nonce_key = self.get_nonce_key(*nonce_index);

        let hash = sha256::Hash::hash(outcome.as_bytes());
        let msg = Message::from_digest(hash.to_byte_array());

        let sig = dlc::secp_utils::schnorrsig_sign_with_nonce(
            &self.secp,
            &msg,
            &self.key_pair,
            &nonce_key.secret_bytes(),
        );

        // verify our nonce is the same as the one in the announcement
        debug_assert!(
            sig.encode()[..32] == data.announcement.oracle_event.oracle_nonces[0].serialize()
        );

        // verify our signature
        if self
            .secp
            .verify_schnorr(&sig, &msg, &self.key_pair.x_only_public_key().0)
            .is_err()
        {
            return Err(Error::Internal);
        };

        let sigs = vec![(outcome.clone(), sig)];

        self.storage
            .save_signatures(event_id.to_string(), sigs)
            .await?;

        let attestation = OracleAttestation {
            event_id: data.announcement.oracle_event.event_id,
            oracle_public_key: self.public_key(),
            signatures: vec![sig],
            outcomes: vec![outcome],
        };

        Ok(attestation)
    }

    pub async fn create_numeric_event(
        &self,
        event_id: String,
        num_digits: u16,
        is_signed: bool,
        precision: i32,
        unit: String,
        event_maturity_epoch: u32,
    ) -> Result<OracleAnnouncement, Error> {
        if num_digits == 0 {
            return Err(Error::InvalidArgument);
        }

        let num_nonces = if is_signed {
            num_digits as usize + 1
        } else {
            num_digits as usize
        };

        let indexes = self.storage.get_next_nonce_indexes(num_nonces).await?;
        let oracle_nonces = indexes
            .iter()
            .map(|i| {
                let nonce_key = self.get_nonce_key(*i);
                nonce_key.x_only_public_key(&self.secp).0
            })
            .collect();
        let event_descriptor =
            EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
                base: 2,
                is_signed,
                unit,
                precision,
                nb_digits: num_digits,
            });
        let oracle_event = OracleEvent {
            oracle_nonces,
            event_id,
            event_maturity_epoch,
            event_descriptor,
        };
        oracle_event.validate().map_err(|_| Error::Internal)?;

        // create signature
        let mut data = Vec::new();
        oracle_event.write(&mut data).map_err(|_| Error::Internal)?;
        let hash = sha256::Hash::hash(&data);
        let msg = Message::from_digest(hash.to_byte_array());
        let announcement_signature = self.secp.sign_schnorr_no_aux_rand(&msg, &self.key_pair);

        let ann = OracleAnnouncement {
            oracle_event,
            oracle_public_key: self.public_key(),
            announcement_signature,
        };
        ann.validate(&self.secp).map_err(|_| Error::Internal)?;

        let _ = self.storage.save_announcement(ann.clone(), indexes).await?;

        Ok(ann)
    }

    pub async fn sign_numeric_event(
        &self,
        event_id: String,
        outcome: i64,
    ) -> Result<OracleAttestation, Error> {
        let Some(data) = self.storage.get_event(event_id.clone()).await? else {
            return Err(Error::NotFound);
        };
        if !data.signatures.is_empty() {
            return Err(Error::EventAlreadySigned);
        }
        let descriptor = match &data.announcement.oracle_event.event_descriptor {
            EventDescriptor::DigitDecompositionEvent(desc) => desc,
            _ => return Err(Error::Internal),
        };
        if descriptor.base != 2 {
            return Err(Error::Internal);
        }
        let max_value = (descriptor.base as i64).pow(descriptor.nb_digits as u32) - 1;
        let min_value = if descriptor.is_signed { -max_value } else { 0 };
        if outcome < min_value || outcome > max_value {
            return Err(Error::InvalidOutcome);
        }

        let digits = format!(
            "{:0width$b}",
            outcome.abs(),
            width = descriptor.nb_digits as usize
        )
        .chars()
        .map(|char| char.to_string())
        .collect::<Vec<_>>();

        let outcomes = if descriptor.is_signed {
            let mut sign = vec![if outcome < 0 {
                "-".to_string()
            } else {
                "+".to_string()
            }];
            sign.extend(digits);
            sign
        } else {
            digits
        };

        if data.indexes.len() != outcomes.len() {
            return Err(Error::Internal);
        }

        let nonce_keys = data.indexes.iter().map(|i| self.get_nonce_key(*i));

        let mut sigs: Vec<(String, Signature)> = vec![];

        let signatures = outcomes
            .iter()
            .zip(nonce_keys)
            .enumerate()
            .map(|(idx, (outcome, nonce_key))| {
                let hash = sha256::Hash::hash(outcome.as_bytes());
                let msg = Message::from_digest(hash.to_byte_array());
                let sig = dlc::secp_utils::schnorrsig_sign_with_nonce(
                    &self.secp,
                    &msg,
                    &self.key_pair,
                    &nonce_key.secret_bytes(),
                );
                // verify our nonce is the same as the one in the announcement
                debug_assert!(
                    sig.encode()[..32]
                        == data.announcement.oracle_event.oracle_nonces[idx].serialize()
                );
                // verify our signature
                if self
                    .secp
                    .verify_schnorr(&sig, &msg, &self.key_pair.x_only_public_key().0)
                    .is_err()
                {
                    return Err(Error::Internal);
                };
                sigs.push((outcome.clone(), sig));
                Ok(sig)
            })
            .collect::<Result<Vec<_>, Error>>()?;

        self.storage.save_signatures(event_id, sigs).await?;

        let attestation = OracleAttestation {
            event_id: data.announcement.oracle_event.event_id,
            oracle_public_key: self.public_key(),
            signatures,
            outcomes,
        };

        Ok(attestation)
    }
}

pub fn derive_signing_key(secp: &Secp256k1<All>, xpriv: Xpriv) -> Result<SecretKey, Error> {
    let signing_key = xpriv
        .derive_priv(
            secp,
            &DerivationPath::from_str(SIGNING_KEY_PATH).map_err(|_| Error::Internal)?,
        )
        .map_err(|_| Error::Internal)?
        .private_key;
    Ok(signing_key)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::storage::MemoryStorage;
    use bitcoin::secp256k1::rand::{thread_rng, Rng};

    fn create_oracle() -> Oracle<MemoryStorage> {
        let mut seed: [u8; 64] = [0; 64];
        thread_rng().fill(&mut seed);
        let xpriv = Xpriv::new_master(Network::Regtest, &seed).unwrap();
        Oracle::from_xpriv(MemoryStorage::default(), xpriv).unwrap()
    }

    #[tokio::test]
    async fn test_create_enum_event() {
        let oracle = create_oracle();

        let event_id = "test".to_string();
        let outcomes = vec!["a".to_string(), "b".to_string()];
        let event_maturity_epoch = 100;
        let ann = oracle
            .create_enum_event(event_id.clone(), outcomes.clone(), event_maturity_epoch)
            .await
            .unwrap();

        assert!(ann.validate(&oracle.secp).is_ok());
        assert_eq!(ann.oracle_event.event_id, event_id);
        assert_eq!(ann.oracle_event.event_maturity_epoch, event_maturity_epoch);
        assert_eq!(
            ann.oracle_event.event_descriptor,
            EventDescriptor::EnumEvent(EnumEventDescriptor { outcomes })
        );
    }

    #[tokio::test]
    async fn test_sign_enum_event() {
        let oracle = create_oracle();

        let event_id = "test".to_string();
        let outcomes = vec!["a".to_string(), "b".to_string()];
        let event_maturity_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32
            + 86400;
        let ann = oracle
            .create_enum_event(event_id.clone(), outcomes.clone(), event_maturity_epoch)
            .await
            .unwrap();

        println!("{}", hex::encode(ann.encode()));

        let attestation = oracle
            .sign_enum_event(event_id, "a".to_string())
            .await
            .unwrap();
        assert!(attestation.outcomes.contains(&"a".to_string()));
        assert_eq!(attestation.oracle_public_key, oracle.public_key());
        assert_eq!(attestation.signatures.len(), 1);
        assert_eq!(attestation.outcomes.len(), 1);
        let sig = attestation.signatures.first().unwrap();

        // check first 32 bytes of signature is expected nonce
        let expected_nonce = ann.oracle_event.oracle_nonces.first().unwrap().serialize();
        let bytes = sig.encode();
        let (rx, _sig) = bytes.split_at(32);

        println!("{}", hex::encode(attestation.encode()));

        assert_eq!(rx, expected_nonce)
    }

    #[tokio::test]
    async fn test_create_unsigned_numeric_event() {
        let oracle = create_oracle();

        let event_id = "test_unsigned_numeric".to_string();
        let num_digits = 20;

        let event_maturity_epoch = 100;
        let ann = oracle
            .create_numeric_event(
                event_id.clone(),
                num_digits,
                false,
                0,
                "m/s".into(),
                event_maturity_epoch,
            )
            .await
            .unwrap();

        assert!(ann.validate(&oracle.secp).is_ok());
        assert_eq!(ann.oracle_event.event_id, event_id);
        assert_eq!(ann.oracle_event.event_maturity_epoch, event_maturity_epoch);
        assert_eq!(
            ann.oracle_event.event_descriptor,
            EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: false,
                unit: "m/s".into(),
                precision: 0,
                nb_digits: 20,
            })
        );
    }

    #[tokio::test]
    async fn test_sign_unsigned_numeric_event() {
        let oracle = create_oracle();

        let event_id = "test_unsigned_numeric".to_string();
        let num_digits = 16;

        let event_maturity_epoch = 100;
        let ann = oracle
            .create_numeric_event(
                event_id.clone(),
                num_digits,
                false,
                0,
                "m/s".into(),
                event_maturity_epoch,
            )
            .await
            .unwrap();

        println!("{}", hex::encode(ann.encode()));
        let res = oracle.sign_numeric_event(event_id.clone(), 0x55555).await;
        assert!(res.is_err());
        let attestation = oracle
            .sign_numeric_event(event_id.clone(), 0x5555)
            .await
            .unwrap();
        assert_eq!(
            attestation.outcomes,
            vec!["0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1"]
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(attestation.oracle_public_key, oracle.public_key());
        assert_eq!(attestation.signatures.len(), 16);
        assert_eq!(attestation.outcomes.len(), 16);

        for i in 0..attestation.signatures.len() {
            let sig = attestation.signatures[i];

            // check first 32 bytes of signature is expected nonce
            let expected_nonce = ann.oracle_event.oracle_nonces[i].serialize();
            let bytes = sig.encode();
            let (rx, _sig) = bytes.split_at(32);

            assert_eq!(rx, expected_nonce)
        }

        println!("{}", hex::encode(attestation.encode()));
    }

    #[tokio::test]
    async fn test_create_signed_numeric_event() {
        let oracle = create_oracle();

        let event_id = "test_signed_numeric".to_string();
        let num_digits = 20;

        let event_maturity_epoch = 100;
        let ann = oracle
            .create_numeric_event(
                event_id.clone(),
                num_digits,
                true,
                0,
                "m/s".into(),
                event_maturity_epoch,
            )
            .await
            .unwrap();

        assert!(ann.validate(&oracle.secp).is_ok());
        assert_eq!(ann.oracle_event.event_id, event_id);
        assert_eq!(ann.oracle_event.event_maturity_epoch, event_maturity_epoch);
        assert_eq!(
            ann.oracle_event.event_descriptor,
            EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
                base: 2,
                is_signed: true,
                unit: "m/s".into(),
                precision: 0,
                nb_digits: 20,
            })
        );
    }

    #[tokio::test]
    async fn test_sign_signed_positive_numeric_event() {
        let oracle = create_oracle();

        let event_id = "test_signed_numeric".to_string();
        let num_digits = 16;

        let event_maturity_epoch = 100;
        let ann = oracle
            .create_numeric_event(
                event_id.clone(),
                num_digits,
                true,
                0,
                "m/s".into(),
                event_maturity_epoch,
            )
            .await
            .unwrap();

        println!("{}", hex::encode(ann.encode()));
        let res = oracle.sign_numeric_event(event_id.clone(), 0x55555).await;
        assert!(res.is_err());
        let attestation = oracle.sign_numeric_event(event_id, 0x5555).await.unwrap();
        assert_eq!(
            attestation.outcomes,
            vec![
                "+", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1"
            ]
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
        );
        assert_eq!(attestation.oracle_public_key, oracle.public_key());
        assert_eq!(attestation.signatures.len(), 16 + 1);
        assert_eq!(attestation.outcomes.len(), 16 + 1);

        for i in 0..attestation.signatures.len() {
            let sig = attestation.signatures[i];

            // check first 32 bytes of signature is expected nonce
            let expected_nonce = ann.oracle_event.oracle_nonces[i].serialize();
            let bytes = sig.encode();
            let (rx, _sig) = bytes.split_at(32);

            assert_eq!(rx, expected_nonce)
        }

        println!("{}", hex::encode(attestation.encode()));
    }

    #[tokio::test]
    async fn test_sign_signed_negative_numeric_event() {
        let oracle = create_oracle();

        let event_id = "test_signed_numeric".to_string();
        let num_digits = 16;

        let event_maturity_epoch = 100;
        let ann = oracle
            .create_numeric_event(
                event_id.clone(),
                num_digits,
                true,
                0,
                "m/s".into(),
                event_maturity_epoch,
            )
            .await
            .unwrap();

        println!("{}", hex::encode(ann.encode()));
        let res = oracle.sign_numeric_event(event_id.clone(), -0x55555).await;
        assert!(res.is_err());
        let attestation = oracle.sign_numeric_event(event_id, -0x5555).await.unwrap();
        assert_eq!(
            attestation.outcomes,
            vec![
                "-", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1", "0", "1"
            ]
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
        );
        assert_eq!(attestation.oracle_public_key, oracle.public_key());
        assert_eq!(attestation.signatures.len(), 16 + 1);
        assert_eq!(attestation.outcomes.len(), 16 + 1);

        for i in 0..attestation.signatures.len() {
            let sig = attestation.signatures[i];

            // check first 32 bytes of signature is expected nonce
            let expected_nonce = ann.oracle_event.oracle_nonces[i].serialize();
            let bytes = sig.encode();
            let (rx, _sig) = bytes.split_at(32);

            assert_eq!(rx, expected_nonce)
        }

        println!("{}", hex::encode(attestation.encode()));
    }
}
