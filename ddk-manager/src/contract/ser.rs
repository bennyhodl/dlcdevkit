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
