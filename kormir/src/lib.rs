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
use bitcoin::secp256k1::{All, Secp256k1, SecretKey};
use bitcoin::Network;
use secp256k1_zkp::Keypair;
use std::cmp::{max, min};
use std::str::FromStr;

pub use bitcoin;
pub use bitcoin::secp256k1::schnorr::Signature;
pub use ddk_messages::oracle_msgs::{
    DigitDecompositionEventDescriptor, EnumEventDescriptor, EventDescriptor, OracleAnnouncement,
    OracleAttestation, OracleEvent,
};
pub use lightning;
pub use lightning::util::ser::{Readable, Writeable};
#[cfg(feature = "nostr")]
pub use nostr;

// first key for taproot address
/// Derivation path used to derive the Taproot signing key from an `Xpriv`.
///
/// Follows BIP-86 single-sig Taproot path: `m/86'/0'/0'/0/0`.
const SIGNING_KEY_PATH: &str = "m/86'/0'/0'/0/0";

/// Creates an enum event announcement for oracle events with discrete outcomes.
///
/// This function creates an `OracleAnnouncement` for events where the outcome is one of
/// a predefined set of discrete options (e.g., "heads" or "tails" for a coin flip).
///
/// # Arguments
/// * `secp` - Secp256k1 context for cryptographic operations
/// * `key_pair` - Oracle's key pair for signing the announcement
/// * `event_id` - Unique identifier for this event
/// * `outcomes` - List of possible outcomes for this event
/// * `event_maturity_epoch` - Unix timestamp when the event matures
/// * `nonce` - Public key for the nonce used in this event
///
/// # Returns
/// * `Ok(OracleAnnouncement)` - The signed announcement if successful
/// * `Err(Error::InvalidEventId)` - If the event_id is empty
/// * `Err(Error::InvalidOutcomes)` - If the outcomes list is empty
/// * `Err(Error::Internal)` - If cryptographic operations fail
///
/// # Example
/// ```rust
/// use kormir::*;
/// use bitcoin::secp256k1::{rand, Secp256k1, SecretKey};
/// use secp256k1_zkp::Keypair;
///
/// let secp = Secp256k1::new();
/// let key_pair = Keypair::new(&secp, &mut rand::thread_rng());
/// let nonce_key = SecretKey::from_keypair(&Keypair::new(&secp, &mut rand::thread_rng()));
/// let nonce = nonce_key.x_only_public_key(&secp).0;
///
/// let announcement = create_enum_event(
///     &secp,
///     &key_pair,
///     "coin_flip",
///     &vec!["heads".to_string(), "tails".to_string()],
///     1640995200, // 2022-01-01 00:00:00 UTC
///     &nonce,
/// ).unwrap();
/// ```
///
/// # DLC Spec
/// * [Simple Enumeration](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#simple-enumeration)
/// * [Oracle Announcement](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_announcement-type)
/// * [Oracle Event](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_event-type)
/// * [Enum Event Descriptor](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#enum_event_descriptor)
pub fn create_enum_event(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    event_id: &str,
    outcomes: &[String],
    event_maturity_epoch: u32,
    nonce: &XOnlyPublicKey,
) -> Result<OracleAnnouncement, Error> {
    if event_id.is_empty() {
        return Err(Error::InvalidEventId);
    }
    if outcomes.is_empty() {
        return Err(Error::InvalidOutcomes);
    }
    let oracle_nonces = vec![*nonce];
    let event_descriptor = EventDescriptor::EnumEvent(EnumEventDescriptor {
        outcomes: outcomes.to_owned(),
    });
    let oracle_event = OracleEvent {
        oracle_nonces,
        event_id: event_id.to_owned(),
        event_maturity_epoch,
        event_descriptor,
    };
    oracle_event.validate().map_err(|_| Error::Internal)?;

    // create signature
    let msg = ddk_messages::oracle_msgs::tagged_announcement_msg(&oracle_event);
    let announcement_signature = secp.sign_schnorr_no_aux_rand(&msg, key_pair);

    let ann = OracleAnnouncement {
        oracle_event,
        oracle_public_key: key_pair.public_key().x_only_public_key().0,
        announcement_signature,
    };
    ann.validate(secp).map_err(|_| Error::Internal)?;
    Ok(ann)
}

/// Signs an enum event with a specific outcome.
///
/// This function creates an `OracleAttestation` by signing the chosen outcome
/// for a previously announced enum event. The signature uses the oracle's private key
/// and the nonce key to ensure cryptographic security.
///
/// # Arguments
/// * `secp` - Secp256k1 context for cryptographic operations
/// * `key_pair` - Oracle's key pair for signing
/// * `announcement` - The original event announcement
/// * `outcome` - The specific outcome to sign (must be one of the announced outcomes)
/// * `nonce_key` - The private key corresponding to the nonce used in the announcement
///
/// # Returns
/// * `Ok(OracleAttestation)` - The signed attestation if successful
/// * `Err(Error::InvalidEventDescriptor)` - If the event descriptor is not an EnumEvent
/// * `Err(Error::InvalidOutcome)` - If the outcome is not in the announced list
/// * `Err(Error::InvalidAnnouncement)` - If our public key doesn't match the announcement's oracle_public_key
/// * `Err(Error::InvalidNonces)` - If the nonce_key don't match the announcement's nonce
/// * `Err(Error::Internal)` - If cryptographic operations fail
///
/// # Example
/// ```rust
/// use kormir::*;
/// use bitcoin::secp256k1::{rand, Secp256k1, SecretKey};
/// use secp256k1_zkp::Keypair;
///
/// let secp = Secp256k1::new();
/// let key_pair = Keypair::new(&secp, &mut rand::thread_rng());
/// let nonce_key = SecretKey::from_keypair(&Keypair::new(&secp, &mut rand::thread_rng()));
/// let nonce = nonce_key.x_only_public_key(&secp).0;
///
/// // First create the announcement
/// let announcement = create_enum_event(
///     &secp,
///     &key_pair,
///     "coin_flip",
///     &vec!["heads".to_string(), "tails".to_string()],
///     1640995200,
///     &nonce,
/// ).unwrap();
///
/// // Then sign the outcome
/// let attestation = sign_enum_event(
///     &secp,
///     &key_pair,
///     &announcement,
///     &"heads".to_string(),
///     &nonce_key,
/// ).unwrap();
/// ```
///
/// # DLC Spec
/// * [Simple Enumeration](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#simple-enumeration)
/// * [Oracle Attestation](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_attestation-type)
/// * [Signing Algorithm](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#signing-algorithm)
pub fn sign_enum_event(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    announcement: &OracleAnnouncement,
    outcome: &str,
    nonce_key: &SecretKey,
) -> Result<OracleAttestation, Error> {
    let descriptor = match &announcement.oracle_event.event_descriptor {
        EventDescriptor::EnumEvent(desc) => desc,
        _ => return Err(Error::InvalidEventDescriptor),
    };
    if !descriptor.outcomes.contains(&outcome.to_owned()) {
        return Err(Error::InvalidOutcome);
    }
    if key_pair.x_only_public_key().0 != announcement.oracle_public_key {
        return Err(Error::InvalidAnnouncement);
    }

    if announcement.oracle_event.oracle_nonces.is_empty()
        || nonce_key.x_only_public_key(secp).0 != announcement.oracle_event.oracle_nonces[0]
    {
        return Err(Error::InvalidNonces);
    }

    let msg = ddk_messages::oracle_msgs::tagged_attestation_msg(outcome);

    let sig = ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
        secp,
        &msg,
        key_pair,
        &nonce_key.secret_bytes(),
    );

    // verify our signature
    if secp
        .verify_schnorr(&sig, &msg, &key_pair.x_only_public_key().0)
        .is_err()
    {
        return Err(Error::Internal);
    };

    let attestation = OracleAttestation {
        event_id: announcement.oracle_event.event_id.clone(),
        oracle_public_key: key_pair.public_key().x_only_public_key().0,
        signatures: vec![sig],
        outcomes: vec![outcome.to_owned()],
    };

    Ok(attestation)
}

/// Creates a numeric event announcement for oracle events with numeric outcomes.
///
/// This function creates an `OracleAnnouncement` for events where the outcome is a numeric
/// value that can be decomposed into digits. The value is represented in a specified base
/// (currently only base 2 is supported) and can be signed or unsigned.
///
/// # Arguments
/// * `secp` - Secp256k1 context for cryptographic operations
/// * `key_pair` - Oracle's key pair for signing the announcement
/// * `event_id` - Unique identifier for this event
/// * `base` - Numeric base for digit decomposition (must be 2)
/// * `num_digits` - Number of digits in the numeric representation
/// * `is_signed` - Whether the numeric value can be negative
/// * `precision` - Decimal precision for the numeric value
/// * `unit` - Unit of measurement for the numeric value
/// * `event_maturity_epoch` - Unix timestamp when the event matures
/// * `nonces` - Vector of public keys for nonces (length must match required nonces)
///
/// # Returns
/// * `Ok(OracleAnnouncement)` - The signed announcement if successful
/// * `Err(Error::InvalidEventId)` - If the event_id is empty
/// * `Err(Error::InvalidBase)` - If base is not 2
/// * `Err(Error::InvalidNumberOfDigits)` - If num_digits is 0 or more than 63
/// * `Err(Error::InvalidNonces)` - If the number of nonces doesn't match the required count
/// * `Err(Error::Internal)` - If cryptographic operations fail
///
/// # Example
/// ```rust
/// use kormir::*;
/// use bitcoin::secp256k1::{rand, Secp256k1, SecretKey};
/// use secp256k1_zkp::Keypair;
///
/// let secp = Secp256k1::new();
/// let key_pair = Keypair::new(&secp, &mut rand::thread_rng());
/// let nonce_keys: Vec<SecretKey> = (0..6)
///     .map(|_| SecretKey::from_keypair(&Keypair::new(&secp, &mut rand::thread_rng())))
///     .collect();
/// let nonces = nonce_keys.iter().map(|k| k.x_only_public_key(&secp).0).collect::<Vec<bitcoin::XOnlyPublicKey>>();
///
/// let announcement = create_numeric_event(
///     &secp,
///     &key_pair,
///     &"temperature".to_string(),
///     2, // base 2
///     5, // 5 digits
///     true, // signed
///     1, // 1 decimal place
///     &"°C".to_string(),
///     1640995200, // 2022-01-01 00:00:00 UTC
///     &nonces,
/// ).unwrap();
/// ```
///
/// # DLC Spec
/// * [Digit Decomposition](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#digit-decomposition)
/// * [Oracle Announcement](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_announcement-type)
/// * [Oracle Event](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_event-type)
/// * [Digit Decomposition Event Descriptor](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#digit_decomposition_event_descriptor)
#[allow(clippy::too_many_arguments)]
pub fn create_numeric_event(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    event_id: &str,
    base: u16,
    num_digits: u16,
    is_signed: bool,
    precision: i32,
    unit: &str,
    event_maturity_epoch: u32,
    nonces: &[XOnlyPublicKey],
) -> Result<OracleAnnouncement, Error> {
    if event_id.is_empty() {
        return Err(Error::InvalidEventId);
    }
    if base != 2 {
        return Err(Error::InvalidBase);
    }
    if num_digits == 0 || num_digits > 63 {
        return Err(Error::InvalidNumberOfDigits);
    }

    let num_nonces = if is_signed {
        num_digits as usize + 1
    } else {
        num_digits as usize
    };

    if nonces.len() != num_nonces {
        return Err(Error::InvalidNonces);
    }

    let event_descriptor =
        EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
            base,
            is_signed,
            unit: unit.to_owned(),
            precision,
            nb_digits: num_digits,
        });
    let oracle_event = OracleEvent {
        oracle_nonces: nonces.to_owned(),
        event_id: event_id.to_owned(),
        event_maturity_epoch,
        event_descriptor,
    };
    oracle_event.validate().map_err(|_| Error::Internal)?;

    // create signature
    let msg = ddk_messages::oracle_msgs::tagged_announcement_msg(&oracle_event);
    let announcement_signature = secp.sign_schnorr_no_aux_rand(&msg, key_pair);

    let ann = OracleAnnouncement {
        oracle_event,
        oracle_public_key: key_pair.x_only_public_key().0,
        announcement_signature,
    };
    ann.validate(secp).map_err(|_| Error::Internal)?;

    Ok(ann)
}

/// Signs a numeric event with a specific numeric outcome.
///
/// This function creates an `OracleAttestation` by signing a numeric outcome
/// for a previously announced numeric event. The numeric value is decomposed into
/// individual digits, each signed with its corresponding nonce key.
///
/// The function includes special clamping logic as described in the DLC spec:
/// - For unsigned events: negative values are clamped to 0, values exceeding the maximum are clamped to the maximum
/// - For signed events: values are clamped to the valid range [-max_value, +max_value]
///
/// # Arguments
/// * `secp` - Secp256k1 context for cryptographic operations
/// * `key_pair` - Oracle's key pair for signing
/// * `announcement` - The original event announcement
/// * `outcome` - The numeric outcome to sign (will be clamped if out of range)
/// * `nonce_keys` - Vector of private keys corresponding to the nonces used in the announcement
///
/// # Returns
/// * `Ok(OracleAttestation)` - The signed attestation if successful
/// * `Err(Error::InvalidEventDescriptor)` - If the event descriptor is not a DigitDecompositionEvent
/// * `Err(Error::InvalidBase)` - If base is not 2
/// * `Err(Error::InvalidNumberOfDigits)` - If nb_digits is 0 or more than 63
/// * `Err(Error::InvalidAnnouncement)` - If our public key doesn't match the announcement's oracle_public_key
/// * `Err(Error::InvalidOutcome)` - If the outcome is out of range and clamp_outcome is false
/// * `Err(Error::InvalidNonces)` - If the number of nonce_keys doesn't match the number of digits, nonce_keys don't match the announcement's nonces
/// * `Err(Error::Internal)` - If cryptographic operations fail
///
/// # Example
/// ```rust
/// use kormir::*;
/// use bitcoin::secp256k1::{rand, Secp256k1, SecretKey};
/// use secp256k1_zkp::Keypair;
///
/// let secp = Secp256k1::new();
/// let key_pair = Keypair::new(&secp, &mut rand::thread_rng());
/// let nonce_keys: Vec<SecretKey> = (0..5)
///     .map(|_| SecretKey::from_keypair(&Keypair::new(&secp, &mut rand::thread_rng())))
///     .collect();
/// let nonces = nonce_keys.iter().map(|k| k.x_only_public_key(&secp).0).collect::<Vec<bitcoin::XOnlyPublicKey>>();
///
/// // First create the announcement
/// let announcement = create_numeric_event(
///     &secp,
///     &key_pair,
///     &"temperature".to_string(),
///     2, // base 2
///     4, // 4 digits
///     true, // signed
///     0, // 0 decimal places
///     &"°C".to_string(),
///     1640995200,
///     &nonces,
/// ).unwrap();
///
/// // Then sign the outcome (will be clamped to valid range)
/// let attestation = sign_numeric_event(
///     &secp,
///     &key_pair,
///     &announcement,
///     15, // This will be decomposed into binary digits
///     &nonce_keys,
/// ).unwrap();
/// ```
///
/// # DLC Spec
/// * [Digit Decomposition](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#digit-decomposition)
/// * [Oracle Attestation](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Messaging.md#the-oracle_attestation-type)
/// * [Signing Algorithm](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#signing-algorithm)
pub fn sign_numeric_event(
    secp: &Secp256k1<All>,
    key_pair: &Keypair,
    announcement: &OracleAnnouncement,
    outcome: i64,
    nonce_keys: &[SecretKey],
) -> Result<OracleAttestation, Error> {
    let descriptor = match &announcement.oracle_event.event_descriptor {
        EventDescriptor::DigitDecompositionEvent(desc) => desc,
        _ => return Err(Error::InvalidEventDescriptor),
    };
    if descriptor.base != 2 {
        return Err(Error::InvalidBase);
    }
    if key_pair.x_only_public_key().0 != announcement.oracle_public_key {
        return Err(Error::InvalidAnnouncement);
    }
    nonce_keys
        .iter()
        .zip(&announcement.oracle_event.oracle_nonces)
        .try_for_each(|(nonce_key, nonce)| {
            if nonce_key.x_only_public_key(secp).0 != *nonce {
                Err(Error::InvalidNonces)
            } else {
                Ok(())
            }
        })?;
    let max_value = get_max_value(descriptor)?;
    let min_value = get_min_value(descriptor)?;
    let outcome_to_sign = if outcome < min_value || outcome > max_value {
        max(min(outcome, max_value), min_value)
    } else {
        outcome
    };
    let digits = format!(
        "{:0width$b}",
        outcome_to_sign.abs(),
        width = descriptor.nb_digits as usize
    )
    .chars()
    .map(|char| char.to_string())
    .collect::<Vec<_>>();

    let outcomes = if descriptor.is_signed {
        let mut sign = vec![if outcome_to_sign < 0 {
            "-".to_string()
        } else {
            "+".to_string()
        }];
        sign.extend(digits);
        sign
    } else {
        digits
    };

    if nonce_keys.len() != outcomes.len() {
        return Err(Error::InvalidNonces);
    }

    let signatures = outcomes
        .iter()
        .zip(nonce_keys)
        .map(|(outcome, nonce_key)| {
            let msg = ddk_messages::oracle_msgs::tagged_attestation_msg(outcome);
            let sig = ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
                secp,
                &msg,
                key_pair,
                &nonce_key.secret_bytes(),
            );
            // verify our signature
            if secp
                .verify_schnorr(&sig, &msg, &key_pair.x_only_public_key().0)
                .is_err()
            {
                return Err(Error::Internal);
            };
            Ok(sig)
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let attestation = OracleAttestation {
        event_id: announcement.oracle_event.event_id.clone(),
        oracle_public_key: key_pair.x_only_public_key().0,
        signatures,
        outcomes,
    };

    Ok(attestation)
}

/// Returns the minimum representable outcome for the provided digit decomposition descriptor.
///
/// For unsigned descriptors, the minimum is 0; for signed descriptors, the minimum
/// is the negation of the maximum representable magnitude.
pub fn get_min_value(descriptor: &DigitDecompositionEventDescriptor) -> Result<i64, Error> {
    if descriptor.is_signed {
        get_max_value(descriptor).map(|x| -x)
    } else {
        Ok(0)
    }
}

/// Returns the maximum representable (absolute) magnitude for the descriptor.
///
/// Computed as `(base^nb_digits) - 1`. For unsigned descriptors this is the maximum
/// value; for signed descriptors, this is the maximum magnitude.
pub fn get_max_value(descriptor: &DigitDecompositionEventDescriptor) -> Result<i64, Error> {
    if descriptor.nb_digits == 0 || descriptor.nb_digits > 63 {
        Err(Error::InvalidNumberOfDigits)
    } else {
        Ok((descriptor.base as i64).pow(descriptor.nb_digits as u32) - 1)
    }
}

/// Oracle encapsulates the oracle's signing key, nonce derivation and persistence layer
/// to create announcements and produce attestations for enum and numeric events.
#[derive(Debug, Clone)]
pub struct Oracle<S: Storage> {
    pub storage: S,
    key_pair: Keypair,
    nonce_xpriv: Xpriv,
    secp: Secp256k1<All>,
}

impl<S: Storage> Oracle<S> {
    /// Creates a new `Oracle` from a signing key and nonce master `Xpriv`.
    ///
    /// The `nonce_xpriv` is used to derive hardened per-event nonce keys.
    pub fn new(storage: S, signing_key: SecretKey, nonce_xpriv: Xpriv) -> Self {
        let secp = Secp256k1::new();
        Self {
            storage,
            key_pair: Keypair::from_secret_key(&secp, &signing_key),
            nonce_xpriv,
            secp,
        }
    }

    /// Constructs an `Oracle` from a master `Xpriv` by deriving the Taproot signing key
    /// at `SIGNING_KEY_PATH`, and creating a deterministic `nonce_xpriv` used for nonces.
    pub fn from_xpriv(storage: S, xpriv: Xpriv) -> Result<Self, Error> {
        let secp = Secp256k1::new();

        let signing_key = derive_signing_key(&secp, xpriv)?;
        Self::from_signing_key(storage, signing_key)
    }

    /// Constructs an `Oracle` from a provided signing key. The `nonce_xpriv` is
    /// deterministically derived from the SHA256 of the signing key bytes.
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

    /// Returns the oracle's x-only public key, used in announcements and attestations.
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

    /// Derives the hardened nonce private key at `index` from the oracle's `nonce_xpriv`.
    fn get_nonce_key(&self, index: u32) -> SecretKey {
        self.nonce_xpriv
            .derive_priv(
                &self.secp,
                &[ChildNumber::from_hardened_idx(index).unwrap()],
            )
            .unwrap()
            .private_key
    }

    /// Creates an enum event announcement with a fresh nonce and persists it to `storage`.
    pub async fn create_enum_event(
        &self,
        event_id: String,
        outcomes: Vec<String>,
        event_maturity_epoch: u32,
    ) -> Result<OracleAnnouncement, Error> {
        let nonce_indexes = self.storage.get_next_nonce_indexes(1).await?;
        if nonce_indexes.len() != 1 {
            return Err(Error::Internal);
        }
        let nonce_key = self.get_nonce_key(nonce_indexes[0]);
        let nonce = nonce_key.x_only_public_key(&self.secp).0;
        let ann = create_enum_event(
            &self.secp,
            &self.key_pair,
            &event_id,
            &outcomes,
            event_maturity_epoch,
            &nonce,
        )?;
        let _ = self
            .storage
            .save_announcement(ann.clone(), nonce_indexes)
            .await?;
        Ok(ann)
    }

    /// Signs an enum event outcome for an existing stored event and persists the signature.
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

        let nonce_index = data.indexes[0];
        let nonce_key = self.get_nonce_key(nonce_index);

        let attestation = sign_enum_event(
            &self.secp,
            &self.key_pair,
            &data.announcement,
            &outcome,
            &nonce_key,
        )?;

        let sigs = vec![(outcome.clone(), attestation.signatures.clone()[0])];

        self.storage
            .save_signatures(event_id.to_string(), sigs)
            .await?;

        Ok(attestation)
    }

    /// Creates a numeric event announcement with fresh nonces and persists it to `storage`.
    pub async fn create_numeric_event(
        &self,
        event_id: String,
        num_digits: u16,
        is_signed: bool,
        precision: i32,
        unit: String,
        event_maturity_epoch: u32,
    ) -> Result<OracleAnnouncement, Error> {
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
            .collect::<Vec<XOnlyPublicKey>>();

        let ann = create_numeric_event(
            &self.secp,
            &self.key_pair,
            &event_id,
            2,
            num_digits,
            is_signed,
            precision,
            &unit,
            event_maturity_epoch,
            &oracle_nonces,
        )?;

        let _ = self.storage.save_announcement(ann.clone(), indexes).await?;

        Ok(ann)
    }

    /// Signs a numeric event outcome (with clamping) and persists the signatures to `storage`.
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

        let nonce_keys = data
            .indexes
            .iter()
            .map(|i| self.get_nonce_key(*i))
            .collect::<Vec<SecretKey>>();

        let attestation = sign_numeric_event(
            &self.secp,
            &self.key_pair,
            &data.announcement,
            outcome,
            &nonce_keys,
        )?;

        let sigs = attestation
            .outcomes
            .iter()
            .cloned()
            .zip(attestation.signatures.clone())
            .collect();

        self.storage.save_signatures(event_id, sigs).await?;

        Ok(attestation)
    }
}

/// Derives the Taproot signing `SecretKey` from a master `Xpriv` using `SIGNING_KEY_PATH`.
///
/// # Arguments
/// * `secp` - Secp256k1 context used for derivation
/// * `xpriv` - Master extended private key
///
/// # Returns
/// * `Ok(SecretKey)` - The derived private key
/// * `Err(Error::Internal)` - If the derivation path or key derivation fails
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

    fn setup_test_oracle() -> Oracle<MemoryStorage> {
        let mut seed: [u8; 64] = [0; 64];
        thread_rng().fill(&mut seed);
        let xpriv = Xpriv::new_master(Network::Regtest, &seed).unwrap();
        Oracle::from_xpriv(MemoryStorage::default(), xpriv).unwrap()
    }

    fn setup_test_keypair() -> (Secp256k1<All>, Keypair) {
        let mut seed: [u8; 64] = [0; 64];
        thread_rng().fill(&mut seed);
        let xpriv = Xpriv::new_master(Network::Regtest, &seed).unwrap();
        let secp = Secp256k1::new();
        let signing_key = derive_signing_key(&secp, xpriv).unwrap();
        let key_pair = Keypair::from_secret_key(&secp, &signing_key);
        (secp, key_pair)
    }

    fn setup_test_nonce(secp: &Secp256k1<All>) -> (SecretKey, XOnlyPublicKey) {
        let mut bytes: [u8; 32] = [0; 32];
        thread_rng().fill(&mut bytes);
        let nonce_key = SecretKey::from_slice(&bytes).unwrap();
        (nonce_key, nonce_key.x_only_public_key(&secp).0)
    }

    fn setup_test_nonces(secp: &Secp256k1<All>, n: u16) -> (Vec<SecretKey>, Vec<XOnlyPublicKey>) {
        let nonce_keys: Vec<SecretKey> = (0..n)
            .map(|_| {
                let mut bytes: [u8; 32] = [0; 32];
                thread_rng().fill(&mut bytes);
                SecretKey::from_slice(&bytes).unwrap()
            })
            .collect();
        let nonces: Vec<XOnlyPublicKey> = nonce_keys
            .iter()
            .map(|nonce_key| nonce_key.x_only_public_key(&secp).0)
            .collect();
        (nonce_keys, nonces)
    }

    #[test]
    fn test_create_enum_event() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "enum".to_string();
        let outcomes = vec!["x".to_string(), "y".to_string()];
        let event_maturity_epoch = 12345u32;

        let (_, nonce) = setup_test_nonce(&secp);

        let ann = create_enum_event(
            &secp,
            &key_pair,
            &event_id,
            &outcomes,
            event_maturity_epoch,
            &nonce,
        )
        .unwrap();

        assert!(ann.validate(&secp).is_ok());
        assert_eq!(ann.oracle_event.event_id, event_id);
        assert_eq!(ann.oracle_event.event_maturity_epoch, event_maturity_epoch);
        assert_eq!(ann.oracle_event.oracle_nonces, vec![nonce]);
        match ann.oracle_event.event_descriptor {
            EventDescriptor::EnumEvent(d) => {
                assert_eq!(d.outcomes, outcomes);
            }
            EventDescriptor::DigitDecompositionEvent(_) => {
                assert!(false, "invalid event descriptor type")
            }
        }
    }

    #[test]
    fn test_sign_enum_event() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "enum_sign".to_string();
        let outcomes = vec!["a".to_string(), "b".to_string()];
        let event_maturity_epoch = 67890u32;

        let (nonce_key, nonce) = setup_test_nonce(&secp);

        let ann = create_enum_event(
            &secp,
            &key_pair,
            &event_id,
            &outcomes,
            event_maturity_epoch,
            &nonce,
        )
        .unwrap();

        let attestation =
            sign_enum_event(&secp, &key_pair, &ann, &"a".to_string(), &nonce_key).unwrap();

        assert!(attestation.outcomes.contains(&"a".to_string()));
        assert_eq!(
            attestation.oracle_public_key,
            key_pair.x_only_public_key().0
        );
        assert_eq!(attestation.signatures.len(), 1);

        // verify our nonce is the same as the one in the announcement
        assert_eq!(
            attestation.signatures[0].encode()[..32],
            ann.oracle_event.oracle_nonces[0].serialize()
        );
    }

    #[test]
    fn test_create_numeric_event() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "numeric".to_string();
        let num_digits = 8u16;
        let is_signed = false;
        let precision = 0i32;
        let unit = "m/s".to_string();
        let event_maturity_epoch = 1111u32;

        let (_, nonces) = setup_test_nonces(&secp, num_digits);

        let ann = create_numeric_event(
            &secp,
            &key_pair,
            &event_id,
            2,
            num_digits,
            is_signed,
            precision,
            &unit,
            event_maturity_epoch,
            &nonces,
        )
        .unwrap();

        assert!(ann.validate(&secp).is_ok());
        assert_eq!(ann.oracle_event.event_id, event_id);
        assert_eq!(ann.oracle_event.event_maturity_epoch, event_maturity_epoch);
        assert_eq!(ann.oracle_event.oracle_nonces, nonces);
        match ann.oracle_event.event_descriptor {
            EventDescriptor::EnumEvent(_) => {
                assert!(false, "invalid event descriptor type")
            }
            EventDescriptor::DigitDecompositionEvent(d) => {
                assert_eq!(d.base, 2);
                assert_eq!(d.is_signed, is_signed);
                assert_eq!(d.unit, unit);
                assert_eq!(d.precision, precision);
                assert_eq!(d.nb_digits, num_digits);
            }
        }
    }

    #[test]
    fn test_sign_numeric_event() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "numeric_sign".to_string();
        let num_digits = 4u16;
        let is_signed = true;
        let precision = 0i32;
        let unit = "m/s".to_string();
        let event_maturity_epoch = 2222u32;

        let (nonce_keys, nonces) = setup_test_nonces(&secp, num_digits + 1);

        let ann = create_numeric_event(
            &secp,
            &key_pair,
            &event_id,
            2,
            num_digits,
            is_signed,
            precision,
            &unit,
            event_maturity_epoch,
            &nonces,
        )
        .unwrap();

        let attestation = sign_numeric_event(&secp, &key_pair, &ann, -0b1010, &nonce_keys).unwrap();

        assert_eq!(
            attestation.outcomes,
            vec!["-", "1", "0", "1", "0"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(attestation.outcomes.len(), (num_digits as usize) + 1);
        assert_eq!(attestation.signatures.len(), (num_digits as usize) + 1);
        assert_eq!(
            attestation.oracle_public_key,
            key_pair.x_only_public_key().0
        );
        // verify our nonces are the same as the one in the announcement
        attestation
            .signatures
            .iter()
            .zip(ann.oracle_event.oracle_nonces)
            .for_each(|(sig, nonce)| assert_eq!(sig.encode()[..32], nonce.serialize()));
    }

    #[test]
    fn test_sign_numeric_event_clamping_unsigned() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "unsigned".to_string();
        let num_digits = 4u16; // base 2 -> range 0..=15
        let is_signed = false;
        let precision = 0i32;
        let unit = "m/s".to_string();
        let event_maturity_epoch = 3333u32;

        let (nonce_keys, nonces) = setup_test_nonces(&secp, num_digits);

        let ann = create_numeric_event(
            &secp,
            &key_pair,
            &event_id,
            2,
            num_digits,
            is_signed,
            precision,
            &unit,
            event_maturity_epoch,
            &nonces,
        )
        .unwrap();

        let att_big = sign_numeric_event(&secp, &key_pair, &ann, 1_000_000, &nonce_keys).unwrap();
        assert_eq!(
            att_big.outcomes,
            vec!["1", "1", "1", "1"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // verify our nonces are the same as the one in the announcement
        att_big
            .signatures
            .iter()
            .zip(ann.oracle_event.oracle_nonces.clone())
            .for_each(|(sig, nonce)| assert_eq!(sig.encode()[..32], nonce.serialize()));

        let att_neg = sign_numeric_event(&secp, &key_pair, &ann, -42, &nonce_keys).unwrap();
        assert_eq!(
            att_neg.outcomes,
            vec!["0", "0", "0", "0"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // verify our nonces are the same as the one in the announcement
        att_neg
            .signatures
            .iter()
            .zip(ann.oracle_event.oracle_nonces.clone())
            .for_each(|(sig, nonce)| assert_eq!(sig.encode()[..32], nonce.serialize()));
    }

    #[test]
    fn test_sign_numeric_event_clamping_signed() {
        let (secp, key_pair) = setup_test_keypair();

        let event_id = "signed".to_string();
        let num_digits = 3u16; // base 2 -> magnitude range 0..=7; signed adds sign nonce
        let is_signed = true;
        let precision = 0i32;
        let unit = "m/s".to_string();
        let event_maturity_epoch = 4444u32;

        let (nonce_keys, nonces) = setup_test_nonces(&secp, num_digits + 1);

        let ann = create_numeric_event(
            &secp,
            &key_pair,
            &event_id,
            2,
            num_digits,
            is_signed,
            precision,
            &unit,
            event_maturity_epoch,
            &nonces,
        )
        .unwrap();

        let att_big = sign_numeric_event(&secp, &key_pair, &ann, 10_000, &nonce_keys).unwrap();
        assert_eq!(
            att_big.outcomes,
            vec!["+", "1", "1", "1"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // verify our nonces are the same as the one in the announcement
        att_big
            .signatures
            .iter()
            .zip(ann.oracle_event.oracle_nonces.clone())
            .for_each(|(sig, nonce)| assert_eq!(sig.encode()[..32], nonce.serialize()));

        let att_small = sign_numeric_event(&secp, &key_pair, &ann, -10_000, &nonce_keys).unwrap();
        assert_eq!(
            att_small.outcomes,
            vec!["-", "1", "1", "1"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        // verify our nonces are the same as the one in the announcement
        att_small
            .signatures
            .iter()
            .zip(ann.oracle_event.oracle_nonces)
            .for_each(|(sig, nonce)| assert_eq!(sig.encode()[..32], nonce.serialize()));
    }

    #[test]
    fn test_error_invalid_event_id() {
        let (secp, key_pair) = setup_test_keypair();
        let (_, nonce) = setup_test_nonce(&secp);

        // Test empty event_id in create_enum_event
        let result = create_enum_event(
            &secp,
            &key_pair,
            "",
            &vec!["a".to_string(), "b".to_string()],
            12345,
            &nonce,
        );
        assert!(matches!(result, Err(Error::InvalidEventId)));

        // Test empty event_id in create_numeric_event
        let (_, nonces) = setup_test_nonces(&secp, 4);
        let result =
            create_numeric_event(&secp, &key_pair, "", 2, 4, false, 0, "m/s", 12345, &nonces);
        assert!(matches!(result, Err(Error::InvalidEventId)));
    }

    #[test]
    fn test_error_invalid_outcomes() {
        let (secp, key_pair) = setup_test_keypair();
        let (_, nonce) = setup_test_nonce(&secp);

        // Test empty outcomes
        let result = create_enum_event(&secp, &key_pair, "test", &vec![], 12345, &nonce);
        assert!(matches!(result, Err(Error::InvalidOutcomes)));
    }

    #[test]
    fn test_error_invalid_base() {
        let (secp, key_pair) = setup_test_keypair();
        let (nonce_keys, nonces) = setup_test_nonces(&secp, 4);

        // Test base != 2 in create_numeric_event
        let result = create_numeric_event(
            &secp, &key_pair, "test", 3, // Invalid base
            4, false, 0, "m/s", 12345, &nonces,
        );
        assert!(matches!(result, Err(Error::InvalidBase)));

        // Test base != 2 in sign_numeric_event (by creating announcement with wrong descriptor)
        // We can't easily create an announcement with base != 2 since create_numeric_event rejects it,
        // but sign_numeric_event also checks base != 2, so we test that path by manually constructing
        // an announcement with wrong base in the descriptor
        let mut ann = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, false, 0, "m/s", 12345, &nonces,
        )
        .unwrap();

        // Modify the descriptor to have wrong base
        if let EventDescriptor::DigitDecompositionEvent(ref mut desc) =
            ann.oracle_event.event_descriptor
        {
            desc.base = 3; // Invalid base
        }

        let result = sign_numeric_event(&secp, &key_pair, &ann, 5, &nonce_keys);
        assert!(matches!(result, Err(Error::InvalidBase)));
    }

    #[test]
    fn test_error_invalid_number_of_digits() {
        let (secp, key_pair) = setup_test_keypair();

        // Test num_digits == 0
        let nonces: Vec<XOnlyPublicKey> = vec![];
        let result = create_numeric_event(
            &secp, &key_pair, "test", 2, 0, // Invalid: zero digits
            false, 0, "m/s", 12345, &nonces,
        );
        assert!(matches!(result, Err(Error::InvalidNumberOfDigits)));

        // Test num_digits > 63
        let (_, nonces) = setup_test_nonces(&secp, 64);
        let result = create_numeric_event(
            &secp, &key_pair, "test", 2, 64, // Invalid: > 63
            false, 0, "m/s", 12345, &nonces,
        );
        assert!(matches!(result, Err(Error::InvalidNumberOfDigits)));

        // Test invalid number of digits in sign_numeric_event
        let (nonce_keys, nonces) = setup_test_nonces(&secp, 4);
        let mut ann = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, false, 0, "m/s", 12345, &nonces,
        )
        .unwrap();

        // Modify the descriptor to have invalid number of digits
        if let EventDescriptor::DigitDecompositionEvent(ref mut desc) =
            ann.oracle_event.event_descriptor
        {
            desc.nb_digits = 0; // Invalid: zero digits
        }

        let result = sign_numeric_event(&secp, &key_pair, &ann, 5, &nonce_keys);
        assert!(matches!(result, Err(Error::InvalidNumberOfDigits)));

        // Test num_digits > 63 in sign_numeric_event
        if let EventDescriptor::DigitDecompositionEvent(ref mut desc) =
            ann.oracle_event.event_descriptor
        {
            desc.nb_digits = 64; // Invalid: > 63
        }

        let result = sign_numeric_event(&secp, &key_pair, &ann, 5, &nonce_keys);
        assert!(matches!(result, Err(Error::InvalidNumberOfDigits)));
    }

    #[test]
    fn test_error_invalid_nonces() {
        let (secp, key_pair) = setup_test_keypair();

        // Test wrong number of nonces for unsigned event
        let (_, wrong_nonces) = setup_test_nonces(&secp, 2);

        let result = create_numeric_event(
            &secp,
            &key_pair,
            "test",
            2,
            4,
            false,
            0,
            "m/s",
            12345,
            &wrong_nonces,
        );
        assert!(matches!(result, Err(Error::InvalidNonces)));

        // Test wrong number of nonces for signed event
        let (_, wrong_nonces) = setup_test_nonces(&secp, 1); // 1 nonce for 4-digit signed (should be 5: 4 digits + 1 sign)
        let result = create_numeric_event(
            &secp,
            &key_pair,
            "test",
            2,
            4,
            true,
            0,
            "m/s",
            12345,
            &wrong_nonces,
        );
        assert!(matches!(result, Err(Error::InvalidNonces)));

        // Test wrong set of nonces
        let (_, nonce1) = setup_test_nonce(&secp);
        let (nonce_key2, _) = setup_test_nonce(&secp);

        let ann = create_enum_event(
            &secp,
            &key_pair,
            "test",
            &vec!["a".to_string(), "b".to_string()],
            12345,
            &nonce1,
        )
        .unwrap();

        let result = sign_enum_event(&secp, &key_pair, &ann, &"a".to_string(), &nonce_key2);

        assert!(matches!(result, Err(Error::InvalidNonces)));

        let (_, nonces1) = setup_test_nonces(&secp, 4);
        let (nonce_keys2, _) = setup_test_nonces(&secp, 4);

        let ann = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, false, 0, "m/s", 12345, &nonces1,
        )
        .unwrap();

        let result = sign_numeric_event(&secp, &key_pair, &ann, -0b1010, &nonce_keys2);

        assert!(matches!(result, Err(Error::InvalidNonces)));
    }

    #[test]
    fn test_error_invalid_outcome() {
        let (secp, key_pair) = setup_test_keypair();
        let (nonce_key, nonce) = setup_test_nonce(&secp);

        // Create an enum event
        let ann = create_enum_event(
            &secp,
            &key_pair,
            "test",
            &vec!["a".to_string(), "b".to_string()],
            12345,
            &nonce,
        )
        .unwrap();

        // Try to sign with invalid outcome
        let result = sign_enum_event(&secp, &key_pair, &ann, "c", &nonce_key);
        assert!(matches!(result, Err(Error::InvalidOutcome)));

        // Create a numeric event
        let (_, nonces) = setup_test_nonces(&secp, 4);
        let _ = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, // 4 digits = range 0..=15
            false, 0, "m/s", 12345, &nonces,
        )
        .unwrap();
    }

    #[test]
    fn test_error_invalid_event_descriptor() {
        let (secp, key_pair) = setup_test_keypair();

        // Create a numeric event
        let (_, nonces) = setup_test_nonces(&secp, 4);
        let numeric_ann = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, false, 0, "m/s", 12345, &nonces,
        )
        .unwrap();

        // Try to use sign_enum_event on a numeric announcement (wrong event descriptor type)
        let (nonce_key, nonce) = setup_test_nonce(&secp);
        let result = sign_enum_event(&secp, &key_pair, &numeric_ann, "a", &nonce_key);
        assert!(matches!(result, Err(Error::InvalidEventDescriptor)));

        // Create an enum event
        let enum_ann = create_enum_event(
            &secp,
            &key_pair,
            "test",
            &vec!["a".to_string(), "b".to_string()],
            12345,
            &nonce,
        )
        .unwrap();

        // Try to use sign_numeric_event on an enum announcement (wrong event descriptor type)
        let result = sign_numeric_event(&secp, &key_pair, &enum_ann, 42, &vec![nonce_key]);
        assert!(matches!(result, Err(Error::InvalidEventDescriptor)));
    }

    #[test]
    fn test_error_invalid_announcement() {
        let (secp, key_pair) = setup_test_keypair();
        let other_key_pair = Keypair::new(&secp, &mut thread_rng());
        let (nonce_key, nonce) = setup_test_nonce(&secp);

        // Create an enum event with one key_pair
        let ann = create_enum_event(
            &secp,
            &key_pair,
            "test",
            &vec!["a".to_string(), "b".to_string()],
            12345,
            &nonce,
        )
        .unwrap();

        // Try to sign with different key_pair (wrong oracle key)
        let result = sign_enum_event(&secp, &other_key_pair, &ann, "a", &nonce_key);
        assert!(matches!(result, Err(Error::InvalidAnnouncement)));

        // Test wrong oracle key in sign_numeric_event
        let (nonce_keys, nonces) = setup_test_nonces(&secp, 4);
        let numeric_ann = create_numeric_event(
            &secp, &key_pair, "test", 2, 4, false, 0, "m/s", 12345, &nonces,
        )
        .unwrap();

        let result = sign_numeric_event(&secp, &other_key_pair, &numeric_ann, 5, &nonce_keys);
        assert!(matches!(result, Err(Error::InvalidAnnouncement)));
    }

    #[tokio::test]
    async fn test_error_not_found() {
        let oracle = setup_test_oracle();

        // Try to sign an event that doesn't exist
        let result = oracle
            .sign_enum_event("nonexistent".to_string(), "a".to_string())
            .await;
        assert!(matches!(result, Err(Error::NotFound)));

        let result = oracle
            .sign_numeric_event("nonexistent".to_string(), 42)
            .await;
        assert!(matches!(result, Err(Error::NotFound)));
    }

    #[tokio::test]
    async fn test_error_event_already_signed() {
        let oracle = setup_test_oracle();

        // Create and sign an enum event
        let event_id = "test_event".to_string();
        let _ann = oracle
            .create_enum_event(
                event_id.clone(),
                vec!["a".to_string(), "b".to_string()],
                12345,
            )
            .await
            .unwrap();

        let _attestation = oracle
            .sign_enum_event(event_id.clone(), "a".to_string())
            .await
            .unwrap();

        // Try to sign again
        let result = oracle
            .sign_enum_event(event_id.clone(), "b".to_string())
            .await;
        assert!(matches!(result, Err(Error::EventAlreadySigned)));

        // Create and sign a numeric event
        let numeric_event_id = "numeric_event".to_string();
        let _ann = oracle
            .create_numeric_event(
                numeric_event_id.clone(),
                4,
                false,
                0,
                "m/s".to_string(),
                12345,
            )
            .await
            .unwrap();

        let _attestation = oracle
            .sign_numeric_event(numeric_event_id.clone(), 5)
            .await
            .unwrap();

        // Try to sign again
        let result = oracle.sign_numeric_event(numeric_event_id, 6).await;
        assert!(matches!(result, Err(Error::EventAlreadySigned)));
    }

    #[tokio::test]
    async fn test_error_storage_failure() {
        // Create a mock storage that returns StorageFailure
        struct FailingStorage;

        impl crate::storage::Storage for FailingStorage {
            async fn get_next_nonce_indexes(&self, _num: usize) -> Result<Vec<u32>, Error> {
                Err(Error::StorageFailure)
            }

            async fn save_announcement(
                &self,
                _announcement: OracleAnnouncement,
                _indexes: Vec<u32>,
            ) -> Result<String, Error> {
                Err(Error::StorageFailure)
            }

            async fn save_signatures(
                &self,
                _event_id: String,
                _sigs: Vec<(String, Signature)>,
            ) -> Result<crate::storage::OracleEventData, Error> {
                Err(Error::StorageFailure)
            }

            async fn get_event(
                &self,
                _event_id: String,
            ) -> Result<Option<crate::storage::OracleEventData>, Error> {
                Err(Error::StorageFailure)
            }
        }

        let mut seed: [u8; 64] = [0; 64];
        thread_rng().fill(&mut seed);
        let xpriv = Xpriv::new_master(Network::Regtest, &seed).unwrap();
        let oracle = Oracle::from_xpriv(FailingStorage, xpriv).unwrap();

        // Test StorageFailure on create_enum_event
        let result = oracle
            .create_enum_event("test".to_string(), vec!["a".to_string()], 12345)
            .await;
        assert!(matches!(result, Err(Error::StorageFailure)));

        // Test StorageFailure on create_numeric_event
        let result = oracle
            .create_numeric_event("test".to_string(), 4, false, 0, "m/s".to_string(), 12345)
            .await;
        assert!(matches!(result, Err(Error::StorageFailure)));

        // Test StorageFailure on get_event (sign_enum_event)
        let result = oracle
            .sign_enum_event("test".to_string(), "a".to_string())
            .await;
        assert!(matches!(result, Err(Error::StorageFailure)));

        // Test StorageFailure on get_event (sign_numeric_event)
        let result = oracle.sign_numeric_event("test".to_string(), 42).await;
        assert!(matches!(result, Err(Error::StorageFailure)));
    }

    #[tokio::test]
    async fn test_kormir_create_enum_event() {
        let oracle = setup_test_oracle();

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
    async fn test_kormir_sign_enum_event() {
        let oracle = setup_test_oracle();

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

        assert_eq!(rx, expected_nonce)
    }

    #[tokio::test]
    async fn test_kormir_create_unsigned_numeric_event() {
        let oracle = setup_test_oracle();

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
    async fn test_kormir_sign_unsigned_numeric_event() {
        let oracle = setup_test_oracle();

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
    }

    #[tokio::test]
    async fn test_kormir_create_signed_numeric_event() {
        let oracle = setup_test_oracle();

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
    async fn test_kormir_sign_signed_positive_numeric_event() {
        let oracle = setup_test_oracle();

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
    }

    #[tokio::test]
    async fn test_kormir_sign_signed_negative_numeric_event() {
        let oracle = setup_test_oracle();

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
    }
}
