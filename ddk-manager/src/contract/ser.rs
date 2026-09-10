//! Serialization trait implementations for various data structures enabling them
//! to be converted to byte arrays.

use crate::contract::accepted_contract::AcceptedContract;
use crate::contract::contract_info::ContractInfo;
use crate::contract::enum_descriptor::EnumDescriptor;
use crate::contract::numerical_descriptor::{DifferenceParams, NumericalDescriptor};
use crate::contract::offered_contract::OfferedContract;
use crate::contract::signed_contract::SignedContract;
use crate::contract::AdaptorInfo;
use crate::contract::{
    ClosedContract, Contract, ContractDescriptor, FailedAcceptContract, FailedSignContract,
    PreClosedContract,
};
use crate::error::Error;
use crate::payout_curve::{
    HyperbolaPayoutCurvePiece, PayoutFunction, PayoutFunctionPiece, PayoutPoint,
    PolynomialPayoutCurvePiece, RoundingInterval, RoundingIntervals,
};
use crate::KeysId;
use bitcoin::Amount;
use ddk_dlc::DlcTransactions;
use ddk_messages::impl_dlc_writeable;
use ddk_messages::ser_impls::{
    read_ecdsa_adaptor_signatures, read_option_cb, read_usize, read_vec, read_vec_cb,
    write_ecdsa_adaptor_signatures, write_option_cb, write_usize, write_vec, write_vec_cb,
};
use ddk_messages::tlv_stream::TlvStream;
use ddk_messages::{AcceptDlc, SignDlc};
use ddk_trie::digit_trie::{DigitNodeData, DigitTrieDump};
use ddk_trie::multi_oracle_trie::{MultiOracleTrie, MultiOracleTrieDump};
use ddk_trie::multi_oracle_trie_with_diff::{MultiOracleTrieWithDiff, MultiOracleTrieWithDiffDump};
use ddk_trie::multi_trie::{MultiTrieDump, MultiTrieNodeData, TrieNodeInfo};
use ddk_trie::{OracleNumericInfo, RangeInfo};
use lightning::io::Read;
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{BigSize, FixedLengthReader, Readable, Writeable, Writer};

/// Trait used to de/serialize an object to/from a vector of bytes.
pub trait Serializable
where
    Self: Sized,
{
    /// Serialize the object.
    fn serialize(&self) -> Result<Vec<u8>, lightning::io::Error>;
    /// Deserialize the object.
    fn deserialize<R: Read>(reader: &mut R) -> Result<Self, DecodeError>;
}

impl<T> Serializable for T
where
    T: Writeable + Readable,
{
    fn serialize(&self) -> Result<Vec<u8>, lightning::io::Error> {
        let mut buffer = Vec::new();
        self.write(&mut buffer)?;
        Ok(buffer)
    }

    fn deserialize<R: Read>(reader: &mut R) -> Result<Self, DecodeError> {
        Readable::read(reader)
    }
}

impl_dlc_writeable!(PayoutPoint, { (event_outcome, writeable), (outcome_payout, writeable), (extra_precision, writeable) });
impl_dlc_writeable_enum!(
    PayoutFunctionPiece,
    (0, PolynomialPayoutCurvePiece),
    (1, HyperbolaPayoutCurvePiece);;;
);
impl_dlc_writeable!(RoundingInterval, { (begin_interval, writeable), (rounding_mod, writeable) });
impl_dlc_writeable!(PayoutFunction, { (payout_function_pieces, vec) });
impl_dlc_writeable!(NumericalDescriptor, { (payout_function, writeable), (rounding_intervals, writeable), (difference_params, option), (oracle_numeric_infos, {cb_writeable, oracle_params::write, oracle_params::read}) });
impl_dlc_writeable!(PolynomialPayoutCurvePiece, { (payout_points, vec) });
impl_dlc_writeable!(RoundingIntervals, { (intervals, vec) });
impl_dlc_writeable!(DifferenceParams, { (max_error_exp, usize), (min_support_exp, usize), (maximize_coverage, writeable) });
impl_dlc_writeable!(HyperbolaPayoutCurvePiece, {
    (left_end_point, writeable),
    (right_end_point, writeable),
    (use_positive_piece, writeable),
    (translate_outcome, float),
    (translate_payout, float),
    (a, float),
    (b, float),
    (c, float),
    (d, float)
});
impl_dlc_writeable_enum!(ContractDescriptor, (0, Enum), (1, Numerical);;;);
impl_dlc_writeable!(ContractInfo, { (contract_descriptor, writeable), (oracle_announcements, vec), (threshold, usize)});
impl_dlc_writeable!(EnumDescriptor, {
    (
        outcome_payouts,
        {vec_cb, ddk_messages::ser_impls::enum_payout::write, ddk_messages::ser_impls::enum_payout::read}
    )
});
impl Writeable for OfferedContract {
    fn write<W: Writer>(&self, w: &mut W) -> Result<(), lightning::io::Error> {
        self.id.write(w)?;
        self.is_offer_party.write(w)?;
        write_vec(&self.contract_info, w)?;
        ddk_messages::ser_impls::party_params::write(&self.offer_params, w)?;
        self.total_collateral.write(w)?;
        write_vec(&self.funding_inputs, w)?;
        self.fund_output_serial_id.write(w)?;
        self.fee_rate_per_vb.write(w)?;
        self.cet_locktime.write(w)?;
        self.refund_locktime.write(w)?;
        self.contract_flags.write(w)?;
        // Written only when present, so contracts stored before ddk tracked
        // the chain hash keep re-serializing to their original bytes.
        if let Some(chain_hash) = self.chain_hash {
            chain_hash.write(w)?;
        }
        self.counter_party.write(w)?;
        self.keys_id.write(w)?;
        Ok(())
    }
}

impl Readable for OfferedContract {
    fn read<R: Read>(r: &mut R) -> Result<Self, DecodeError> {
        let id: [u8; 32] = Readable::read(r)?;
        let is_offer_party: bool = Readable::read(r)?;
        let contract_info = read_vec(r)?;
        let offer_params = ddk_messages::ser_impls::party_params::read(r)?;
        let total_collateral: Amount = Readable::read(r)?;
        let funding_inputs = read_vec(r)?;
        let fund_output_serial_id: u64 = Readable::read(r)?;
        let fee_rate_per_vb: u64 = Readable::read(r)?;
        let cet_locktime: u32 = Readable::read(r)?;
        let refund_locktime: u32 = Readable::read(r)?;

        // Backward compatibility: contract_flags (u8) and later chain_hash
        // ([u8; 32]) were inserted between refund_locktime and counter_party,
        // and either may be absent in a stored contract. A compressed pubkey
        // starts with 0x02/0x03, while contract_flags is 0x00/0x01 and no
        // supported network's chain hash starts with those bytes (see the
        // chain_hash_first_byte_is_not_a_pubkey_prefix test), so peeking one
        // byte tells the formats apart at each step.
        let mut peek = [0u8; 1];
        r.read_exact(&mut peek)?;
        let read_pubkey_from_first_byte =
            |first: u8, r: &mut R| -> Result<secp256k1_zkp::PublicKey, DecodeError> {
                let mut pubkey_bytes = [0u8; 33];
                pubkey_bytes[0] = first;
                r.read_exact(&mut pubkey_bytes[1..])?;
                secp256k1_zkp::PublicKey::from_slice(&pubkey_bytes)
                    .map_err(|_| DecodeError::InvalidValue)
            };
        let (contract_flags, chain_hash, counter_party) = if peek[0] == 0x02 || peek[0] == 0x03 {
            // Stored without contract_flags or chain_hash: this byte starts
            // the counter_party pubkey.
            let pk = read_pubkey_from_first_byte(peek[0], r)?;
            (0u8, None, pk)
        } else {
            // This byte is contract_flags; peek again to tell whether
            // chain_hash follows or counter_party starts directly.
            let contract_flags = peek[0];
            r.read_exact(&mut peek)?;
            if peek[0] == 0x02 || peek[0] == 0x03 {
                // Stored with contract_flags but without chain_hash.
                let pk = read_pubkey_from_first_byte(peek[0], r)?;
                (contract_flags, None, pk)
            } else {
                let mut chain_hash = [0u8; 32];
                chain_hash[0] = peek[0];
                r.read_exact(&mut chain_hash[1..])?;
                let counter_party: secp256k1_zkp::PublicKey = Readable::read(r)?;
                (contract_flags, Some(chain_hash), counter_party)
            }
        };

        let keys_id: KeysId = Readable::read(r)?;

        Ok(Self {
            id,
            is_offer_party,
            contract_info,
            offer_params,
            total_collateral,
            funding_inputs,
            fund_output_serial_id,
            fee_rate_per_vb,
            cet_locktime,
            refund_locktime,
            contract_flags,
            chain_hash,
            counter_party,
            keys_id,
            // Filled by [`Contract::deserialize`], not from these bytes.
            tlvs: Default::default(),
        })
    }
}
impl_dlc_writeable_external!(RangeInfo, range_info, { (cet_index, usize), (adaptor_index, usize)});
impl_dlc_writeable_enum!(AdaptorInfo,;; (0, Numerical, write_multi_oracle_trie, read_multi_oracle_trie), (1, NumericalWithDifference, write_multi_oracle_trie_with_diff, read_multi_oracle_trie_with_diff); (2, Enum));
impl_dlc_writeable_external!(
    DlcTransactions, dlc_transactions,
    { (fund, writeable),
    (cets, vec),
    (refund, writeable),
    (funding_witness_script, writeable),
    (pending_close_txs, vec)}
);

impl_dlc_writeable!(AcceptedContract, {
    (offered_contract, writeable),
    (accept_params, { cb_writeable, ddk_messages::ser_impls::party_params::write, ddk_messages::ser_impls::party_params::read }),
    (funding_inputs, vec),
    (adaptor_infos, vec),
    (adaptor_signatures, { cb_writeable, write_ecdsa_adaptor_signatures, read_ecdsa_adaptor_signatures }),
    (accept_refund_signature, writeable),
    (dlc_transactions, {cb_writeable, dlc_transactions::write, dlc_transactions::read }),
    // The streams are stored by [`Contract::serialize`], not in the struct bytes.
    (tlvs, default)
});
impl_dlc_writeable!(SignedContract, {
    (accepted_contract, writeable),
    (adaptor_signatures, { cb_writeable, write_ecdsa_adaptor_signatures, read_ecdsa_adaptor_signatures }),
    (offer_refund_signature, writeable),
    (funding_signatures, writeable),
    (channel_id, option),
    (tlvs, default)
});
impl_dlc_writeable!(PreClosedContract, {
    (signed_contract, writeable),
    (attestations, {option_cb, write_vec, read_vec}),
    (signed_cet, writeable)
});
impl_dlc_writeable!(ClosedContract, {
    (attestations, {option_cb, write_vec, read_vec}),
    (signed_cet, writeable),
    (contract_id, writeable),
    (temporary_contract_id, writeable),
    (counter_party_id, writeable),
    (funding_txid, writeable),
    (pnl, SignedAmount),
    (signed_contract, writeable)
});

/// Marker for a length-framed message. Old data wrote the message raw, so it
/// starts with the message's u16 wire type, which is never zero.
const FRAMED_MESSAGE_MARKER: u16 = 0;

/// Writes a message with a marker and its byte length. These messages read
/// their TLV stream to the end of the buffer, so without the frame the read
/// would swallow the fields stored after the message.
fn write_framed_message<T: Writeable + Readable, W: Writer>(
    msg: &T,
    w: &mut W,
) -> Result<(), lightning::io::Error> {
    FRAMED_MESSAGE_MARKER.write(w)?;
    let bytes = msg.serialize()?;
    BigSize(bytes.len() as u64).write(w)?;
    w.write_all(&bytes)
}

fn read_framed_message<R: Read, T: Readable>(
    r: &mut R,
    v1_type: u16,
    read_v1_body: fn(&mut R) -> Result<T, DecodeError>,
) -> Result<T, DecodeError> {
    let marker: u16 = Readable::read(r)?;
    if marker == FRAMED_MESSAGE_MARKER {
        let len: BigSize = Readable::read(r)?;
        let mut frame = FixedLengthReader::new(&mut *r, len.0);
        let msg: T = Readable::read(&mut frame)?;
        if frame.bytes_remain() {
            return Err(DecodeError::InvalidValue);
        }
        Ok(msg)
    } else if marker == v1_type {
        // Old unframed data. The two bytes just read are the message type,
        // and the body follows with no TLV stream.
        read_v1_body(r)
    } else {
        Err(DecodeError::InvalidValue)
    }
}

fn read_framed_accept<R: Read>(r: &mut R) -> Result<AcceptDlc, DecodeError> {
    read_framed_message(
        r,
        ddk_messages::types::ACCEPT_TYPE,
        AcceptDlc::read_body_without_tlv_stream,
    )
}

fn read_framed_sign<R: Read>(r: &mut R) -> Result<SignDlc, DecodeError> {
    read_framed_message(
        r,
        ddk_messages::types::SIGN_TYPE,
        SignDlc::read_body_without_tlv_stream,
    )
}

impl_dlc_writeable!(FailedAcceptContract, {(offered_contract, writeable), (accept_message, {cb_writeable, write_framed_message, read_framed_accept}), (error_message, string)});
impl_dlc_writeable!(FailedSignContract, {(accepted_contract, writeable), (sign_message, {cb_writeable, write_framed_message, read_framed_sign}), (error_message, string)});

/// Marker for a versioned contract blob. Old blobs start with the state
/// prefix, which is never zero.
const STORED_CONTRACT_MARKER: u8 = 0;

/// Version of the blob layout written after the marker.
const STORED_CONTRACT_VERSION: u8 = 1;

/// State prefix of a stored [`Contract`].
#[derive(Debug)]
pub enum ContractPrefix {
    /// See [`Contract::Offered`].
    Offered = 1,
    /// See [`Contract::Accepted`].
    Accepted,
    /// See [`Contract::Signed`].
    Signed,
    /// See [`Contract::Confirmed`].
    Confirmed,
    /// See [`Contract::PreClosed`].
    PreClosed,
    /// See [`Contract::Closed`].
    Closed,
    /// See [`Contract::FailedAccept`].
    FailedAccept,
    /// See [`Contract::FailedSign`].
    FailedSign,
    /// See [`Contract::Refunded`].
    Refunded,
    /// See [`Contract::Rejected`].
    Rejected,
}

impl From<ContractPrefix> for u8 {
    fn from(prefix: ContractPrefix) -> u8 {
        prefix as u8
    }
}

impl std::convert::TryFrom<u8> for ContractPrefix {
    type Error = Error;

    fn try_from(v: u8) -> Result<Self, Error> {
        match v {
            1 => Ok(ContractPrefix::Offered),
            2 => Ok(ContractPrefix::Accepted),
            3 => Ok(ContractPrefix::Signed),
            4 => Ok(ContractPrefix::Confirmed),
            5 => Ok(ContractPrefix::PreClosed),
            6 => Ok(ContractPrefix::Closed),
            7 => Ok(ContractPrefix::FailedAccept),
            8 => Ok(ContractPrefix::FailedSign),
            9 => Ok(ContractPrefix::Refunded),
            10 => Ok(ContractPrefix::Rejected),
            _ => Err(Error::StorageError("Unknown prefix".to_string())),
        }
    }
}

impl From<String> for ContractPrefix {
    fn from(s: String) -> Self {
        match s.as_str() {
            "offered" => ContractPrefix::Offered,
            "accepted" => ContractPrefix::Accepted,
            "signed" => ContractPrefix::Signed,
            "confirmed" => ContractPrefix::Confirmed,
            "pre-closed" => ContractPrefix::PreClosed,
            "closed" => ContractPrefix::Closed,
            "failed-accept" => ContractPrefix::FailedAccept,
            "failed-sign" => ContractPrefix::FailedSign,
            "refunded" => ContractPrefix::Refunded,
            "rejected" => ContractPrefix::Rejected,
            _ => ContractPrefix::Offered,
        }
    }
}

impl ContractPrefix {
    /// The prefix byte for the contract's state.
    pub fn get_prefix(input: &Contract) -> u8 {
        let prefix = match input {
            Contract::Offered(_) => ContractPrefix::Offered,
            Contract::Accepted(_) => ContractPrefix::Accepted,
            Contract::Signed(_) => ContractPrefix::Signed,
            Contract::Confirmed(_) => ContractPrefix::Confirmed,
            Contract::PreClosed(_) => ContractPrefix::PreClosed,
            Contract::Closed(_) => ContractPrefix::Closed,
            Contract::FailedAccept(_) => ContractPrefix::FailedAccept,
            Contract::FailedSign(_) => ContractPrefix::FailedSign,
            Contract::Refunded(_) => ContractPrefix::Refunded,
            Contract::Rejected(_) => ContractPrefix::Rejected,
        };
        prefix as u8
    }

    /// The state prefix byte of a stored contract blob, without decoding it.
    /// Lets a store filter by state before paying for [`Contract::deserialize`].
    pub fn peek(buff: &[u8]) -> Result<u8, Error> {
        let position = match buff.first() {
            Some(&STORED_CONTRACT_MARKER) => 2,
            Some(_) => 0,
            None => return Err(Error::StorageError("empty contract blob".to_string())),
        };
        buff.get(position)
            .copied()
            .ok_or_else(|| Error::StorageError("contract blob too short".to_string()))
    }
}

// Each struct owns its TLV streams and delegates to the struct it nests, so a
// state that wraps another cannot drop the wrapped streams.
impl OfferedContract {
    fn tlv_streams(&self) -> Vec<&TlvStream> {
        vec![&self.tlvs]
    }

    fn tlv_streams_mut(&mut self) -> Vec<&mut TlvStream> {
        vec![&mut self.tlvs]
    }
}

impl AcceptedContract {
    fn tlv_streams(&self) -> Vec<&TlvStream> {
        let mut streams = self.offered_contract.tlv_streams();
        streams.push(&self.tlvs);
        streams
    }

    fn tlv_streams_mut(&mut self) -> Vec<&mut TlvStream> {
        let mut streams = self.offered_contract.tlv_streams_mut();
        streams.push(&mut self.tlvs);
        streams
    }
}

impl SignedContract {
    fn tlv_streams(&self) -> Vec<&TlvStream> {
        let mut streams = self.accepted_contract.tlv_streams();
        streams.push(&self.tlvs);
        streams
    }

    fn tlv_streams_mut(&mut self) -> Vec<&mut TlvStream> {
        let mut streams = self.accepted_contract.tlv_streams_mut();
        streams.push(&mut self.tlvs);
        streams
    }
}

impl Contract {
    fn tlv_streams(&self) -> Vec<&TlvStream> {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.tlv_streams(),
            Contract::Accepted(a) => a.tlv_streams(),
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => s.tlv_streams(),
            Contract::PreClosed(p) => p.signed_contract.tlv_streams(),
            Contract::Closed(c) => c.signed_contract.tlv_streams(),
            Contract::FailedAccept(f) => f.offered_contract.tlv_streams(),
            Contract::FailedSign(f) => f.accepted_contract.tlv_streams(),
        }
    }

    fn tlv_streams_mut(&mut self) -> Vec<&mut TlvStream> {
        match self {
            Contract::Offered(o) | Contract::Rejected(o) => o.tlv_streams_mut(),
            Contract::Accepted(a) => a.tlv_streams_mut(),
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => {
                s.tlv_streams_mut()
            }
            Contract::PreClosed(p) => p.signed_contract.tlv_streams_mut(),
            Contract::Closed(c) => c.signed_contract.tlv_streams_mut(),
            Contract::FailedAccept(f) => f.offered_contract.tlv_streams_mut(),
            Contract::FailedSign(f) => f.accepted_contract.tlv_streams_mut(),
        }
    }

    /// Serializes the contract for storage. The blob is the marker and
    /// version byte, the state prefix, the struct bytes, and one
    /// length-framed TLV stream per message the contract stands for.
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let struct_bytes = match self {
            Contract::Offered(o) | Contract::Rejected(o) => Serializable::serialize(o),
            Contract::Accepted(a) => Serializable::serialize(a),
            Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => {
                Serializable::serialize(s)
            }
            Contract::PreClosed(p) => Serializable::serialize(p),
            Contract::Closed(c) => Serializable::serialize(c),
            Contract::FailedAccept(f) => Serializable::serialize(f),
            Contract::FailedSign(f) => Serializable::serialize(f),
        }
        .map_err(to_storage_error)?;
        let mut res = vec![
            STORED_CONTRACT_MARKER,
            STORED_CONTRACT_VERSION,
            ContractPrefix::get_prefix(self),
        ];
        res.extend_from_slice(&struct_bytes);
        for stream in self.tlv_streams() {
            let bytes = stream.encode();
            BigSize(bytes.len() as u64)
                .write(&mut res)
                .map_err(to_storage_error)?;
            res.extend_from_slice(&bytes);
        }
        Ok(res)
    }

    /// Deserializes a stored contract. Blobs written before the marker
    /// existed start with the state prefix and carry no streams.
    pub fn deserialize(buff: &[u8]) -> Result<Contract, Error> {
        let mut cursor = lightning::io::Cursor::new(buff);
        let mut first = [0u8; 1];
        cursor.read_exact(&mut first)?;
        if first[0] != STORED_CONTRACT_MARKER {
            return Self::read_struct(first[0], &mut cursor);
        }
        let mut version = [0u8; 1];
        cursor.read_exact(&mut version)?;
        if version[0] != STORED_CONTRACT_VERSION {
            return Err(Error::StorageError(format!(
                "unknown stored contract version {}",
                version[0]
            )));
        }
        let mut prefix = [0u8; 1];
        cursor.read_exact(&mut prefix)?;
        let mut contract = Self::read_struct(prefix[0], &mut cursor)?;
        for stream in contract.tlv_streams_mut() {
            let len: BigSize = Readable::read(&mut cursor).map_err(to_storage_error)?;
            let mut frame = FixedLengthReader::new(&mut cursor, len.0);
            *stream = ddk_messages::tlv_stream::TlvStream::read_to_end(&mut frame)
                .map_err(to_storage_error)?;
        }
        Ok(contract)
    }

    fn read_struct<R: Read>(prefix: u8, r: &mut R) -> Result<Contract, Error> {
        let prefix: ContractPrefix = prefix.try_into()?;
        Ok(match prefix {
            ContractPrefix::Offered => {
                Contract::Offered(OfferedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Accepted => {
                Contract::Accepted(AcceptedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Signed => {
                Contract::Signed(SignedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Confirmed => {
                Contract::Confirmed(SignedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::PreClosed => {
                Contract::PreClosed(PreClosedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Closed => {
                Contract::Closed(ClosedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::FailedAccept => Contract::FailedAccept(
                FailedAcceptContract::deserialize(r).map_err(to_storage_error)?,
            ),
            ContractPrefix::FailedSign => {
                Contract::FailedSign(FailedSignContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Refunded => {
                Contract::Refunded(SignedContract::deserialize(r).map_err(to_storage_error)?)
            }
            ContractPrefix::Rejected => {
                Contract::Rejected(OfferedContract::deserialize(r).map_err(to_storage_error)?)
            }
        })
    }
}

fn to_storage_error<E: std::fmt::Debug>(e: E) -> Error {
    Error::StorageError(format!("{e:?}"))
}

impl_dlc_writeable_external!(DigitTrieDump<Vec<RangeInfo> >, digit_trie_dump_vec_range, { (node_data, {vec_cb, write_digit_node_data_vec_range, read_digit_node_data_vec_range}), (root, {option_cb, write_usize, read_usize}), (base, usize)});
impl_dlc_writeable_external!(DigitTrieDump<RangeInfo>, digit_trie_dump_range, { (node_data, {vec_cb, write_digit_node_data_range, read_digit_node_data_range}), (root, {option_cb, write_usize, read_usize}), (base, usize)});
impl_dlc_writeable_external!(DigitTrieDump<Vec<TrieNodeInfo> >, digit_trie_dump_trie, { (node_data, {vec_cb, write_digit_node_data_trie, read_digit_node_data_trie}), (root, {option_cb, write_usize, read_usize}), (base, usize)});
impl_dlc_writeable_external!(MultiOracleTrieDump, multi_oracle_trie_dump, { (digit_trie_dump, {cb_writeable, digit_trie_dump_vec_range::write, digit_trie_dump_vec_range::read}), (threshold, usize), (oracle_numeric_infos, {cb_writeable, oracle_params::write, oracle_params::read}), (extra_cover_trie_dump, {option_cb, multi_trie_dump::write, multi_trie_dump::read}) });
impl_dlc_writeable_external!(OracleNumericInfo, oracle_params, { (base, usize), (nb_digits, {vec_cb, write_usize, read_usize}) });
impl_dlc_writeable_external_enum!(
    MultiTrieNodeData<RangeInfo>,
    multi_trie_node_data,
    (0, Leaf, digit_trie_dump_range),
    (1, Node, digit_trie_dump_trie)
);
impl_dlc_writeable_external!(MultiTrieDump<RangeInfo>, multi_trie_dump, { (node_data, {vec_cb, multi_trie_node_data::write, multi_trie_node_data::read}), (nb_tries, usize), (nb_required, usize), (min_support_exp, usize), (max_error_exp, usize), (maximize_coverage, writeable), (oracle_numeric_infos, {cb_writeable, oracle_params::write, oracle_params::read}) });
impl_dlc_writeable_external!(MultiOracleTrieWithDiffDump, multi_oracle_trie_with_diff_dump, { (multi_trie_dump, {cb_writeable, multi_trie_dump::write, multi_trie_dump::read}), (oracle_numeric_infos, {cb_writeable, oracle_params::write, oracle_params::read}) });
impl_dlc_writeable_external!(TrieNodeInfo, trie_node_info, { (trie_index, usize), (store_index, usize) });

fn write_digit_node_data_trie<W: Writer>(
    input: &DigitNodeData<Vec<TrieNodeInfo>>,
    writer: &mut W,
) -> Result<(), lightning::io::Error> {
    let cb = |x: &Vec<TrieNodeInfo>, writer: &mut W| -> Result<(), lightning::io::Error> {
        write_vec_cb(x, writer, &trie_node_info::write)
    };
    write_digit_node_data(input, writer, &cb)
}

fn read_digit_node_data_trie<R: Read>(
    reader: &mut R,
) -> Result<DigitNodeData<Vec<TrieNodeInfo>>, DecodeError> {
    let cb = |reader: &mut R| -> Result<Vec<TrieNodeInfo>, DecodeError> {
        read_vec_cb(reader, &trie_node_info::read)
    };
    read_digit_node_data(reader, &cb)
}

fn write_digit_node_data_range<W: Writer>(
    input: &DigitNodeData<RangeInfo>,
    writer: &mut W,
) -> Result<(), lightning::io::Error> {
    write_digit_node_data(input, writer, &range_info::write)
}

fn read_digit_node_data_range<R: Read>(
    reader: &mut R,
) -> Result<DigitNodeData<RangeInfo>, DecodeError> {
    read_digit_node_data(reader, &range_info::read)
}

fn write_digit_node_data_vec_range<W: Writer>(
    input: &DigitNodeData<Vec<RangeInfo>>,
    writer: &mut W,
) -> Result<(), lightning::io::Error> {
    let cb = |x: &Vec<RangeInfo>, writer: &mut W| -> Result<(), lightning::io::Error> {
        write_vec_cb(x, writer, &range_info::write)
    };
    write_digit_node_data(input, writer, &cb)
}

fn read_digit_node_data_vec_range<R: Read>(
    reader: &mut R,
) -> Result<DigitNodeData<Vec<RangeInfo>>, DecodeError> {
    let cb = |reader: &mut R| -> Result<Vec<RangeInfo>, DecodeError> {
        read_vec_cb(reader, &range_info::read)
    };
    read_digit_node_data(reader, &cb)
}

fn write_digit_node_data<W: Writer, T, F>(
    input: &DigitNodeData<T>,
    writer: &mut W,
    cb: &F,
) -> Result<(), lightning::io::Error>
where
    F: Fn(&T, &mut W) -> Result<(), lightning::io::Error>,
{
    write_option_cb(&input.data, writer, &cb)?;
    write_vec_cb(&input.prefix, writer, &write_usize)?;
    let cb = |x: &Vec<Option<usize>>, writer: &mut W| -> Result<(), lightning::io::Error> {
        let cb = |y: &Option<usize>, writer: &mut W| -> Result<(), lightning::io::Error> {
            write_option_cb(y, writer, &write_usize)
        };
        write_vec_cb(x, writer, &cb)
    };
    write_option_cb(&input.children, writer, &cb)
}

fn read_digit_node_data<R: Read, T, F>(
    reader: &mut R,
    cb: &F,
) -> Result<DigitNodeData<T>, DecodeError>
where
    F: Fn(&mut R) -> Result<T, DecodeError>,
{
    let cb1 = |reader: &mut R| -> Result<T, DecodeError> { cb(reader) };
    let cb = |reader: &mut R| -> Result<Vec<Option<usize>>, DecodeError> {
        let cb = |reader: &mut R| -> Result<Option<usize>, DecodeError> {
            read_option_cb(reader, &read_usize)
        };
        read_vec_cb(reader, &cb)
    };

    Ok(DigitNodeData {
        data: read_option_cb(reader, &cb1)?,
        prefix: read_vec_cb(reader, &read_usize)?,
        children: read_option_cb(reader, &cb)?,
    })
}

fn write_multi_oracle_trie<W: Writer>(
    trie: &MultiOracleTrie,
    w: &mut W,
) -> Result<(), lightning::io::Error> {
    multi_oracle_trie_dump::write(&trie.dump(), w)
}

fn read_multi_oracle_trie<R: Read>(reader: &mut R) -> Result<MultiOracleTrie, DecodeError> {
    let dump = multi_oracle_trie_dump::read(reader)?;
    Ok(MultiOracleTrie::from_dump(dump))
}

fn write_multi_oracle_trie_with_diff<W: Writer>(
    trie: &MultiOracleTrieWithDiff,
    w: &mut W,
) -> Result<(), lightning::io::Error> {
    multi_oracle_trie_with_diff_dump::write(&trie.dump(), w)
}

fn read_multi_oracle_trie_with_diff<R: Read>(
    reader: &mut R,
) -> Result<MultiOracleTrieWithDiff, DecodeError> {
    let dump = multi_oracle_trie_with_diff_dump::read(reader)?;
    Ok(MultiOracleTrieWithDiff::from_dump(dump))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::chain_hash_from_network;
    use bitcoin::Network;
    use lightning::io::Cursor;

    fn offered_contract() -> OfferedContract {
        let offer_dlc: ddk_messages::OfferDlc =
            serde_json::from_str(include_str!("../../test_inputs/offer_contract.json")).unwrap();
        let counter_party = "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443"
            .parse()
            .unwrap();
        OfferedContract::try_from_offer_dlc(&offer_dlc, counter_party, [7u8; 32]).unwrap()
    }

    fn accepted_contract() -> AcceptedContract {
        use secp256k1_zkp::{Message, Secp256k1, SecretKey};
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[1; 32]).unwrap();
        let dummy_transaction = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        };
        let offered = offered_contract();
        AcceptedContract {
            accept_params: ddk_dlc::PartyParams {
                fund_pubkey: secret_key.public_key(&secp),
                change_script_pubkey: bitcoin::ScriptBuf::new(),
                change_serial_id: 1,
                payout_script_pubkey: bitcoin::ScriptBuf::new(),
                payout_serial_id: 2,
                inputs: Vec::new(),
                dlc_inputs: Vec::new(),
                input_amount: Amount::from_sat(1_000),
                collateral: Amount::from_sat(500),
            },
            offered_contract: offered,
            funding_inputs: Vec::new(),
            adaptor_infos: Vec::new(),
            adaptor_signatures: Vec::new(),
            accept_refund_signature: secp.sign_ecdsa(&Message::from_digest([1; 32]), &secret_key),
            dlc_transactions: DlcTransactions {
                // The fund output must exist for `get_sign_dlc` to find it.
                fund: bitcoin::Transaction {
                    output: vec![bitcoin::TxOut {
                        value: Amount::from_sat(1_000),
                        script_pubkey: bitcoin::ScriptBuf::new().to_p2wsh(),
                    }],
                    ..dummy_transaction.clone()
                },
                cets: vec![dummy_transaction.clone()],
                refund: dummy_transaction,
                funding_witness_script: bitcoin::ScriptBuf::new(),
                pending_close_txs: Vec::new(),
            },
            tlvs: Default::default(),
        }
    }

    /// A stream holding one record of type 65007 with `body` as its one-byte body.
    fn stream_with_record(body: u8) -> ddk_messages::tlv_stream::TlvStream {
        let bytes = [0xfd, 0xfd, 0xef, 0x01, body];
        ddk_messages::tlv_stream::TlvStream::read_to_end(&mut Cursor::new(bytes)).unwrap()
    }

    /// A failed accept stored before the message was length-framed. The
    /// message was written raw, followed directly by the error string.
    #[test]
    fn failed_accept_stored_before_framing_still_loads() {
        let offered = offered_contract();
        let accept_message = accepted_contract().get_accept_contract_msg(&[]);
        let mut stored = offered.serialize().unwrap();
        stored.extend(accept_message.serialize().unwrap());
        ddk_messages::ser_impls::write_string("kaput", &mut stored).unwrap();

        let read = FailedAcceptContract::deserialize(&mut Cursor::new(&stored)).unwrap();

        assert_eq!(read.accept_message, accept_message);
        assert_eq!(read.error_message, "kaput");
    }

    /// The frame keeps the message's TLV stream from reading into the error
    /// string that is stored after it.
    #[test]
    fn failed_accept_with_records_round_trips() {
        let mut accept_message = accepted_contract().get_accept_contract_msg(&[]);
        accept_message.tlvs = stream_with_record(7);
        let contract = FailedAcceptContract {
            offered_contract: offered_contract(),
            accept_message,
            error_message: "kaput".to_string(),
        };

        let stored = contract.serialize().unwrap();
        let read = FailedAcceptContract::deserialize(&mut Cursor::new(&stored)).unwrap();

        assert_eq!(read.accept_message, contract.accept_message);
        assert_eq!(read.error_message, contract.error_message);
    }

    #[test]
    fn failed_sign_stored_before_framing_still_loads() {
        let accepted = accepted_contract();
        let sign_message = SignedContract {
            accepted_contract: accepted.clone(),
            adaptor_signatures: Vec::new(),
            offer_refund_signature: accepted.accept_refund_signature,
            funding_signatures: ddk_messages::FundingSignatures {
                funding_signatures: Vec::new(),
            },
            channel_id: None,
            tlvs: Default::default(),
        }
        .get_sign_dlc(Vec::new());
        let mut stored = accepted.serialize().unwrap();
        stored.extend(sign_message.serialize().unwrap());
        ddk_messages::ser_impls::write_string("kaput", &mut stored).unwrap();

        let read = FailedSignContract::deserialize(&mut Cursor::new(&stored)).unwrap();

        assert_eq!(read.sign_message, sign_message);
        assert_eq!(read.error_message, "kaput");
    }

    #[test]
    fn failed_sign_with_records_round_trips() {
        let accepted = accepted_contract();
        let mut sign_message = SignedContract {
            accepted_contract: accepted.clone(),
            adaptor_signatures: Vec::new(),
            offer_refund_signature: accepted.accept_refund_signature,
            funding_signatures: ddk_messages::FundingSignatures {
                funding_signatures: Vec::new(),
            },
            channel_id: None,
            tlvs: Default::default(),
        }
        .get_sign_dlc(Vec::new());
        sign_message.tlvs = stream_with_record(9);
        let contract = FailedSignContract {
            accepted_contract: accepted,
            sign_message,
            error_message: "kaput".to_string(),
        };

        let stored = contract.serialize().unwrap();
        let read = FailedSignContract::deserialize(&mut Cursor::new(&stored)).unwrap();

        assert_eq!(read.sign_message, contract.sign_message);
        assert_eq!(read.error_message, contract.error_message);
    }

    /// Serializes `contract` in the formats used before chain_hash (and,
    /// with `with_contract_flags` false, before contract_flags) existed.
    fn serialize_pre_chain_hash(contract: &OfferedContract, with_contract_flags: bool) -> Vec<u8> {
        let mut w = Vec::new();
        contract.id.write(&mut w).unwrap();
        contract.is_offer_party.write(&mut w).unwrap();
        write_vec(&contract.contract_info, &mut w).unwrap();
        ddk_messages::ser_impls::party_params::write(&contract.offer_params, &mut w).unwrap();
        contract.total_collateral.write(&mut w).unwrap();
        write_vec(&contract.funding_inputs, &mut w).unwrap();
        contract.fund_output_serial_id.write(&mut w).unwrap();
        contract.fee_rate_per_vb.write(&mut w).unwrap();
        contract.cet_locktime.write(&mut w).unwrap();
        contract.refund_locktime.write(&mut w).unwrap();
        if with_contract_flags {
            contract.contract_flags.write(&mut w).unwrap();
        }
        contract.counter_party.write(&mut w).unwrap();
        contract.keys_id.write(&mut w).unwrap();
        w
    }

    #[test]
    fn chain_hash_survives_storage_round_trip() {
        let mut contract = offered_contract();
        contract.chain_hash = Some(chain_hash_from_network(Network::Bitcoin));
        contract.contract_flags = 1;

        let serialized = contract.serialize().unwrap();
        let read = OfferedContract::deserialize(&mut Cursor::new(&serialized)).unwrap();

        assert_eq!(read.chain_hash, contract.chain_hash);
        assert_eq!(read.contract_flags, contract.contract_flags);
        assert_eq!(read.counter_party, contract.counter_party);
        assert_eq!(read.keys_id, contract.keys_id);
    }

    #[test]
    fn contract_stored_without_chain_hash_reads_as_none() {
        let mut contract = offered_contract();
        contract.contract_flags = 1;
        let serialized = serialize_pre_chain_hash(&contract, true);

        let read = OfferedContract::deserialize(&mut Cursor::new(&serialized)).unwrap();

        assert_eq!(read.chain_hash, None);
        assert_eq!(read.contract_flags, contract.contract_flags);
        assert_eq!(read.counter_party, contract.counter_party);
        assert_eq!(read.keys_id, contract.keys_id);
    }

    #[test]
    fn contract_stored_without_contract_flags_reads_as_none() {
        let contract = offered_contract();
        let serialized = serialize_pre_chain_hash(&contract, false);

        let read = OfferedContract::deserialize(&mut Cursor::new(&serialized)).unwrap();

        assert_eq!(read.chain_hash, None);
        assert_eq!(read.contract_flags, 0);
        assert_eq!(read.counter_party, contract.counter_party);
        assert_eq!(read.keys_id, contract.keys_id);
    }

    /// A contract with no chain hash writes the bytes it was stored as, so
    /// upgrading ddk does not rewrite contracts already in a database.
    #[test]
    fn contract_without_chain_hash_reserializes_unchanged() {
        let mut contract = offered_contract();
        contract.contract_flags = 1;
        let stored = serialize_pre_chain_hash(&contract, true);

        let read = OfferedContract::deserialize(&mut Cursor::new(&stored)).unwrap();

        assert_eq!(read.serialize().unwrap(), stored);
    }

    /// The backward-compatible read of [`OfferedContract`] tells a chain hash
    /// apart from a compressed pubkey by its first byte, so no supported
    /// network's chain hash may start with a pubkey prefix (0x02/0x03).
    #[test]
    fn chain_hash_first_byte_is_not_a_pubkey_prefix() {
        for network in [
            Network::Bitcoin,
            Network::Testnet,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            let first = chain_hash_from_network(network)[0];
            assert!(
                first != 0x02 && first != 0x03,
                "{network} chain hash starts with a pubkey prefix byte"
            );
        }
    }
}
