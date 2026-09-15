//! The columnar layout of a stored contract: one row in `dlc_contracts`.
//!
//! The DLC wire messages are the truth. The offer, accept, and sign messages
//! are stored byte for byte, so they keep their TLV streams and their own
//! protocol versioning. The state the manager derives and that the messages
//! do not carry, such as the adaptor info and the DLC transactions, lives in
//! its own column, one type per column. [`ContractRow::from_contract`] is the
//! one writer and [`ContractRow::into_contract`] is the one reader. Every
//! read path of the Postgres store goes through them.

use bitcoin::consensus;
use bitcoin::secp256k1::PublicKey;
use bitcoin::{SignedAmount, Transaction, Txid};
use ddk_manager::contract::accepted_contract::AcceptedContract;
use ddk_manager::contract::offered_contract::OfferedContract;
use ddk_manager::contract::ser::{dlc_transactions, ContractPrefix};
use ddk_manager::contract::signed_contract::SignedContract;
use ddk_manager::contract::{
    AdaptorInfo, ClosedContract, Contract, FailedAcceptContract, FailedSignContract,
    PreClosedContract,
};
use ddk_manager::error::Error;
use ddk_messages::oracle_msgs::OracleAttestation;
use ddk_messages::ser_impls::{party_params, read_vec, write_vec};
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};
use lightning::util::ser::{Readable, Writeable};
use sqlx::FromRow;
use std::str::FromStr;

/// The layout version of the rows this module writes.
///
/// Version 1 is the legacy blob layout in the `contract_data` table. It never
/// appears in `dlc_contracts`; rows are moved out of it by
/// `PostgresStore::migrate_legacy_contracts`.
pub const CONTRACT_ROW_FORMAT_VERSION: i16 = 2;

/// The layout version of the legacy blob rows in `contract_data`.
pub const LEGACY_BLOB_FORMAT_VERSION: i16 = 1;

/// The value of `announcement_id` and `oracle_pubkey` when a contract has no
/// announcement to report. Kept from the metadata table so consumers keep
/// their filter.
pub const NO_ANNOUNCEMENT: &str = "legacy_data";

/// One row of `dlc_contracts`. The column list is the struct, so adding a
/// field is a migration plus a change to the two functions below.
#[derive(Debug, Clone, FromRow)]
pub struct ContractRow {
    /// Hex contract id, or the temporary id before the contract is accepted.
    pub id: String,
    pub format_version: i16,
    /// The [`ContractPrefix`] of the stored state.
    pub state: i16,
    /// Hex temporary contract id.
    pub temporary_id: String,
    pub is_offer_party: bool,
    /// Hex compressed public key of the counter party.
    pub counter_party: String,
    pub keys_id: Vec<u8>,
    pub contract_flags: i16,
    pub chain_hash: Option<Vec<u8>>,
    pub offer_collateral: i64,
    pub accept_collateral: i64,
    pub total_collateral: i64,
    pub fee_rate_per_vb: i64,
    pub cet_locktime: i32,
    pub refund_locktime: i32,
    pub announcement_id: String,
    pub oracle_pubkey: String,
    pub funding_txid: Option<String>,
    pub cet_txid: Option<String>,
    pub pnl: Option<i64>,
    /// The offer message, wire encoded.
    pub offer_message: Vec<u8>,
    /// The accept message, wire encoded. Set from the accepted state on, and
    /// on a failed accept, where it is the message that failed.
    pub accept_message: Option<Vec<u8>>,
    /// The sign message, wire encoded. Set from the signed state on, and on
    /// a failed sign, where it is the message that failed.
    pub sign_message: Option<Vec<u8>>,
    /// The offer party's params, wire encoded.
    pub offer_params: Vec<u8>,
    /// The accept party's params, wire encoded. Set from the accepted state on.
    pub accept_params: Option<Vec<u8>>,
    /// The adaptor infos, wire encoded. Set from the accepted state on.
    pub adaptor_infos: Option<Vec<u8>>,
    /// The DLC transactions, wire encoded. Set from the accepted state on.
    pub dlc_transactions: Option<Vec<u8>>,
    pub channel_id: Option<Vec<u8>>,
    /// The oracle attestations, wire encoded. Set on pre-closed and closed
    /// contracts that closed with an attestation.
    pub attestations: Option<Vec<u8>>,
    /// The signed CET, consensus encoded. Set on pre-closed contracts and on
    /// closed contracts that closed with a CET.
    pub signed_cet: Option<Vec<u8>>,
    /// The error of a failed accept or failed sign.
    pub error_message: Option<String>,
}

fn storage_error(column: &str, error: impl std::fmt::Debug) -> Error {
    Error::StorageError(format!("dlc_contracts.{column}: {error:?}"))
}

fn decode<T: Readable>(column: &str, bytes: &[u8]) -> Result<T, Error> {
    let mut cursor = lightning::io::Cursor::new(bytes);
    T::read(&mut cursor).map_err(|e| storage_error(column, e))
}

fn decode_with<'a, T, R>(column: &str, bytes: &'a [u8], read: R) -> Result<T, Error>
where
    R: FnOnce(&mut lightning::io::Cursor<&'a [u8]>) -> Result<T, lightning::ln::msgs::DecodeError>,
{
    let mut cursor = lightning::io::Cursor::new(bytes);
    read(&mut cursor).map_err(|e| storage_error(column, e))
}

fn encode_with<T, W>(column: &str, value: &T, write: W) -> Result<Vec<u8>, Error>
where
    W: FnOnce(&T, &mut Vec<u8>) -> Result<(), lightning::io::Error>,
{
    let mut buffer = Vec::new();
    write(value, &mut buffer).map_err(|e| storage_error(column, e))?;
    Ok(buffer)
}

fn required<T>(column: &str, value: Option<T>) -> Result<T, Error> {
    value.ok_or_else(|| storage_error(column, "missing for this state"))
}

fn array_32(column: &str, bytes: &[u8]) -> Result<[u8; 32], Error> {
    bytes
        .try_into()
        .map_err(|_| storage_error(column, format!("expected 32 bytes, got {}", bytes.len())))
}

fn hex_32(column: &str, hex: &str) -> Result<[u8; 32], Error> {
    let bytes = hex::decode(hex).map_err(|e| storage_error(column, e))?;
    array_32(column, &bytes)
}

fn adaptor_signatures(
    signatures: &ddk_messages::CetAdaptorSignatures,
) -> Vec<ddk_dlc::secp256k1_zkp::EcdsaAdaptorSignature> {
    signatures
        .ecdsa_adaptor_signatures
        .iter()
        .map(|s| s.signature)
        .collect()
}

/// The offered, accepted, and signed contract a state wraps, whichever exist.
fn parts(
    contract: &Contract,
) -> (
    &OfferedContract,
    Option<&AcceptedContract>,
    Option<&SignedContract>,
) {
    match contract {
        Contract::Offered(o) | Contract::Rejected(o) => (o, None, None),
        Contract::Accepted(a) => (&a.offered_contract, Some(a), None),
        Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => (
            &s.accepted_contract.offered_contract,
            Some(&s.accepted_contract),
            Some(s),
        ),
        Contract::PreClosed(p) => parts_of_signed(&p.signed_contract),
        Contract::Closed(c) => parts_of_signed(&c.signed_contract),
        Contract::FailedAccept(f) => (&f.offered_contract, None, None),
        Contract::FailedSign(f) => (
            &f.accepted_contract.offered_contract,
            Some(&f.accepted_contract),
            None,
        ),
    }
}

fn parts_of_signed(
    signed: &SignedContract,
) -> (
    &OfferedContract,
    Option<&AcceptedContract>,
    Option<&SignedContract>,
) {
    (
        &signed.accepted_contract.offered_contract,
        Some(&signed.accepted_contract),
        Some(signed),
    )
}

impl ContractRow {
    /// The one writer: the row for a contract in any state.
    pub fn from_contract(contract: &Contract) -> Result<Self, Error> {
        let (offered, accepted, signed) = parts(contract);
        // The `Contract` accessors return zeros for a closed contract and
        // unwrap its CET, so the metadata comes from the offered contract
        // every state wraps.
        let offer_collateral = offered.offer_params.collateral;
        let total_collateral = offered.total_collateral;
        let accept_collateral = total_collateral
            .checked_sub(offer_collateral)
            .unwrap_or(bitcoin::Amount::ZERO);

        let announcement = offered
            .contract_info
            .first()
            .and_then(|info| info.oracle_announcements.first());
        let announcement_id = announcement
            .map(|a| a.oracle_event.event_id.clone())
            .unwrap_or_else(|| NO_ANNOUNCEMENT.to_string());
        let oracle_pubkey = announcement
            .map(|a| a.oracle_public_key.to_string())
            .unwrap_or_else(|| NO_ANNOUNCEMENT.to_string());

        let offer_message = OfferDlc::from(offered).encode();
        let accept_message = match contract {
            Contract::FailedAccept(f) => Some(f.accept_message.encode()),
            _ => accepted.map(|a| a.get_accept_contract_msg(&a.adaptor_signatures).encode()),
        };
        let sign_message = match contract {
            Contract::FailedSign(f) => Some(f.sign_message.encode()),
            _ => signed.map(|s| s.get_sign_dlc(s.adaptor_signatures.clone()).encode()),
        };

        let offer_params = encode_with("offer_params", &offered.offer_params, party_params::write)?;
        let accept_params = accepted
            .map(|a| encode_with("accept_params", &a.accept_params, party_params::write))
            .transpose()?;
        let adaptor_infos = accepted
            .map(|a| encode_with("adaptor_infos", &a.adaptor_infos, write_vec))
            .transpose()?;
        let dlc_transactions = accepted
            .map(|a| {
                encode_with(
                    "dlc_transactions",
                    &a.dlc_transactions,
                    dlc_transactions::write,
                )
            })
            .transpose()?;

        let (attestations, signed_cet) = match contract {
            Contract::PreClosed(p) => (p.attestations.as_ref(), Some(&p.signed_cet)),
            Contract::Closed(c) => (c.attestations.as_ref(), c.signed_cet.as_ref()),
            _ => (None, None),
        };
        let attestations = attestations
            .map(|a| encode_with("attestations", a, write_vec))
            .transpose()?;
        let signed_cet = signed_cet.map(consensus::serialize);

        let error_message = match contract {
            Contract::FailedAccept(f) => Some(f.error_message.clone()),
            Contract::FailedSign(f) => Some(f.error_message.clone()),
            _ => None,
        };

        Ok(ContractRow {
            id: hex::encode(contract.get_id()),
            format_version: CONTRACT_ROW_FORMAT_VERSION,
            state: ContractPrefix::get_prefix(contract) as i16,
            temporary_id: hex::encode(contract.get_temporary_id()),
            is_offer_party: offered.is_offer_party,
            counter_party: hex::encode(contract.get_counter_party_id().serialize()),
            keys_id: offered.keys_id().to_vec(),
            contract_flags: offered.contract_flags as i16,
            chain_hash: offered.chain_hash.map(|h| h.to_vec()),
            offer_collateral: offer_collateral.to_sat() as i64,
            accept_collateral: accept_collateral.to_sat() as i64,
            total_collateral: total_collateral.to_sat() as i64,
            fee_rate_per_vb: offered.fee_rate_per_vb as i64,
            cet_locktime: offered.cet_locktime as i32,
            refund_locktime: offered.refund_locktime as i32,
            announcement_id,
            oracle_pubkey,
            funding_txid: contract.get_funding_txid().map(|txid| txid.to_string()),
            cet_txid: contract.get_cet_txid().map(|txid| txid.to_string()),
            pnl: Some(contract.get_pnl().to_sat()),
            offer_message,
            accept_message,
            sign_message,
            offer_params,
            accept_params,
            adaptor_infos,
            dlc_transactions,
            channel_id: signed.and_then(|s| s.channel_id).map(|id| id.to_vec()),
            attestations,
            signed_cet,
            error_message,
        })
    }

    /// The one reader: the contract a row stands for.
    pub fn into_contract(self) -> Result<Contract, Error> {
        if self.format_version != CONTRACT_ROW_FORMAT_VERSION {
            return Err(storage_error(
                "format_version",
                format!("unknown row format {}", self.format_version),
            ));
        }
        let prefix = ContractPrefix::try_from(self.state as u8)?;
        Ok(match prefix {
            ContractPrefix::Offered => Contract::Offered(self.offered()?),
            ContractPrefix::Rejected => Contract::Rejected(self.offered()?),
            ContractPrefix::Accepted => Contract::Accepted(self.accepted()?),
            ContractPrefix::Signed => Contract::Signed(self.signed()?),
            ContractPrefix::Confirmed => Contract::Confirmed(self.signed()?),
            ContractPrefix::Refunded => Contract::Refunded(self.signed()?),
            ContractPrefix::PreClosed => Contract::PreClosed(PreClosedContract {
                signed_contract: self.signed()?,
                attestations: self.attestations()?,
                signed_cet: required("signed_cet", self.signed_cet()?)?,
            }),
            ContractPrefix::Closed => Contract::Closed(ClosedContract {
                attestations: self.attestations()?,
                signed_cet: self.signed_cet()?,
                contract_id: hex_32("id", &self.id)?,
                temporary_contract_id: hex_32("temporary_id", &self.temporary_id)?,
                counter_party_id: self.counter_party()?,
                funding_txid: Txid::from_str(required(
                    "funding_txid",
                    self.funding_txid.as_deref(),
                )?)
                .map_err(|e| storage_error("funding_txid", e))?,
                pnl: SignedAmount::from_sat(required("pnl", self.pnl)?),
                signed_contract: self.signed()?,
            }),
            ContractPrefix::FailedAccept => Contract::FailedAccept(FailedAcceptContract {
                offered_contract: self.offered()?,
                accept_message: self.accept_message()?,
                error_message: required("error_message", self.error_message.clone())?,
            }),
            ContractPrefix::FailedSign => Contract::FailedSign(FailedSignContract {
                accepted_contract: self.accepted()?,
                sign_message: self.sign_message()?,
                error_message: required("error_message", self.error_message.clone())?,
            }),
        })
    }

    fn counter_party(&self) -> Result<PublicKey, Error> {
        PublicKey::from_str(&self.counter_party).map_err(|e| storage_error("counter_party", e))
    }

    fn offered(&self) -> Result<OfferedContract, Error> {
        let offer: OfferDlc = decode("offer_message", &self.offer_message)?;
        let keys_id = array_32("keys_id", &self.keys_id)?;
        let mut offered =
            OfferedContract::try_from_offer_dlc(&offer, self.counter_party()?, keys_id)
                .map_err(|e| storage_error("offer_message", e))?;
        offered.is_offer_party = self.is_offer_party;
        offered.chain_hash = self
            .chain_hash
            .as_deref()
            .map(|h| array_32("chain_hash", h))
            .transpose()?;
        offered.offer_params = decode_with("offer_params", &self.offer_params, party_params::read)?;
        Ok(offered)
    }

    fn accept_message(&self) -> Result<AcceptDlc, Error> {
        decode(
            "accept_message",
            required("accept_message", self.accept_message.as_deref())?,
        )
    }

    fn accepted(&self) -> Result<AcceptedContract, Error> {
        let accept = self.accept_message()?;
        Ok(AcceptedContract {
            offered_contract: self.offered()?,
            accept_params: decode_with(
                "accept_params",
                required("accept_params", self.accept_params.as_deref())?,
                party_params::read,
            )?,
            funding_inputs: accept.funding_inputs,
            adaptor_infos: decode_with::<Vec<AdaptorInfo>, _>(
                "adaptor_infos",
                required("adaptor_infos", self.adaptor_infos.as_deref())?,
                read_vec,
            )?,
            adaptor_signatures: adaptor_signatures(&accept.cet_adaptor_signatures),
            accept_refund_signature: accept.refund_signature,
            dlc_transactions: decode_with(
                "dlc_transactions",
                required("dlc_transactions", self.dlc_transactions.as_deref())?,
                dlc_transactions::read,
            )?,
            tlvs: accept.tlvs,
        })
    }

    fn sign_message(&self) -> Result<SignDlc, Error> {
        decode(
            "sign_message",
            required("sign_message", self.sign_message.as_deref())?,
        )
    }

    fn signed(&self) -> Result<SignedContract, Error> {
        let sign = self.sign_message()?;
        Ok(SignedContract {
            accepted_contract: self.accepted()?,
            adaptor_signatures: adaptor_signatures(&sign.cet_adaptor_signatures),
            offer_refund_signature: sign.refund_signature,
            funding_signatures: sign.funding_signatures,
            channel_id: self
                .channel_id
                .as_deref()
                .map(|id| array_32("channel_id", id))
                .transpose()?,
            tlvs: sign.tlvs,
        })
    }

    fn attestations(&self) -> Result<Option<Vec<OracleAttestation>>, Error> {
        self.attestations
            .as_deref()
            .map(|bytes| decode_with("attestations", bytes, read_vec))
            .transpose()
    }

    fn signed_cet(&self) -> Result<Option<Transaction>, Error> {
        self.signed_cet
            .as_deref()
            .map(|bytes| consensus::deserialize(bytes).map_err(|e| storage_error("signed_cet", e)))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::ser::{deserialize_contract, serialize_contract};
    use ddk_messages::tlv_stream::TlvStream;

    fn fixture(name: &str) -> Contract {
        let path = format!(
            "{}/../testconfig/contract_binaries/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let bytes = std::fs::read(path).unwrap();
        deserialize_contract(&bytes).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
    }

    /// The row keeps everything the legacy blob kept: a contract written as
    /// a row and read back serializes to the same legacy bytes.
    fn assert_round_trips(contract: &Contract) {
        let expected = serialize_contract(contract).unwrap();
        let row = ContractRow::from_contract(contract).unwrap();
        assert_eq!(row.format_version, CONTRACT_ROW_FORMAT_VERSION);
        let read = row.into_contract().unwrap();
        assert_eq!(serialize_contract(&read).unwrap(), expected);
    }

    #[test]
    fn stored_contracts_round_trip_byte_for_byte() {
        for name in [
            "Offered",
            "Accepted",
            "Signed",
            "Confirmed",
            "PreClosed",
            "Closed",
            "old/Offered",
        ] {
            assert_round_trips(&fixture(name));
        }
    }

    /// The states that wrap another state's struct share its columns.
    #[test]
    fn wrapped_states_round_trip() {
        let Contract::Signed(signed) = fixture("Signed") else {
            panic!("fixture is not signed")
        };
        let Contract::Offered(offered) = fixture("Offered") else {
            panic!("fixture is not offered")
        };
        let Contract::Accepted(accepted) = fixture("Accepted") else {
            panic!("fixture is not accepted")
        };
        let accept_message = accepted.get_accept_contract_msg(&accepted.adaptor_signatures);
        let sign_message = signed.get_sign_dlc(signed.adaptor_signatures.clone());

        assert_round_trips(&Contract::Rejected(offered.clone()));
        assert_round_trips(&Contract::Refunded(signed.clone()));
        assert_round_trips(&Contract::FailedAccept(FailedAcceptContract {
            offered_contract: offered,
            accept_message,
            error_message: "bad accept".to_string(),
        }));
        assert_round_trips(&Contract::FailedSign(FailedSignContract {
            accepted_contract: accepted,
            sign_message,
            error_message: "bad sign".to_string(),
        }));
    }

    /// A stream holding one record of type 65007 with `body` as its one-byte body.
    fn stream_with_record(body: u8) -> TlvStream {
        let bytes = [0xfd, 0xfd, 0xef, 0x01, body];
        TlvStream::read_to_end(&mut lightning::io::Cursor::new(bytes)).unwrap()
    }

    /// The TLV streams travel inside the stored messages, on every state.
    #[test]
    fn tlv_streams_survive_on_every_state() {
        let Contract::Closed(mut closed) = fixture("Closed") else {
            panic!("fixture is not closed")
        };
        closed
            .signed_contract
            .accepted_contract
            .offered_contract
            .tlvs = stream_with_record(1);
        closed.signed_contract.accepted_contract.tlvs = stream_with_record(2);
        closed.signed_contract.tlvs = stream_with_record(3);

        let row = ContractRow::from_contract(&Contract::Closed(closed)).unwrap();
        let Contract::Closed(read) = row.into_contract().unwrap() else {
            panic!("state changed in storage")
        };
        let signed = read.signed_contract;
        assert_eq!(
            signed.accepted_contract.offered_contract.tlvs,
            stream_with_record(1)
        );
        assert_eq!(signed.accepted_contract.tlvs, stream_with_record(2));
        assert_eq!(signed.tlvs, stream_with_record(3));
    }

    /// A row of an unknown layout is an error, not a misread.
    #[test]
    fn unknown_format_version_is_rejected() {
        let mut row = ContractRow::from_contract(&fixture("Offered")).unwrap();
        row.format_version = 99;
        assert!(row.into_contract().is_err());
    }

    /// A state that needs a column the row does not hold is an error that
    /// names the column.
    #[test]
    fn missing_column_names_the_column() {
        let mut row = ContractRow::from_contract(&fixture("Signed")).unwrap();
        row.sign_message = None;
        let error = row.into_contract().unwrap_err().to_string();
        assert!(error.contains("sign_message"), "{error}");
    }
}
