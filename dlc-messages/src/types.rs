//! Type definitions and constants for DLC messages

use lightning::ln::wire::Type;

// Define all type constants and implement Type trait using macro
macro_rules! impl_type {
    ($const_name: ident, $type_name: ident, $type_val: expr) => {
        /// The type prefix for a message.
        pub const $const_name: u16 = $type_val;

        impl Type for $type_name {
            fn type_id(&self) -> u16 {
                $const_name
            }
        }
    };
}

// Re-export the types that will get impl_type
pub use crate::{AcceptDlc, CloseDlc, OfferDlc, SignDlc};

pub use crate::channel::{
    AcceptChannel, CollaborativeCloseOffer, OfferChannel, Reject, RenewAccept, RenewConfirm,
    RenewFinalize, RenewOffer, RenewRevoke, SettleAccept, SettleConfirm, SettleFinalize,
    SettleOffer, SignChannel,
};

// DLC message types
impl_type!(OFFER_TYPE, OfferDlc, 42778);
impl_type!(ACCEPT_TYPE, AcceptDlc, 42780);
impl_type!(SIGN_TYPE, SignDlc, 42782);
impl_type!(CLOSE_TYPE, CloseDlc, 42784);

// Channel message types
impl_type!(OFFER_CHANNEL_TYPE, OfferChannel, 43000);
impl_type!(ACCEPT_CHANNEL_TYPE, AcceptChannel, 43002);
impl_type!(SIGN_CHANNEL_TYPE, SignChannel, 43004);
impl_type!(SETTLE_CHANNEL_OFFER_TYPE, SettleOffer, 43006);
impl_type!(SETTLE_CHANNEL_ACCEPT_TYPE, SettleAccept, 43008);
impl_type!(SETTLE_CHANNEL_CONFIRM_TYPE, SettleConfirm, 43010);
impl_type!(SETTLE_CHANNEL_FINALIZE_TYPE, SettleFinalize, 43012);
impl_type!(RENEW_CHANNEL_OFFER_TYPE, RenewOffer, 43014);
impl_type!(RENEW_CHANNEL_ACCEPT_TYPE, RenewAccept, 43016);
impl_type!(RENEW_CHANNEL_CONFIRM_TYPE, RenewConfirm, 43018);
impl_type!(RENEW_CHANNEL_FINALIZE_TYPE, RenewFinalize, 43020);
impl_type!(RENEW_CHANNEL_REVOKE_TYPE, RenewRevoke, 43026);
impl_type!(
    COLLABORATIVE_CLOSE_OFFER_TYPE,
    CollaborativeCloseOffer,
    43022
);
impl_type!(REJECT, Reject, 43024);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract_msgs::{
        ContractDescriptor, ContractInfo, ContractInfoInner, EnumeratedContractDescriptor,
    };
    use crate::oracle_msgs::{OracleEvent, OracleInfo};
    use crate::OfferDlc;
    use bitcoin::Amount;
    use bitcoin::ScriptBuf;
    use lightning::util::ser::{Readable, Writeable};
    use secp256k1_zkp::{rand, SECP256K1};

    fn xonly_pubkey() -> secp256k1_zkp::XOnlyPublicKey {
        secp256k1_zkp::Keypair::new(SECP256K1, &mut rand::thread_rng())
            .x_only_public_key()
            .0
    }

    fn pubkey() -> secp256k1_zkp::PublicKey {
        secp256k1_zkp::Keypair::new(SECP256K1, &mut rand::thread_rng()).public_key()
    }

    #[test]
    fn test_type_id_serialization() {
        // Create a minimal OfferDlc for testing
        let offer = OfferDlc {
            protocol_version: 1,
            contract_flags: 0,
            chain_hash: [0u8; 32],
            temporary_contract_id: [1u8; 32],
            contract_info: ContractInfo::SingleContractInfo(
                crate::contract_msgs::SingleContractInfo {
                    total_collateral: Amount::from_sat(100000),
                    contract_info: ContractInfoInner {
                        contract_descriptor: ContractDescriptor::EnumeratedContractDescriptor(
                            EnumeratedContractDescriptor { payouts: vec![] },
                        ),
                        oracle_info: OracleInfo::Single(crate::oracle_msgs::SingleOracleInfo {
                            oracle_announcement: crate::oracle_msgs::OracleAnnouncement {
                                announcement_signature:
                                    secp256k1_zkp::schnorr::Signature::from_slice(&[0u8; 64])
                                        .unwrap(),
                                oracle_public_key: xonly_pubkey(),
                                oracle_event: OracleEvent {
                                    oracle_nonces: vec![xonly_pubkey()],
                                    event_maturity_epoch: 1,
                                    event_id: "oracle".to_string(),
                                    event_descriptor:
                                        crate::oracle_msgs::EventDescriptor::EnumEvent(
                                            crate::oracle_msgs::EnumEventDescriptor {
                                                outcomes: vec!["1".to_string(), "2".to_string()],
                                            },
                                        ),
                                },
                            },
                        }),
                    },
                },
            ),
            funding_pubkey: pubkey(),
            payout_spk: ScriptBuf::new(),
            payout_serial_id: 0,
            offer_collateral: Amount::from_sat(50000),
            funding_inputs: vec![],
            change_spk: ScriptBuf::new(),
            change_serial_id: 1,
            fund_output_serial_id: 2,
            fee_rate_per_vb: 1,
            cet_locktime: 100,
            refund_locktime: 200,
        };

        // Serialize the offer
        let mut serialized = Vec::new();
        offer.write(&mut serialized).unwrap();

        // Check that the first 2 bytes are the type_id
        assert_eq!(&serialized[0..2], &OFFER_TYPE.to_be_bytes());

        // Deserialize and check we get the same offer back
        let deserialized = OfferDlc::read(&mut &serialized[..]).unwrap();
        assert_eq!(offer.protocol_version, deserialized.protocol_version);
        assert_eq!(
            offer.temporary_contract_id,
            deserialized.temporary_contract_id
        );
    }

    #[test]
    fn test_wrong_type_id_fails() {
        // Create a buffer with wrong type_id
        let mut bad_data = Vec::new();
        bad_data.extend_from_slice(&9999u16.to_be_bytes()); // Wrong type_id
        bad_data.extend_from_slice(&[0u8; 100]); // Some dummy data

        // Should fail to deserialize
        let result = OfferDlc::read(&mut &bad_data[..]);
        assert!(result.is_err());
    }
}
