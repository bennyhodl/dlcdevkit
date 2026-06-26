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
    ClosedContract, ContractDescriptor, FailedAcceptContract, FailedSignContract, PreClosedContract,
};
use crate::KeysId;
use bitcoin::Amount;
use crate::payout_curve::{
    HyperbolaPayoutCurvePiece, PayoutFunction, PayoutFunctionPiece, PayoutPoint,
    PolynomialPayoutCurvePiece, RoundingInterval, RoundingIntervals,
};
use ddk_dlc::DlcTransactions;
use ddk_messages::impl_dlc_writeable;
use ddk_messages::ser_impls::{
    read_ecdsa_adaptor_signatures, read_option_cb, read_usize, read_vec, read_vec_cb,
    write_ecdsa_adaptor_signatures, write_option_cb, write_usize, write_vec, write_vec_cb,
};
use ddk_trie::digit_trie::{DigitNodeData, DigitTrieDump};
use ddk_trie::multi_oracle_trie::{MultiOracleTrie, MultiOracleTrieDump};
use ddk_trie::multi_oracle_trie_with_diff::{MultiOracleTrieWithDiff, MultiOracleTrieWithDiffDump};
use ddk_trie::multi_trie::{MultiTrieDump, MultiTrieNodeData, TrieNodeInfo};
use ddk_trie::{OracleNumericInfo, RangeInfo};
use lightning::io::Read;
use lightning::ln::msgs::DecodeError;
use lightning::util::ser::{Readable, Writeable, Writer};

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

        // Backward compatibility: contract_flags (u8) was inserted between
        // refund_locktime and counter_party. Peek one byte: 0x02/0x03 means
        // old format (compressed pubkey prefix), 0x00/0x01 means new format
        // (contract_flags value).
        let mut peek = [0u8; 1];
        r.read_exact(&mut peek)?;
        let (contract_flags, counter_party) = if peek[0] == 0x02 || peek[0] == 0x03 {
            // Old format: this byte is the start of counter_party pubkey
            let mut pubkey_bytes = [0u8; 33];
            pubkey_bytes[0] = peek[0];
            r.read_exact(&mut pubkey_bytes[1..])?;
            let pk = secp256k1_zkp::PublicKey::from_slice(&pubkey_bytes)
                .map_err(|_| DecodeError::InvalidValue)?;
            (0u8, pk)
        } else {
            // New format: this byte is contract_flags
            let counter_party: secp256k1_zkp::PublicKey = Readable::read(r)?;
            (peek[0], counter_party)
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
            counter_party,
            keys_id,
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
    (funding_script_pubkey, writeable),
    (pending_close_txs, vec)}
);
impl_dlc_writeable!(AcceptedContract, {
    (offered_contract, writeable),
    (accept_params, { cb_writeable, ddk_messages::ser_impls::party_params::write, ddk_messages::ser_impls::party_params::read }),
    (funding_inputs, vec),
    (adaptor_infos, vec),
    (adaptor_signatures, { cb_writeable, write_ecdsa_adaptor_signatures, read_ecdsa_adaptor_signatures }),
    (accept_refund_signature, writeable),
    (dlc_transactions, {cb_writeable, dlc_transactions::write, dlc_transactions::read })
});
impl_dlc_writeable!(SignedContract, {
    (accepted_contract, writeable),
    (adaptor_signatures, { cb_writeable, write_ecdsa_adaptor_signatures, read_ecdsa_adaptor_signatures }),
    (offer_refund_signature, writeable),
    (funding_signatures, writeable),
    (channel_id, option)
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
impl_dlc_writeable!(FailedAcceptContract, {(offered_contract, writeable), (accept_message, writeable), (error_message, string)});
impl_dlc_writeable!(FailedSignContract, {(accepted_contract, writeable), (sign_message, writeable), (error_message, string)});

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
