//! Data structure and functions related to peer communication.

// Coding conventions
#![forbid(unsafe_code)]
#![deny(non_upper_case_globals)]
#![deny(non_camel_case_types)]
#![deny(non_snake_case)]
#![deny(unused_mut)]
#![deny(dead_code)]
#![deny(unused_imports)]
#![deny(missing_docs)]

extern crate bitcoin;
extern crate ddk_dlc;
extern crate secp256k1_zkp;

/// The `lightning` crate this one serializes against, re-exported because the
/// macros here expand to `$crate::lightning::…` so callers do not need their own
/// `lightning` dependency.
pub extern crate lightning;
#[macro_use]
pub mod ser_macros;
pub mod ser_impls;

pub use ser_impls::{TlvRecord, TlvType};
pub use tlv_stream::{TlvStream, TlvStreamRecord};

#[cfg(any(test, feature = "use-serde"))]
extern crate serde;

#[cfg(test)]
extern crate serde_json;

pub mod channel;
pub mod contract_msgs;
pub mod message_handler;
pub mod oracle_msgs;
pub mod segmentation;
pub mod tlv_stream;
pub mod types;

#[cfg(any(test, feature = "use-serde"))]
pub mod serde_utils;

use std::fmt::Display;

use crate::ser_impls::{read_ecdsa_adaptor_signature, write_ecdsa_adaptor_signature};
use crate::types::*;
use bitcoin::{consensus::Decodable, OutPoint, Transaction};
use bitcoin::{Amount, ScriptBuf};
use channel::{
    AcceptChannel, CollaborativeCloseOffer, OfferChannel, Reject, RenewAccept, RenewConfirm,
    RenewFinalize, RenewOffer, RenewRevoke, SettleAccept, SettleConfirm, SettleFinalize,
    SettleOffer, SignChannel,
};
use contract_msgs::ContractInfo;
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_dlc::{Error, TxInputInfo};
use lightning::ln::msgs::DecodeError;
use lightning::ln::wire::Type;
use lightning::util::ser::{Readable, Writeable, Writer};
use secp256k1_zkp::Verification;
use secp256k1_zkp::{ecdsa::Signature, EcdsaAdaptorSignature, PublicKey, Secp256k1};
use segmentation::{SegmentChunk, SegmentStart};

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains information about a DLC input to be used in a funding transaction.
pub struct DlcInput {
    /// The local funding public key.
    pub local_fund_pubkey: PublicKey,
    /// The remote funding public key.
    pub remote_fund_pubkey: PublicKey,
    /// Contract id of the DLC input.
    pub contract_id: [u8; 32],
}

impl_dlc_writeable!(DlcInput, {
    (local_fund_pubkey, writeable),
    (remote_fund_pubkey, writeable),
    (contract_id, writeable)
});

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains information about a specific input to be used in a funding transaction,
/// as well as its corresponding on-chain UTXO.
pub struct FundingInput {
    /// Serial id used for input ordering in the funding transaction.
    pub input_serial_id: u64,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_string"
        )
    )]
    /// The previous transaction used by the associated input in serialized format.
    pub prev_tx: Vec<u8>,
    /// The vout of the output used by the associated input.
    pub prev_tx_vout: u32,
    /// The sequence number to use for the input.
    pub sequence: u32,
    /// The maximum witness length that can be used to spend the previous UTXO.
    pub max_witness_len: u16,
    /// The redeem script of the previous UTXO.
    pub redeem_script: ScriptBuf,
    /// The optional sub-type of including a DLC input.
    pub dlc_input: Option<DlcInput>,
}

impl_dlc_writeable!(FundingInput, {
    (input_serial_id, writeable),
    (prev_tx, vec),
    (prev_tx_vout, writeable),
    (sequence, writeable),
    (max_witness_len, writeable),
    (redeem_script, writeable),
    (dlc_input, option)
});

impl From<&FundingInput> for TxInputInfo {
    fn from(funding_input: &FundingInput) -> TxInputInfo {
        TxInputInfo {
            outpoint: OutPoint {
                txid: Transaction::consensus_decode(&mut funding_input.prev_tx.as_slice())
                    .expect("Transaction Decode Error")
                    .compute_txid(),
                vout: funding_input.prev_tx_vout,
            },
            max_witness_len: (funding_input.max_witness_len as usize),
            redeem_script: funding_input.redeem_script.clone(),
            serial_id: funding_input.input_serial_id,
        }
    }
}

impl From<&FundingInput> for DlcInputInfo {
    fn from(funding_input: &FundingInput) -> Self {
        let fund_tx = Transaction::consensus_decode(&mut funding_input.prev_tx.as_slice()).unwrap();
        Self {
            fund_tx: fund_tx.clone(),
            fund_vout: funding_input.prev_tx_vout,
            local_fund_pubkey: funding_input.dlc_input.as_ref().unwrap().local_fund_pubkey,
            remote_fund_pubkey: funding_input.dlc_input.as_ref().unwrap().remote_fund_pubkey,
            fund_amount: fund_tx.output[funding_input.prev_tx_vout as usize].value,
            max_witness_len: funding_input.max_witness_len as usize,
            input_serial_id: funding_input.input_serial_id,
            contract_id: funding_input.dlc_input.as_ref().unwrap().contract_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains an adaptor signature for a CET input and its associated DLEQ proof.
pub struct CetAdaptorSignature {
    /// The signature.
    pub signature: EcdsaAdaptorSignature,
}

impl_dlc_writeable!(CetAdaptorSignature, {
     (signature, { cb_writeable, write_ecdsa_adaptor_signature, read_ecdsa_adaptor_signature })
});

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains a list of adaptor signature for a number of CET inputs.
pub struct CetAdaptorSignatures {
    /// The set of signatures.
    pub ecdsa_adaptor_signatures: Vec<CetAdaptorSignature>,
}

impl From<&[EcdsaAdaptorSignature]> for CetAdaptorSignatures {
    fn from(signatures: &[EcdsaAdaptorSignature]) -> Self {
        CetAdaptorSignatures {
            ecdsa_adaptor_signatures: signatures
                .iter()
                .map(|x| CetAdaptorSignature { signature: *x })
                .collect(),
        }
    }
}

impl From<&CetAdaptorSignatures> for Vec<EcdsaAdaptorSignature> {
    fn from(signatures: &CetAdaptorSignatures) -> Vec<EcdsaAdaptorSignature> {
        signatures
            .ecdsa_adaptor_signatures
            .iter()
            .map(|x| x.signature)
            .collect::<Vec<_>>()
    }
}

impl_dlc_writeable!(CetAdaptorSignatures, { (ecdsa_adaptor_signatures, vec) });

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains the witness elements to use to make a funding transaction input valid.
pub struct FundingSignature {
    /// The set of witness elements.
    pub witness_elements: Vec<WitnessElement>,
}

impl_dlc_writeable!(FundingSignature, { (witness_elements, vec) });

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains a list of witness elements to satisfy the spending conditions of
/// funding inputs.
pub struct FundingSignatures {
    /// The set of funding signatures.
    pub funding_signatures: Vec<FundingSignature>,
}

impl_dlc_writeable!(FundingSignatures, { (funding_signatures, vec) });

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains serialized data representing a single witness stack element.
pub struct WitnessElement {
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_string"
        )
    )]
    /// The serialized witness data.
    pub witness: Vec<u8>,
}

impl_dlc_writeable!(WitnessElement, { (witness, vec) });

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Fields used to negotiate contract information.
pub enum NegotiationFields {
    /// Negotiation for single event based contract.
    Single(SingleNegotiationFields),
    /// Negotiation for multiple event based contract.
    Disjoint(DisjointNegotiationFields),
}

impl_dlc_writeable_enum!(NegotiationFields, (0, Single), (1, Disjoint);;;);

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Negotiation fields for contract based on a single event.
pub struct SingleNegotiationFields {
    /// Proposed rounding intervals.
    rounding_intervals: contract_msgs::RoundingIntervals,
}

impl_dlc_writeable!(SingleNegotiationFields, { (rounding_intervals, writeable) });

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Negotiation fields for contract based on multiple events.
pub struct DisjointNegotiationFields {
    /// The negotiation fields for each contract event.
    negotiation_fields: Vec<NegotiationFields>,
}

impl_dlc_writeable!(DisjointNegotiationFields, { (negotiation_fields, vec) });

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
/// Contains information about a party wishing to enter into a DLC with
/// another party. The contained information is sufficient for any other party
/// to create a set of transactions representing the contract and its terms.
pub struct OfferDlc {
    /// The version of the protocol used by the peer.
    pub protocol_version: u32,
    /// Feature flags to be used for the offered contract.
    pub contract_flags: u8,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_array"
        )
    )]
    /// The identifier of the chain on which the contract will be settled.
    pub chain_hash: [u8; 32],
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_array"
        )
    )]
    /// Temporary contract id to identify the contract.
    pub temporary_contract_id: [u8; 32],
    /// Information about the contract event, payouts and oracles.
    pub contract_info: ContractInfo,
    /// The public key of the offerer to be used to lock the collateral.
    pub funding_pubkey: PublicKey,
    /// The SPK where the offerer will receive their payout.
    pub payout_spk: ScriptBuf,
    /// Serial id to order CET outputs.
    pub payout_serial_id: u64,
    /// Collateral of the offer party.
    pub offer_collateral: Amount,
    /// Inputs used by the offer party to fund the contract.
    pub funding_inputs: Vec<FundingInput>,
    /// The SPK where the offer party will receive their change.
    pub change_spk: ScriptBuf,
    /// Serial id to order funding transaction outputs.
    pub change_serial_id: u64,
    /// Serial id to order funding transaction outputs.
    pub fund_output_serial_id: u64,
    /// The fee rate to use to compute transaction fees for this contract.
    pub fee_rate_per_vb: u64,
    /// The lock time for the CETs.
    pub cet_locktime: u32,
    /// The lock time for the refund transactions.
    pub refund_locktime: u32,
    #[cfg_attr(
        feature = "use-serde",
        serde(default, skip_serializing_if = "TlvStream::is_empty")
    )]
    /// The TLV records appended after the fixed fields.
    ///
    /// Empty for a peer that appends none, in which case it encodes to no bytes and the
    /// message is byte-identical to one written before this field existed. Records this
    /// build has no type for are held verbatim rather than dropped; see
    /// [`tlv_stream`](crate::tlv_stream).
    pub tlvs: TlvStream,
}

impl OfferDlc {
    /// Returns the total collateral locked in the contract.
    pub fn get_total_collateral(&self) -> Amount {
        match &self.contract_info {
            ContractInfo::SingleContractInfo(single) => single.total_collateral,
            ContractInfo::DisjointContractInfo(disjoint) => disjoint.total_collateral,
        }
    }

    /// Returns whether the message satisfies validity requirements.
    pub fn validate<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        min_timeout_interval: u32,
        max_timeout_interval: u32,
    ) -> Result<(), Error> {
        match &self.contract_info {
            ContractInfo::SingleContractInfo(s) => s.contract_info.oracle_info.validate(secp)?,
            ContractInfo::DisjointContractInfo(d) => {
                if d.contract_infos.len() < 2 {
                    return Err(Error::InvalidArgument(
                        "Need at least two contract infos for disjoint contract".to_string(),
                    ));
                }

                for c in &d.contract_infos {
                    c.oracle_info.validate(secp)?;
                }
            }
        }

        // The CET locktime is pinned to the closest maturity date: a lower value
        // produces CETs spendable before the event matures, a higher value delays
        // execution past it.
        let closest_maturity_date = self.contract_info.get_closest_maturity_date();
        let valid_dates = self.cet_locktime == closest_maturity_date
            && closest_maturity_date + min_timeout_interval <= self.refund_locktime
            && self.refund_locktime <= closest_maturity_date + max_timeout_interval;
        if !valid_dates {
            return Err(Error::InvalidArgument(
                "CET locktime must equal the closest maturity date and the refund locktime must be within the timeout interval".to_string(),
            ));
        }

        Ok(())
    }
}

impl Writeable for OfferDlc {
    fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
        OFFER_TYPE.write(w)?;
        self.protocol_version.write(w)?;
        self.contract_flags.write(w)?;
        self.chain_hash.write(w)?;
        self.temporary_contract_id.write(w)?;
        self.contract_info.write(w)?;
        self.funding_pubkey.write(w)?;
        self.payout_spk.write(w)?;
        self.payout_serial_id.write(w)?;
        self.offer_collateral.write(w)?;
        crate::ser_impls::write_vec(&self.funding_inputs, w)?;
        self.change_spk.write(w)?;
        self.change_serial_id.write(w)?;
        self.fund_output_serial_id.write(w)?;
        self.fee_rate_per_vb.write(w)?;
        self.cet_locktime.write(w)?;
        self.refund_locktime.write(w)?;
        // Must stay last: the reader takes everything after this point as the stream.
        self.tlvs.write(w)?;
        Ok(())
    }
}

impl Readable for OfferDlc {
    fn read<R: lightning::io::Read>(r: &mut R) -> Result<Self, DecodeError> {
        let type_id: u16 = Readable::read(r)?;
        if type_id != OFFER_TYPE {
            return Err(DecodeError::UnknownRequiredFeature);
        }

        // Backward compatibility: detect old format (no protocol_version) vs new format.
        // New format: [protocol_version: 4 bytes][contract_flags: 1 byte][chain_hash: 32 bytes]
        // Old format: [contract_flags: 1 byte][chain_hash: 32 bytes]
        //
        // Read 5 bytes. Interpret first 4 as u32. If 1-10, it's protocol_version (new format).
        // In old format, these 4 bytes are contract_flags (0x00/0x01) + 3 chain_hash bytes,
        // which produces values far outside 1-10 for any Bitcoin network.
        let mut peek = [0u8; 5];
        r.read_exact(&mut peek)?;
        let possible_pv = u32::from_be_bytes([peek[0], peek[1], peek[2], peek[3]]);

        let (protocol_version, contract_flags, chain_hash) = if (1..=10).contains(&possible_pv) {
            // New format: peek[0..4] = protocol_version, peek[4] = contract_flags
            let chain_hash: [u8; 32] = Readable::read(r)?;
            (possible_pv, peek[4], chain_hash)
        } else {
            // Old format: peek[0] = contract_flags, peek[1..5] = first 4 chain_hash bytes
            let mut remaining = [0u8; 28];
            r.read_exact(&mut remaining)?;
            let mut chain_hash = [0u8; 32];
            chain_hash[..4].copy_from_slice(&peek[1..5]);
            chain_hash[4..].copy_from_slice(&remaining);
            (1u32, peek[0], chain_hash)
        };

        Ok(Self {
            protocol_version,
            contract_flags,
            chain_hash,
            temporary_contract_id: Readable::read(r)?,
            contract_info: Readable::read(r)?,
            funding_pubkey: Readable::read(r)?,
            payout_spk: Readable::read(r)?,
            payout_serial_id: Readable::read(r)?,
            offer_collateral: Readable::read(r)?,
            funding_inputs: crate::ser_impls::read_vec(r)?,
            change_spk: Readable::read(r)?,
            change_serial_id: Readable::read(r)?,
            fund_output_serial_id: Readable::read(r)?,
            fee_rate_per_vb: Readable::read(r)?,
            cet_locktime: Readable::read(r)?,
            refund_locktime: Readable::read(r)?,
            // Reads to the end of the message. Before this, trailing records were left
            // unread and silently dropped on the way back out.
            tlvs: TlvStream::read_to_end(r)?,
        })
    }
}

/// Contains information about a party wishing to accept a DLC offer. The contained
/// information is sufficient for the offering party to re-build the set of
/// transactions representing the contract and its terms, and guarantees the offering
/// party that they can safely provide signatures for their funding input.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct AcceptDlc {
    /// The version of the protocol used by the peer.
    pub protocol_version: u32,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_array"
        )
    )]
    /// The temporary contract id for the contract.
    pub temporary_contract_id: [u8; 32],
    /// The collateral input by the accept party.
    pub accept_collateral: Amount,
    /// The public key of the accept party to be used to lock the collateral.
    pub funding_pubkey: PublicKey,
    /// The SPK where the accept party will receive their payout.
    pub payout_spk: ScriptBuf,
    /// Serial id to order CET outputs.
    pub payout_serial_id: u64,
    /// Inputs used by the accept party to fund the contract.
    pub funding_inputs: Vec<FundingInput>,
    /// The SPK where the accept party will receive their change.
    pub change_spk: ScriptBuf,
    /// Serial id to order funding transaction outputs.
    pub change_serial_id: u64,
    /// The set of adaptor signatures from the accept party.
    pub cet_adaptor_signatures: CetAdaptorSignatures,
    /// The refund signature of the accept party.
    pub refund_signature: Signature,
    /// The negotiation fields from the accept party.
    pub negotiation_fields: Option<NegotiationFields>,
    #[cfg_attr(
        feature = "use-serde",
        serde(default, skip_serializing_if = "TlvStream::is_empty")
    )]
    /// The TLV records appended after the fixed fields.
    ///
    /// Empty for a peer that appends none, in which case it encodes to no bytes and the
    /// message is byte-identical to one written before this field existed. Records this
    /// build has no type for are held verbatim rather than dropped; see
    /// [`tlv_stream`](crate::tlv_stream).
    pub tlvs: TlvStream,
}

impl_dlc_writeable!(
    AcceptDlc,
    ACCEPT_TYPE,
    {
        (protocol_version, writeable),
        (temporary_contract_id, writeable),
        (accept_collateral, writeable),
        (funding_pubkey, writeable),
        (payout_spk, writeable),
        (payout_serial_id, writeable),
        (funding_inputs, vec),
        (change_spk, writeable),
        (change_serial_id, writeable),
        (cet_adaptor_signatures, writeable),
        (refund_signature, writeable),
        (negotiation_fields, option)
    },
    tlvs
);

/// Contains all the required signatures for the DLC transactions from the offering
/// party.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct SignDlc {
    /// The version of the protocol used by the peer.
    pub protocol_version: u32,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_array"
        )
    )]
    /// The id of the contract referred to by this message.
    pub contract_id: [u8; 32],
    /// The set of adaptor signatures from the offer party.
    pub cet_adaptor_signatures: CetAdaptorSignatures,
    /// The refund signature from the offer party.
    pub refund_signature: Signature,
    /// The set of funding signatures from the offer party.
    pub funding_signatures: FundingSignatures,
    #[cfg_attr(
        feature = "use-serde",
        serde(default, skip_serializing_if = "TlvStream::is_empty")
    )]
    /// The TLV records appended after the fixed fields.
    ///
    /// Empty for a peer that appends none, in which case it encodes to no bytes and the
    /// message is byte-identical to one written before this field existed. Records this
    /// build has no type for are held verbatim rather than dropped; see
    /// [`tlv_stream`](crate::tlv_stream).
    pub tlvs: TlvStream,
}

impl_dlc_writeable!(
    SignDlc,
    SIGN_TYPE,
    {
        (protocol_version, writeable),
        (contract_id, writeable),
        (cet_adaptor_signatures, writeable),
        (refund_signature, writeable),
        (funding_signatures, writeable)
    },
    tlvs
);

/// Contains information about a party wishing to close a DLC contract.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "use-serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "camelCase")
)]
pub struct CloseDlc {
    /// The version of the protocol used by the peer.
    pub protocol_version: u32,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "crate::serde_utils::serialize_hex",
            deserialize_with = "crate::serde_utils::deserialize_hex_array"
        )
    )]
    /// The id of the contract to close.
    pub contract_id: [u8; 32],
    /// The signature for the closing transaction.
    pub close_signature: Signature,
    /// The payout amount for the accept party in satoshis.
    pub accept_payout: Amount,
    /// The fee rate to use to compute transaction fees for this contract.
    pub fee_rate_per_vb: u64,
    /// Serial id for the funding input.
    pub fund_input_serial_id: u64,
    /// The funding inputs to use.
    pub funding_inputs: Vec<FundingInput>,
    /// The funding signatures.
    pub funding_signatures: FundingSignatures,
}

impl_dlc_writeable!(CloseDlc, CLOSE_TYPE, {
    (protocol_version, writeable),
    (contract_id, writeable),
    (close_signature, writeable),
    (accept_payout, writeable),
    (fee_rate_per_vb, writeable),
    (fund_input_serial_id, writeable),
    (funding_inputs, vec),
    (funding_signatures, writeable)
});

#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub enum Message {
    Offer(OfferDlc),
    Accept(AcceptDlc),
    Sign(SignDlc),
    Close(CloseDlc),
    OfferChannel(OfferChannel),
    AcceptChannel(AcceptChannel),
    SignChannel(SignChannel),
    SettleOffer(SettleOffer),
    SettleAccept(SettleAccept),
    SettleConfirm(SettleConfirm),
    SettleFinalize(SettleFinalize),
    RenewOffer(RenewOffer),
    RenewAccept(RenewAccept),
    RenewConfirm(RenewConfirm),
    RenewFinalize(RenewFinalize),
    RenewRevoke(RenewRevoke),
    CollaborativeCloseOffer(CollaborativeCloseOffer),
    Reject(Reject),
}

macro_rules! impl_type_writeable_for_enum {
    ($type_name: ident, {$($variant_name: ident),*}) => {
       impl Type for $type_name {
           fn type_id(&self) -> u16 {
               match self {
                   $($type_name::$variant_name(v) => v.type_id(),)*
               }
           }
       }

       impl Writeable for $type_name {
            fn write<W: Writer>(&self, writer: &mut W) -> Result<(), ::lightning::io::Error> {
                match self {
                   $($type_name::$variant_name(v) => v.write(writer),)*
                }
            }
       }
    };
}

impl_type_writeable_for_enum!(Message,
{
    Offer,
    Accept,
    Sign,
    Close,
    OfferChannel,
    AcceptChannel,
    SignChannel,
    SettleOffer,
    SettleAccept,
    SettleConfirm,
    SettleFinalize,
    RenewOffer,
    RenewAccept,
    RenewConfirm,
    RenewFinalize,
    RenewRevoke,
    CollaborativeCloseOffer,
    Reject
});

#[derive(Debug, Clone)]
/// Wrapper for DLC related message and segmentation related messages.
#[allow(clippy::large_enum_variant)]
pub enum WireMessage {
    /// Message related to establishment of a DLC contract.
    Message(Message),
    /// Message indicating an incoming segmented message.
    SegmentStart(SegmentStart),
    /// Message providing a chunk of a segmented message.
    SegmentChunk(SegmentChunk),
}

impl Display for WireMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Message(_) => "Message",
            Self::SegmentStart(_) => "SegmentStart",
            Self::SegmentChunk(_) => "SegmentChunk",
        };
        f.write_str(name)
    }
}

impl_type_writeable_for_enum!(WireMessage, { Message, SegmentStart, SegmentChunk });

#[cfg(test)]
mod tests {
    use secp256k1_zkp::SECP256K1;

    use super::*;

    macro_rules! roundtrip_test {
        ($type: ty, $input: ident) => {
            let msg: $type = serde_json::from_str(&$input).unwrap();
            test_roundtrip(msg);
        };
    }

    fn test_roundtrip<T: Writeable + Readable + PartialEq + std::fmt::Debug>(msg: T) {
        let mut buf = Vec::new();
        msg.write(&mut buf).expect("Error writing message");
        let mut cursor = lightning::io::Cursor::new(buf);
        let deser = Readable::read(&mut cursor).expect("Error reading message");
        assert_eq!(msg, deser);
    }

    #[test]
    fn offer_msg_roundtrip() {
        let input = include_str!("./test_inputs/offer_msg.json");
        roundtrip_test!(OfferDlc, input);
    }

    /// The offer fixture encoded by the last release that had no TLV stream, captured
    /// from that code rather than regenerated here.
    const OFFER_MSG_PRE_TLV: &str = include_str!("./test_inputs/offer_msg_pre_tlv.hex");

    fn offer_fixture() -> OfferDlc {
        serde_json::from_str(include_str!("./test_inputs/offer_msg.json")).unwrap()
    }

    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn read_offer(bytes: &[u8]) -> Result<OfferDlc, DecodeError> {
        Readable::read(&mut lightning::io::Cursor::new(bytes.to_vec()))
    }

    /// A record at type 65003 with a 3-byte body: the shape node-dlc appends and this
    /// crate used to drop.
    const UNKNOWN_RECORD: &[u8] = &[0xfd, 0xfd, 0xeb, 0x03, 0x01, 0x02, 0x03];

    #[test]
    fn offer_with_no_records_is_byte_identical_to_the_previous_release() {
        // An empty stream must contribute zero bytes, or every offer on the wire changes
        // and the temporary contract id derived from those bytes changes with it.
        let offer = offer_fixture();
        assert!(offer.tlvs.is_empty());
        assert_eq!(offer.encode(), from_hex(OFFER_MSG_PRE_TLV.trim()));
    }

    #[test]
    fn offer_from_the_previous_release_still_decodes() {
        let offer = read_offer(&from_hex(OFFER_MSG_PRE_TLV.trim())).unwrap();
        assert!(offer.tlvs.is_empty());
        assert_eq!(offer, offer_fixture());
    }

    #[test]
    fn unknown_record_survives_a_decode_encode_cycle() {
        // The regression this whole field exists for: before it, these 7 bytes were read
        // past, never stored, and never written back — with no error at any layer.
        let mut bytes = offer_fixture().encode();
        bytes.extend_from_slice(UNKNOWN_RECORD);

        let offer = read_offer(&bytes).unwrap();
        assert_eq!(offer.tlvs.raw().count(), 1);
        assert_eq!(offer.encode(), bytes);
    }

    /// A record defined the way an application defines one: the two macros, an odd type in
    /// the custom range, and nothing this crate knows about the contents.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct ApplicationRecord {
        reference: u64,
        label: String,
    }

    impl_dlc_writeable!(ApplicationRecord, {
        (reference, writeable),
        (label, string)
    });
    impl_dlc_tlv_record!(ApplicationRecord, 65007);

    fn application_record() -> ApplicationRecord {
        ApplicationRecord {
            reference: 42,
            label: "loan".to_string(),
        }
    }

    #[test]
    fn an_application_record_round_trips_on_an_offer() {
        let record = application_record();
        let mut offer = offer_fixture();
        offer.tlvs.set(&record);

        let decoded = read_offer(&offer.encode()).unwrap();
        assert_eq!(decoded, offer);
        assert_eq!(
            decoded.tlvs.get::<ApplicationRecord>().unwrap(),
            Some(record)
        );
    }

    #[test]
    fn an_application_record_round_trips_on_an_accept_and_a_sign() {
        let record = application_record();

        let mut accept = accept_fixture();
        accept.tlvs.set(&record);
        let decoded: AcceptDlc = read_msg(&accept.encode()).unwrap();
        assert_eq!(
            decoded.tlvs.get::<ApplicationRecord>().unwrap(),
            Some(record.clone())
        );

        let mut sign = sign_fixture();
        sign.tlvs.set(&record);
        let decoded: SignDlc = read_msg(&sign.encode()).unwrap();
        assert_eq!(
            decoded.tlvs.get::<ApplicationRecord>().unwrap(),
            Some(record)
        );
    }

    #[test]
    fn known_and_unknown_records_coexist_in_wire_order() {
        let mut offer = offer_fixture();
        offer.tlvs.set(&application_record());
        let mut bytes = offer.encode();
        bytes.extend_from_slice(UNKNOWN_RECORD);

        let decoded = read_offer(&bytes).unwrap();
        assert!(decoded.tlvs.get::<ApplicationRecord>().unwrap().is_some());
        assert_eq!(decoded.encode(), bytes);
    }

    #[test]
    fn old_format_offer_without_protocol_version_decodes_with_an_empty_stream() {
        // What a pre-`protocol_version` peer sends: the 4 version bytes are simply absent.
        // Reading to end must not turn that into an error, or peers valid today break.
        let bytes = offer_fixture().encode();
        let mut old_format = bytes[..2].to_vec();
        old_format.extend_from_slice(&bytes[6..]);

        let offer = read_offer(&old_format).unwrap();
        assert_eq!(offer.protocol_version, 1);
        assert!(offer.tlvs.is_empty());
        assert_eq!(offer, offer_fixture());
    }

    #[test]
    fn offer_with_a_truncated_record_is_rejected() {
        let mut bytes = offer_fixture().encode();
        bytes.extend_from_slice(&UNKNOWN_RECORD[..UNKNOWN_RECORD.len() - 1]);
        assert!(read_offer(&bytes).is_err());
    }

    /// The accept and sign fixtures encoded by the last release that had no TLV stream,
    /// captured from that code rather than regenerated here.
    const ACCEPT_MSG_PRE_TLV: &str = include_str!("./test_inputs/accept_msg_pre_tlv.hex");
    const SIGN_MSG_PRE_TLV: &str = include_str!("./test_inputs/sign_msg_pre_tlv.hex");

    fn accept_fixture() -> AcceptDlc {
        serde_json::from_str(include_str!("./test_inputs/accept_msg.json")).unwrap()
    }

    fn sign_fixture() -> SignDlc {
        serde_json::from_str(include_str!("./test_inputs/sign_msg.json")).unwrap()
    }

    fn read_msg<T: Readable>(bytes: &[u8]) -> Result<T, DecodeError> {
        Readable::read(&mut lightning::io::Cursor::new(bytes.to_vec()))
    }

    #[test]
    fn accept_with_no_records_is_byte_identical_to_the_previous_release() {
        let accept = accept_fixture();
        assert!(accept.tlvs.is_empty());
        assert_eq!(accept.encode(), from_hex(ACCEPT_MSG_PRE_TLV.trim()));
    }

    #[test]
    fn sign_with_no_records_is_byte_identical_to_the_previous_release() {
        let sign = sign_fixture();
        assert!(sign.tlvs.is_empty());
        assert_eq!(sign.encode(), from_hex(SIGN_MSG_PRE_TLV.trim()));
    }

    #[test]
    fn accept_from_the_previous_release_still_decodes() {
        let accept: AcceptDlc = read_msg(&from_hex(ACCEPT_MSG_PRE_TLV.trim())).unwrap();
        assert!(accept.tlvs.is_empty());
        assert_eq!(accept, accept_fixture());
    }

    #[test]
    fn sign_from_the_previous_release_still_decodes() {
        let sign: SignDlc = read_msg(&from_hex(SIGN_MSG_PRE_TLV.trim())).unwrap();
        assert!(sign.tlvs.is_empty());
        assert_eq!(sign, sign_fixture());
    }

    #[test]
    fn unknown_record_survives_a_decode_encode_cycle_on_an_accept() {
        // node-dlc keeps `unknownTlvs` on its accept too, so these bytes are what a real
        // peer expects back.
        let mut bytes = accept_fixture().encode();
        bytes.extend_from_slice(UNKNOWN_RECORD);

        let accept: AcceptDlc = read_msg(&bytes).unwrap();
        assert_eq!(accept.tlvs.raw().count(), 1);
        assert_eq!(accept.encode(), bytes);
    }

    #[test]
    fn unknown_record_survives_a_decode_encode_cycle_on_a_sign() {
        let mut bytes = sign_fixture().encode();
        bytes.extend_from_slice(UNKNOWN_RECORD);

        let sign: SignDlc = read_msg(&bytes).unwrap();
        assert_eq!(sign.tlvs.raw().count(), 1);
        assert_eq!(sign.encode(), bytes);
    }

    #[test]
    fn accept_with_a_truncated_record_is_rejected() {
        let mut bytes = accept_fixture().encode();
        bytes.extend_from_slice(&UNKNOWN_RECORD[..UNKNOWN_RECORD.len() - 1]);
        assert!(read_msg::<AcceptDlc>(&bytes).is_err());
    }

    #[test]
    fn sign_with_a_truncated_record_is_rejected() {
        let mut bytes = sign_fixture().encode();
        bytes.extend_from_slice(&UNKNOWN_RECORD[..UNKNOWN_RECORD.len() - 1]);
        assert!(read_msg::<SignDlc>(&bytes).is_err());
    }

    #[test]
    fn the_stream_is_the_last_thing_each_message_writes() {
        // The one invariant the macro's trailing-field position exists to hold. A field
        // added after the stream would be swallowed by it on the way back in, so assert
        // the record bytes really are the message suffix rather than trusting placement.
        let mut offer = offer_fixture();
        offer.tlvs = read_msg::<AcceptDlc>(&{
            let mut b = accept_fixture().encode();
            b.extend_from_slice(UNKNOWN_RECORD);
            b
        })
        .unwrap()
        .tlvs;
        assert!(offer.encode().ends_with(UNKNOWN_RECORD));

        let mut accept = accept_fixture();
        accept.tlvs = offer.tlvs.clone();
        assert!(accept.encode().ends_with(UNKNOWN_RECORD));

        let mut sign = sign_fixture();
        sign.tlvs = offer.tlvs.clone();
        assert!(sign.encode().ends_with(UNKNOWN_RECORD));
    }

    #[test]
    fn accept_msg_roundtrip() {
        let input = include_str!("./test_inputs/accept_msg.json");
        roundtrip_test!(AcceptDlc, input);
    }

    #[test]
    fn sign_msg_roundtrip() {
        let input = include_str!("./test_inputs/sign_msg.json");
        roundtrip_test!(SignDlc, input);
    }

    #[test]
    fn close_msg_roundtrip() {
        let input = include_str!("./test_inputs/close_msg.json");
        roundtrip_test!(CloseDlc, input);
    }

    #[test]
    fn valid_offer_message_passes_validation() {
        let input = include_str!("./test_inputs/offer_msg.json");
        let valid_offer: OfferDlc = serde_json::from_str(input).unwrap();
        valid_offer
            .validate(SECP256K1, 86400 * 7, 86400 * 14)
            .expect("to validate valid offer messages.");
    }

    #[test]
    fn valid_offer_message_passes_with_dlc_input() {
        let input = include_str!("./test_inputs/offer_msg_with_dlc_input.json");
        let valid_offer: OfferDlc = serde_json::from_str(input).unwrap();

        for input in &valid_offer.funding_inputs {
            assert!(input.dlc_input.is_some());
        }
        valid_offer
            .validate(SECP256K1, 86400 * 7, 86400 * 14)
            .expect("to validate valid offer messages.");
    }

    #[test]
    fn invalid_offer_messages_fail_validation() {
        let input = include_str!("./test_inputs/offer_msg.json");
        let offer: OfferDlc = serde_json::from_str(input).unwrap();

        let mut invalid_maturity = offer.clone();
        invalid_maturity.cet_locktime += 3;

        let mut premature_cet_locktime = offer.clone();
        premature_cet_locktime.cet_locktime -= 3;

        let mut zero_cet_locktime = offer.clone();
        zero_cet_locktime.cet_locktime = 0;

        let mut too_short_timeout = offer.clone();
        too_short_timeout.refund_locktime -= 100;

        let mut too_long_timeout = offer;
        too_long_timeout.refund_locktime -= 100;

        for invalid in &[
            invalid_maturity,
            premature_cet_locktime,
            zero_cet_locktime,
            too_short_timeout,
            too_long_timeout,
        ] {
            invalid
                .validate(SECP256K1, 86400 * 7, 86400 * 14)
                .expect_err("Should not pass validation of invalid offer message.");
        }
    }

    #[test]
    fn disjoint_contract_offer_messages_fail_validation() {
        let input = include_str!("./test_inputs/offer_msg_disjoint.json");
        let offer: OfferDlc = serde_json::from_str(input).unwrap();

        let mut no_contract_input = offer.clone();
        no_contract_input.contract_info =
            ContractInfo::DisjointContractInfo(contract_msgs::DisjointContractInfo {
                total_collateral: Amount::ONE_BTC,
                contract_infos: vec![],
            });

        let mut single_contract_input = offer.clone();
        single_contract_input.contract_info =
            if let ContractInfo::DisjointContractInfo(d) = offer.contract_info {
                let mut single = d;
                single.contract_infos.remove(1);
                ContractInfo::DisjointContractInfo(single)
            } else {
                panic!("Expected disjoint contract info.");
            };

        for invalid in &[no_contract_input, single_contract_input] {
            invalid
                .validate(SECP256K1, 86400 * 7, 86400 * 14)
                .expect_err("Should not pass validation of invalid offer message.");
        }
    }
}
