//! Structs containing oracle information.

use crate::ser_impls::{
    read_as_tlv, read_i32, read_schnorr_pubkey, read_schnorrsig, read_strings_u16, write_as_tlv,
    write_i32, write_schnorr_pubkey, write_schnorrsig, write_strings_u16, TlvRecord,
};
use bitcoin::hashes::{Hash, HashEngine};
use ddk_dlc::{Error, OracleInfo as DlcOracleInfo};
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{Readable, Writeable, Writer};
use secp256k1_zkp::Verification;
use secp256k1_zkp::{schnorr::Signature, Message, Secp256k1, XOnlyPublicKey};
#[cfg(feature = "use-serde")]
use serde::{Deserialize, Serialize};

/// The type of the announcement struct.
pub const ANNOUNCEMENT_TYPE: u16 = 55332;
/// The type of the oracle event struct.
pub const ORACLE_EVENT_TYPE: u16 = 55330;
/// The type of the attestation struct.
pub const ATTESTATION_TYPE: u16 = 55400;

/// The tag of the oracle announcement struct.
pub const ORACLE_ANNOUNCEMENT_TAG: &[u8] = b"DLC/oracle/announcement/v0";
/// The tag of the oracle attestation struct.
pub const ORACLE_ATTESTATION_TAG: &[u8] = b"DLC/oracle/attestation/v0";

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Information about an oracle used in a contract.
pub enum OracleInfo {
    /// Used when a contract uses a single oracle.
    Single(SingleOracleInfo),
    /// Used when a contract uses multiple oracles.
    Multi(MultiOracleInfo),
}

impl<'a> OracleInfo {
    /// Returns the first event descriptor.
    pub fn get_first_event_descriptor(&'a self) -> &'a EventDescriptor {
        match self {
            OracleInfo::Single(single) => &single.oracle_announcement.oracle_event.event_descriptor,
            OracleInfo::Multi(multi) => {
                &multi.oracle_announcements[0].oracle_event.event_descriptor
            }
        }
    }
}

impl OracleInfo {
    /// Returns the closest maturity date amongst all events
    pub fn get_closest_maturity_date(&self) -> u32 {
        match self {
            OracleInfo::Single(s) => s.oracle_announcement.oracle_event.event_maturity_epoch,
            OracleInfo::Multi(m) => m
                .oracle_announcements
                .iter()
                .map(|x| x.oracle_event.event_maturity_epoch)
                .min()
                .expect("to have at least one event"),
        }
    }

    /// Checks that the info satisfies the validity conditions.
    pub fn validate<C: Verification>(&self, secp: &Secp256k1<C>) -> Result<(), Error> {
        match self {
            OracleInfo::Single(s) => s.oracle_announcement.validate(secp)?,
            OracleInfo::Multi(m) => {
                for o in &m.oracle_announcements {
                    o.validate(secp)?;
                }
            }
        };

        Ok(())
    }
}

impl_dlc_writeable_enum!(
    OracleInfo, (0, Single), (1, Multi);;;
);

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Structure containing information about an oracle to be used as external
/// data source for a DLC contract.
pub struct SingleOracleInfo {
    /// The oracle announcement from the oracle.
    pub oracle_announcement: OracleAnnouncement,
}

impl_dlc_writeable!(SingleOracleInfo, {
    (oracle_announcement, {cb_writeable, write_as_tlv, read_as_tlv })
});

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Information about oracles used in multi oracle based contracts.
pub struct MultiOracleInfo {
    /// The threshold to be used for the contract (e.g. 2 of 3).
    pub threshold: u16,
    /// The set of oracle announcements.
    pub oracle_announcements: Vec<OracleAnnouncement>,
    /// The parameters to be used when allowing differences between oracle
    /// outcomes in numerical outcome contracts.
    pub oracle_params: Option<OracleParams>,
}

impl_dlc_writeable!(MultiOracleInfo, {
    (threshold, writeable),
    (oracle_announcements, {vec_cb, write_as_tlv, read_as_tlv}),
    (oracle_params, option)
});

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Parameter describing allowed differences between oracles in numerical outcome
/// contracts.
pub struct OracleParams {
    /// The maximum allowed difference between oracle expressed as a power of 2.
    pub max_error_exp: u16,
    /// The minimum allowed difference that should be supported by the contract
    /// expressed as a power of 2.
    pub min_fail_exp: u16,
    /// Whether to maximize coverage of the interval between [`Self::max_error_exp`]
    /// and [`Self::min_fail_exp`].
    pub maximize_coverage: bool,
}

impl_dlc_writeable!(OracleParams, {
    (max_error_exp, writeable),
    (min_fail_exp, writeable),
    (maximize_coverage, writeable)
});

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
/// An oracle announcement that describe an event and the way that an oracle will
/// attest to it.
pub struct OracleAnnouncement {
    /// The signature enabling verifying the origin of the announcement.
    pub announcement_signature: Signature,
    /// The public key of the oracle.
    pub oracle_public_key: XOnlyPublicKey,
    /// The description of the event and attesting.
    pub oracle_event: OracleEvent,
}

impl_dlc_tlv_record!(OracleAnnouncement, ANNOUNCEMENT_TYPE);

/// Returns the message to be signed for an oracle announcement.
///
/// Follows the signing validation rules from the [DLC spec](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#signing-algorithm).
///
/// The event is hashed in its standalone TLV form, header included, which is what
/// oracles in the wild sign over.
pub fn tagged_announcement_msg(event: &OracleEvent) -> Message {
    let tag_hash = bitcoin::hashes::sha256::Hash::hash(ORACLE_ANNOUNCEMENT_TAG);
    let event_hex = event.to_tlv_bytes();
    let mut hash_engine = bitcoin::hashes::sha256::Hash::engine();
    hash_engine.input(&tag_hash[..]);
    hash_engine.input(&tag_hash[..]);
    hash_engine.input(&event_hex);
    let hash = bitcoin::hashes::sha256::Hash::from_engine(hash_engine);
    Message::from_digest(hash.to_byte_array())
}

/// Returns the message to be signed for an oracle attestation.
///
/// Follows the signing validation rules from the [DLC spec](https://github.com/discreetlogcontracts/dlcspecs/blob/master/Oracle.md#signing-algorithm).
pub fn tagged_attestation_msg(outcome: &str) -> Message {
    let tag_hash = bitcoin::hashes::sha256::Hash::hash(ORACLE_ATTESTATION_TAG);
    let mut hash_engine = bitcoin::hashes::sha256::Hash::engine();
    hash_engine.input(&tag_hash[..]);
    hash_engine.input(&tag_hash[..]);
    hash_engine.input(outcome.as_bytes());
    let hash = bitcoin::hashes::sha256::Hash::from_engine(hash_engine);
    Message::from_digest(hash.to_byte_array())
}

impl OracleAnnouncement {
    /// Returns whether the announcement satisfy validity checks.
    pub fn validate<C: Verification>(&self, secp: &Secp256k1<C>) -> Result<(), Error> {
        let msg = tagged_announcement_msg(&self.oracle_event);
        secp.verify_schnorr(&self.announcement_signature, &msg, &self.oracle_public_key)?;
        self.oracle_event.validate()
    }
}

impl_dlc_writeable!(OracleAnnouncement, {
    (announcement_signature, {cb_writeable, write_schnorrsig, read_schnorrsig}),
    (oracle_public_key, {cb_writeable, write_schnorr_pubkey, read_schnorr_pubkey}),
    (oracle_event, {cb_writeable, write_as_tlv, read_as_tlv})
});

impl From<&OracleAnnouncement> for DlcOracleInfo {
    fn from(input: &OracleAnnouncement) -> DlcOracleInfo {
        DlcOracleInfo {
            public_key: input.oracle_public_key,
            nonces: input.oracle_event.oracle_nonces.clone(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
/// Information about an event and the way that the oracle will attest to it.
pub struct OracleEvent {
    /// The nonces that the oracle will use to attest to the event outcome.
    pub oracle_nonces: Vec<XOnlyPublicKey>,
    /// The expected maturity of the contract.
    // TODO(tibo): should validate that with the contract maturity.
    pub event_maturity_epoch: u32,
    /// The description of the event.
    pub event_descriptor: EventDescriptor,
    /// The id of the event.
    pub event_id: String,
}

impl OracleEvent {
    /// Returns whether the event passes validity checks.
    pub fn validate(&self) -> Result<(), Error> {
        let expected_nb_nonces = match &self.event_descriptor {
            EventDescriptor::EnumEvent(_) => 1,
            EventDescriptor::DigitDecompositionEvent(d) => {
                if d.is_signed {
                    d.nb_digits as usize + 1
                } else {
                    d.nb_digits as usize
                }
            }
        };

        if expected_nb_nonces == self.oracle_nonces.len() {
            Ok(())
        } else {
            Err(Error::InvalidArgument(format!(
                "Expected number of nonces is not equal to actual number of nonces. expected={} actual={}",
                expected_nb_nonces,
                self.oracle_nonces.len()
            )))
        }
    }
}

impl_dlc_tlv_record!(OracleEvent, ORACLE_EVENT_TYPE);

impl_dlc_writeable!(OracleEvent, {
    (oracle_nonces, {vec_u16_cb, write_schnorr_pubkey, read_schnorr_pubkey}),
    (event_maturity_epoch, writeable),
    (event_descriptor, writeable),
    (event_id, string)
});

#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
/// Description of an event.
pub enum EventDescriptor {
    /// Used for events based on enumerated outcomes.
    EnumEvent(EnumEventDescriptor),
    /// Used for event based on numerical outcomes.
    DigitDecompositionEvent(DigitDecompositionEventDescriptor),
}

impl_dlc_writeable_enum_as_tlv!(EventDescriptor, (55302, EnumEvent), (55306, DigitDecompositionEvent););

#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
/// Describes the outcomes of an event as an enumeration.
pub struct EnumEventDescriptor {
    /// The possible outcomes of the event.
    pub outcomes: Vec<String>,
}

impl_dlc_writeable!(EnumEventDescriptor, {
    (outcomes, {cb_writeable, write_strings_u16, read_strings_u16})
});

#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
/// Describes the outcomes of a numerical outcome event.
pub struct DigitDecompositionEventDescriptor {
    /// The base in which the outcome will be represented.
    pub base: u16,
    /// Whether the outcome value is signed.
    pub is_signed: bool,
    /// The unit in which the outcome is represented.
    pub unit: String,
    /// The precision used to represent the event outcome.
    pub precision: i32,
    /// The number of digits used to represent the event outcome.
    // TODO:(tibo) should validate that nb_digits == nb_nonces
    pub nb_digits: u16,
}

impl_dlc_writeable!(DigitDecompositionEventDescriptor, {
    (base, writeable),
    (is_signed, writeable),
    (unit, string),
    (precision, {cb_writeable, write_i32, read_i32}),
    (nb_digits, writeable)
});

/// An attestation from an oracle providing signatures over an outcome value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct OracleAttestation {
    /// The identifier of the announcement.
    pub event_id: String,
    /// The public key of the oracle.
    pub oracle_public_key: XOnlyPublicKey,
    /// The signatures over the event outcome.
    pub signatures: Vec<Signature>,
    /// The set of strings representing the outcome value.
    pub outcomes: Vec<String>,
}

impl OracleAttestation {
    /// Returns whether the attestation satisfy validity checks.
    pub fn validate<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        announcement: &OracleAnnouncement,
    ) -> Result<(), Error> {
        if self.outcomes.len() != self.signatures.len() {
            return Err(Error::InvalidArgument(format!(
                "Outcomes length is not equal to signatures length. outcomes={} signatures={}",
                self.outcomes.len(),
                self.signatures.len()
            )));
        }

        if self.oracle_public_key != announcement.oracle_public_key {
            return Err(Error::InvalidArgument(format!(
                "Oracle public key is not equal to announcement oracle public key. oracle_public_key={} announcement_oracle_public_key={}",
                self.oracle_public_key,
                announcement.oracle_public_key
            )));
        }

        self.signatures
            .iter()
            .zip(self.outcomes.iter())
            .try_for_each(|(sig, outcome)| {
                let msg = tagged_attestation_msg(outcome);
                secp.verify_schnorr(sig, &msg, &self.oracle_public_key)
                    .map_err(|_| Error::InvalidArgument(format!(
                        "Failed to verify schnorr signature. signature={} oracle_public_key={} msg={}",
                        sig,
                        self.oracle_public_key,
                        msg
                    )))?;

                Ok::<(), ddk_dlc::Error>(())
            })?;

        if !self
            .signatures
            .iter()
            .zip(announcement.oracle_event.oracle_nonces.iter())
            .all(|(sig, nonce)| sig.encode()[..32] == nonce.serialize())
        {
            return Err(Error::InvalidArgument(format!(
                "Signatures are not equal to nonces. signatures={} nonces={}",
                self.signatures.len(),
                announcement.oracle_event.oracle_nonces.len()
            )));
        }

        Ok(())
    }
    /// Returns the nonces used by the oracle to sign the event outcome.
    /// This is used for finding the matching oracle announcement.
    pub fn nonces(&self) -> Vec<XOnlyPublicKey> {
        self.signatures
            .iter()
            .map(|s| XOnlyPublicKey::from_slice(&s[0..32]).expect("valid signature"))
            .collect()
    }
}

impl_dlc_tlv_record!(OracleAttestation, ATTESTATION_TYPE);

impl_dlc_writeable!(OracleAttestation, {
    (event_id, string),
    (oracle_public_key, {cb_writeable, write_schnorr_pubkey, read_schnorr_pubkey}),
    (signatures, {vec_u16_cb, write_schnorrsig, read_schnorrsig}),
    (outcomes, {cb_writeable, write_strings_u16, read_strings_u16})
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ser_impls::TlvType;
    use bitcoin::bip32::{ChildNumber, Xpriv};
    use bitcoin::hex::FromHex;
    use bitcoin::Network;
    use secp256k1_zkp::rand::Fill;
    use secp256k1_zkp::SecretKey;
    use secp256k1_zkp::{rand::thread_rng, SECP256K1};
    use secp256k1_zkp::{schnorr::Signature as SchnorrSignature, Keypair, XOnlyPublicKey};

    fn enum_descriptor() -> EnumEventDescriptor {
        EnumEventDescriptor {
            outcomes: vec!["1".to_string(), "2".to_string(), "3".to_string()],
        }
    }

    fn digit_descriptor() -> DigitDecompositionEventDescriptor {
        DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "kg/sats".to_string(),
            precision: 1,
            nb_digits: 10,
        }
    }

    fn signed_digit_descriptor() -> DigitDecompositionEventDescriptor {
        DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: true,
            unit: "kg/sats".to_string(),
            precision: 1,
            nb_digits: 10,
        }
    }

    fn some_schnorr_pubkey() -> XOnlyPublicKey {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        XOnlyPublicKey::from_keypair(&key_pair).0
    }

    fn digit_event(nb_nonces: usize) -> OracleEvent {
        OracleEvent {
            oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
            event_maturity_epoch: 10,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(digit_descriptor()),
            event_id: "test".to_string(),
        }
    }

    fn signed_digit_event(nb_nonces: usize) -> OracleEvent {
        OracleEvent {
            oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
            event_maturity_epoch: 10,
            event_descriptor: EventDescriptor::DigitDecompositionEvent(signed_digit_descriptor()),
            event_id: "test-signed".to_string(),
        }
    }

    fn enum_event(nb_nonces: usize) -> OracleEvent {
        OracleEvent {
            oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
            event_maturity_epoch: 10,
            event_descriptor: EventDescriptor::EnumEvent(enum_descriptor()),
            event_id: "test".to_string(),
        }
    }

    fn create_nonce_key() -> (SecretKey, XOnlyPublicKey) {
        let mut nonce_seed = [0u8; 32];
        nonce_seed.try_fill(&mut thread_rng()).unwrap();
        let nonce_priv = Xpriv::new_master(Network::Bitcoin, &nonce_seed)
            .unwrap()
            .derive_priv(SECP256K1, &[ChildNumber::from_normal_idx(1).unwrap()])
            .unwrap()
            .private_key;

        let nonce_xpub = nonce_priv.x_only_public_key(SECP256K1).0;

        (nonce_priv, nonce_xpub)
    }

    /// A real announcement served by a production oracle, in the standalone hex form
    /// its HTTP API returns.
    const REAL_ANNOUNCEMENT_HEX: &str = "fdd824fd012a73740e61118e5d1c2c223c986b859a42c2cca56cb621d13ff8880ea856caf24aa29bc36a1e0d5c471b6a2baa68f98a413362c4e6a97f13c04b186395e9e0d8fcc3d07289c2ade25405c1c421b38c9322cd73fb2c89f42ce0730a35fae1f8875dfdd822c60001529fadc9958e1e1cf8ea29f05a25d67e04a0c21dcbfcb99b8b24d32f274795eb68e81101fdd8064e0004086e6f742d70616964067265706169641d6c6971756964617465642d62792d6d617475726174696f6e2d646174651d6c6971756964617465642d62792d70726963652d7468726573686f6c644d6c6f616e2d6d6174757265642d38313233313935633631653439376631323465623764336266626531323232613530326233306162343139363766306466323036306133656533366635623063";

    /// The matching attestation from the same oracle.
    const REAL_ATTESTATION_HEX: &str = "fdd868d04d6c6f616e2d6d6174757265642d39666235336365663365663134383863626230373532643036636263373738633237353532663963373666656634353436653637343038353935626532363832dde465c101a1aaa5a88c0d35d21744eb5352c62b7c7665d62d5f20c770ddfd8f00010c914fa0638767ee55ef16ea009f85548ac43c3d447b47c1ff22724fd0a48b159c9690d0cec7e7ae47a1ceb232b0a2c13404173b9f5e9dc16fe63d31c78c615100011d6c6971756964617465642d62792d6d617475726174696f6e2d64617465";

    /// A numeric announcement, taken from node-dlc's own test vectors.
    ///
    /// This is the only fixture covering `digit_decomposition_event_descriptor`
    /// (55306) in bytes another implementation produced. The enum fixtures all
    /// take the 55302 branch, so without this the descriptor read path is
    /// exercised only against announcements this crate wrote itself.
    const NODE_DLC_NUMERIC_ANNOUNCEMENT_HEX: &str = "fdd824fd02ab1efe41fa42ea1dcd103a0251929dd2b192d2daece8a4ce4d81f68a183b750d92d6f02d796965dc79adf4e7786e08f861a1ecc897afbba2dab9cff6eb0a81937eb8b005b07acf849ad2cec22107331dedbf5a607654fad4eafe39c278e27dde68fdd822fd02450011f9313f1edd903fab297d5350006b669506eb0ffda0bb58319b4df89ac24e14fd15f9791dc78d1596b06f4969bdb37d9e394dc9fedaa18d694027fa32b5ea2a5e60080c58e13727367c3a4ce1ad65dfb3c7e3ca1ea912b0299f6e383bab2875058aa96a1c74633130af6fbd008788de6ac9db76da4ecc7303383cc1a49f525316413850f7e3ac385019d560e84c5b3a3e9ae6c83f59fe4286ddfd23ea46d7ae04610a175cd28a9bf5f574e245c3dfe230dc4b0adf4daaea96780e594f6464f676505f4b74cfe3ffc33415a23de795bf939ce64c0c02033bbfc6c9ff26fb478943a1ece775f38f5db067ca4b2a9168b40792398def9164bfe5c46838472dc3c162af16c811b7a116e9417d5bccb9e5b8a5d7d26095aba993696188c3f85a02f7ab8d12ada171c352785eb63417228c7e248909fc2d673e1bb453140bf8bf429375819afb5e9556663b76ff09c2a7ba9779855ffddc6d360cb459cf8c42a2b949d0de19fe96163d336fd66a4ce2f1791110e679572a20036ffae50204ef520c01058ff4bef28218d1c0e362ee3694ad8b2ae83a51c86c4bc1630ed6202a158810096726f809fc828fafdcf053496affdf887ae8c54b6ca4323ccecf6a51121c4f0c60e790536dab41b221db1c6b35065dc19a9d31cf75901aa35eefecbb6fefd07296cda13cb34ce3b58eba20a0eb8f9614994ec7fee3cc290e30e6b1e3211ae1f3a85b6de6abdbb77d6d9ed33a1cee3bd5cd93a71f12c9c45e385d744ad0e7286660305100fdd80a11000200076274632f75736400000000001109425443205072696365";

    /// An enum announcement, also from node-dlc's test vectors.
    const NODE_DLC_ENUM_ANNOUNCEMENT_HEX: &str = "fdd824a4fab22628f6e2602e1671c286a2f63a9246794008627a1749639217f4214cb4a9494c93d1a852221080f44f697adb4355df59eb339f6ba0f9b01ba661a8b108d4da078bbb1d34e7729e38e2ae34236e776da121af442626fa31e31ae55a279a0bfdd8224000013cfba011378411b20a5ab773cb95daab93e9bcd1e4cce44986a7dda84e01841b00000000fdd8061000020664756d6d79310664756d6d79320564756d6d79";

    /// The record types are consensus. Pinning them here means a rename or a
    /// refactor that changes one is a test failure rather than a wire break
    /// discovered by a peer.
    #[test]
    fn record_types_match_the_specification() {
        assert_eq!(<OracleAnnouncement as TlvType>::TYPE_ID, 55332);
        assert_eq!(<OracleEvent as TlvType>::TYPE_ID, 55330);
        assert_eq!(<OracleAttestation as TlvType>::TYPE_ID, 55400);
    }

    /// The JSON shape is a separate wire format with its own consumers: oracle
    /// HTTP APIs exchange announcements and attestations this way, and it is what
    /// `KormirOracleClient` deserializes. Nothing here changed it, and this pins
    /// that so a future serialization change cannot break it unnoticed.
    #[cfg(feature = "use-serde")]
    #[test]
    fn json_representation_is_unchanged() {
        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX).unwrap();
        let value = serde_json::to_value(&announcement).unwrap();

        // camelCase keys, hex-encoded keys and signatures.
        assert!(value.get("announcementSignature").is_some());
        assert!(value.get("oraclePublicKey").is_some());
        let event = value.get("oracleEvent").expect("oracleEvent");
        assert!(event.get("oracleNonces").is_some());
        assert!(event.get("eventMaturityEpoch").is_some());
        assert!(event.get("eventDescriptor").is_some());
        assert_eq!(
            event.get("eventId").and_then(|v| v.as_str()),
            Some("loan-matured-8123195c61e497f124eb7d3bfbe1222a502b30ab41967f0df2060a3ee36f5b0c")
        );

        let back: OracleAnnouncement = serde_json::from_value(value).unwrap();
        assert_eq!(back, announcement);
        assert_eq!(back.to_tlv_hex(), REAL_ANNOUNCEMENT_HEX);

        let attestation = OracleAttestation::from_tlv_hex(REAL_ATTESTATION_HEX).unwrap();
        let back: OracleAttestation =
            serde_json::from_value(serde_json::to_value(&attestation).unwrap()).unwrap();
        assert_eq!(back, attestation);
        assert_eq!(back.to_tlv_hex(), REAL_ATTESTATION_HEX);
    }

    #[test]
    fn node_dlc_announcements_round_trip() {
        for hex in [
            NODE_DLC_NUMERIC_ANNOUNCEMENT_HEX,
            NODE_DLC_ENUM_ANNOUNCEMENT_HEX,
        ] {
            let announcement =
                OracleAnnouncement::from_tlv_hex(hex).expect("a node-dlc announcement to parse");
            assert_eq!(announcement.to_tlv_hex(), hex);
        }
    }

    #[test]
    fn node_dlc_numeric_announcement_reads_its_digit_decomposition_descriptor() {
        let announcement = OracleAnnouncement::from_tlv_hex(NODE_DLC_NUMERIC_ANNOUNCEMENT_HEX)
            .expect("a node-dlc numeric announcement to parse");

        let EventDescriptor::DigitDecompositionEvent(descriptor) =
            &announcement.oracle_event.event_descriptor
        else {
            panic!("expected a digit decomposition descriptor");
        };

        assert_eq!(descriptor.base, 2);
        assert!(!descriptor.is_signed);
        assert_eq!(descriptor.unit, "btc/usd");
        assert_eq!(descriptor.precision, 0);
        assert_eq!(descriptor.nb_digits, 17);
        assert_eq!(announcement.oracle_event.event_id, "BTC Price");
        assert_eq!(announcement.oracle_event.oracle_nonces.len(), 17);

        // One nonce per digit, which is what the announcement claims and what a
        // contract built from it will rely on.
        announcement.oracle_event.validate().expect("a valid event");
    }

    #[test]
    fn real_oracle_announcement_reads_from_its_standalone_hex() {
        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX)
            .expect("a production announcement to parse");

        assert_eq!(
            announcement.oracle_event.event_id,
            "loan-matured-8123195c61e497f124eb7d3bfbe1222a502b30ab41967f0df2060a3ee36f5b0c"
        );
        announcement
            .validate(SECP256K1)
            .expect("a production announcement to carry a valid signature");

        // The bytes we write back must be the exact bytes the oracle signed over.
        assert_eq!(announcement.to_tlv_hex(), REAL_ANNOUNCEMENT_HEX);
    }

    #[test]
    fn real_oracle_attestation_reads_from_its_standalone_hex() {
        let attestation = OracleAttestation::from_tlv_hex(REAL_ATTESTATION_HEX)
            .expect("a production attestation to parse");

        assert_eq!(attestation.outcomes, vec!["liquidated-by-maturation-date"]);
        assert_eq!(attestation.signatures.len(), 1);
        assert_eq!(attestation.to_tlv_hex(), REAL_ATTESTATION_HEX);
    }

    #[test]
    fn tlv_form_and_body_form_are_distinct_and_neither_reads_as_the_other() {
        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX).unwrap();
        let tlv = announcement.to_tlv_bytes();
        let body = announcement.encode();

        // The TLV form is the body behind a type and length header, and each reader
        // accepts only its own form. This is the mismatch that made callers strip
        // the header by hand; `TlvRecord` is what they should reach for instead.
        let mut header = Vec::new();
        crate::ser_impls::BigSize(ANNOUNCEMENT_TYPE as u64)
            .write(&mut header)
            .unwrap();
        crate::ser_impls::BigSize(body.len() as u64)
            .write(&mut header)
            .unwrap();
        assert_eq!(tlv, [header.as_slice(), body.as_slice()].concat());
        assert!(OracleAnnouncement::read(&mut lightning::io::Cursor::new(&tlv)).is_err());
        assert!(OracleAnnouncement::from_tlv_bytes(&body).is_err());
    }

    #[test]
    fn legacy_body_bytes_still_read_through_the_compatibility_reader() {
        let announcement = OracleAnnouncement::from_tlv_hex(REAL_ANNOUNCEMENT_HEX).unwrap();
        let attestation = OracleAttestation::from_tlv_hex(REAL_ATTESTATION_HEX).unwrap();

        // Both the stored form written before the standalone form was settled...
        assert_eq!(
            OracleAnnouncement::from_tlv_bytes_or_legacy(&announcement.encode()).unwrap(),
            announcement
        );
        assert_eq!(
            OracleAttestation::from_tlv_bytes_or_legacy(&attestation.encode()).unwrap(),
            attestation
        );

        // ...and the current one are readable, so a store holding a mix of the two
        // migrates without anyone having to know which row is which.
        assert_eq!(
            OracleAnnouncement::from_tlv_bytes_or_legacy(&announcement.to_tlv_bytes()).unwrap(),
            announcement
        );
        assert_eq!(
            OracleAttestation::from_tlv_bytes_or_legacy(&attestation.to_tlv_bytes()).unwrap(),
            attestation
        );
    }

    #[test]
    fn a_record_of_the_wrong_type_is_rejected_rather_than_misread() {
        let announcement_bytes = Vec::<u8>::from_hex(REAL_ANNOUNCEMENT_HEX).unwrap();
        let attestation_bytes = Vec::<u8>::from_hex(REAL_ATTESTATION_HEX).unwrap();

        assert!(OracleAttestation::from_tlv_bytes(&announcement_bytes).is_err());
        assert!(OracleAnnouncement::from_tlv_bytes(&attestation_bytes).is_err());

        // The lenient reader must not paper over a type mismatch by falling through
        // to the body reader and decoding the header as message content.
        assert!(OracleAttestation::from_tlv_bytes_or_legacy(&announcement_bytes).is_err());
    }

    #[test]
    fn truncated_and_overlong_records_are_rejected() {
        let bytes = Vec::<u8>::from_hex(REAL_ANNOUNCEMENT_HEX).unwrap();

        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(OracleAnnouncement::from_tlv_bytes(&truncated).is_err());

        let mut trailing = bytes.clone();
        trailing.push(0x00);
        assert!(OracleAnnouncement::from_tlv_bytes(&trailing).is_err());

        // A header that claims a shorter body than the record actually holds must
        // fail, not silently return a value and leave the reader mid-record.
        let mut short_length = bytes.clone();
        short_length[4] = 0x2a - 1;
        assert!(OracleAnnouncement::from_tlv_bytes(&short_length).is_err());
    }

    #[test]
    fn valid_oracle_announcement_passes_validation_test() {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
        let events = [digit_event(10), signed_digit_event(11), enum_event(1)];
        for event in events {
            let msg = tagged_announcement_msg(&event);
            let sig = SECP256K1.sign_schnorr(&msg, &key_pair);
            let valid_announcement = OracleAnnouncement {
                announcement_signature: sig,
                oracle_public_key: oracle_pubkey,
                oracle_event: event,
            };

            valid_announcement
                .validate(SECP256K1)
                .expect("a valid announcement.");
        }
    }

    #[test]
    fn invalid_oracle_announcement_fails_validation_test() {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
        let events = [digit_event(9), signed_digit_event(10), enum_event(2)];
        for event in events {
            let msg = tagged_announcement_msg(&event);
            let sig = SECP256K1.sign_schnorr(&msg, &key_pair);
            let invalid_announcement = OracleAnnouncement {
                announcement_signature: sig,
                oracle_public_key: oracle_pubkey,
                oracle_event: event,
            };

            invalid_announcement
                .validate(SECP256K1)
                .expect_err("invalid announcement should fail validation.");
        }
    }

    #[test]
    fn invalid_oracle_announcement_signature_fails_validation_test() {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
        let event = digit_event(10);
        let msg = tagged_announcement_msg(&event);
        let sig = SECP256K1.sign_schnorr(&msg, &key_pair);
        let mut sig_hex = *sig.as_ref();
        sig_hex[10] = sig_hex[10].checked_add(1).unwrap_or(0);
        let sig = SchnorrSignature::from_slice(&sig_hex).unwrap();
        let invalid_announcement = OracleAnnouncement {
            announcement_signature: sig,
            oracle_public_key: oracle_pubkey,
            oracle_event: event,
        };

        assert!(invalid_announcement.validate(SECP256K1).is_err());
    }

    #[test]
    fn valid_oracle_attestation() {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
        let (nonce_secret, nonce_xpub) = create_nonce_key();

        let oracle_event = OracleEvent {
            event_id: "test".to_string(),
            event_maturity_epoch: 10,
            oracle_nonces: vec![nonce_xpub],
            event_descriptor: EventDescriptor::EnumEvent(enum_descriptor()),
        };

        let msg = tagged_announcement_msg(&oracle_event);
        let sig = SECP256K1.sign_schnorr(&msg, &key_pair);

        let valid_announcement = OracleAnnouncement {
            oracle_public_key: oracle_pubkey,
            announcement_signature: sig,
            oracle_event,
        };

        let msg = tagged_attestation_msg("1");
        let sig = ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
            SECP256K1,
            &msg,
            &key_pair,
            &nonce_secret.secret_bytes(),
        );

        let attestation = OracleAttestation {
            event_id: "test".to_string(),
            oracle_public_key: oracle_pubkey,
            signatures: vec![sig],
            outcomes: vec!["1".to_string()],
        };

        let validation = attestation.validate(SECP256K1, &valid_announcement);

        assert!(validation.is_ok())
    }

    #[test]
    fn invalid_attestation_incorrect_nonce() {
        let key_pair = Keypair::new(SECP256K1, &mut thread_rng());
        let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
        let (_, nonce_xpub) = create_nonce_key();
        let (incorrect_nonce_secret, _) = create_nonce_key();

        let oracle_event = OracleEvent {
            event_id: "test".to_string(),
            event_maturity_epoch: 10,
            oracle_nonces: vec![nonce_xpub],
            event_descriptor: EventDescriptor::EnumEvent(enum_descriptor()),
        };

        let msg = tagged_announcement_msg(&oracle_event);
        let sig = SECP256K1.sign_schnorr(&msg, &key_pair);

        let valid_announcement = OracleAnnouncement {
            oracle_public_key: oracle_pubkey,
            announcement_signature: sig,
            oracle_event,
        };

        let msg = tagged_attestation_msg("1");
        let sig = ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
            SECP256K1,
            &msg,
            &key_pair,
            &incorrect_nonce_secret.secret_bytes(),
        );

        let attestation = OracleAttestation {
            event_id: "test".to_string(),
            oracle_public_key: oracle_pubkey,
            signatures: vec![sig],
            outcomes: vec!["1".to_string()],
        };

        let validation = attestation.validate(SECP256K1, &valid_announcement);

        assert!(validation.is_err())
    }
}
