use ddk_manager::channel::signed_channel::SignedChannelStateType;
use ddk_manager::channel::Channel;
use ddk_manager::contract::accepted_contract::AcceptedContract;
use ddk_manager::contract::offered_contract::OfferedContract;
use ddk_manager::contract::ser::Serializable;
use ddk_manager::contract::signed_contract::SignedContract;
use ddk_manager::contract::{
    ClosedContract, Contract, FailedAcceptContract, FailedSignContract, PreClosedContract,
};
use ddk_manager::error::Error;
use ddk_messages::tlv_stream::TlvStream;
use ddk_messages::Message;
use lightning::io::Read;
use lightning::util::ser::{BigSize, FixedLengthReader, Readable, Writeable};

use crate::error::to_storage_error;

/// Helper from rust-dlc to implement types for contracts.
macro_rules! convertible_enum {
    (enum $name:ident {
        $($vname:ident $(= $val:expr)?,)*;
        $($tname:ident $(= $tval:expr)?,)*
    }, $input:ident) => {
        #[derive(Debug)]
        pub enum $name {
            $($vname $(= $val)?,)*
            $($tname $(= $tval)?,)*
        }

        impl From<$name> for u8 {
            fn from(prefix: $name) -> u8 {
                prefix as u8
            }
        }

        impl std::convert::TryFrom<u8> for $name {
            type Error = Error;

            fn try_from(v: u8) -> Result<Self, Self::Error> {
                match v {
                    $(x if x == u8::from($name::$vname) => Ok($name::$vname),)*
                    $(x if x == u8::from($name::$tname) => Ok($name::$tname),)*
                    _ => Err(Error::StorageError("Unknown prefix".to_string())),
                }
            }
        }

        impl $name {
            pub fn get_prefix(input: &$input) -> u8 {
                let prefix = match input {
                    $($input::$vname(_) => $name::$vname,)*
                    $($input::$tname{..} => $name::$tname,)*
                };
                prefix.into()
            }
        }
    }
}

convertible_enum!(
    enum ContractPrefix {
        Offered = 1,
        // 2
        Accepted,
        // 3
        Signed,
        // 4
        Confirmed,
        // 5
        PreClosed,
        // 6
        Closed,
        // 7
        FailedAccept,
        // 8
        FailedSign,
        // 9
        Refunded,
        // 10
        Rejected,;
    },
    Contract
);

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

convertible_enum!(
    enum ChannelPrefix {
        Offered = 100,
        Accepted,
        Signed,
        FailedAccept,
        FailedSign,
        Closing,
        Closed,
        CounterClosed,
        ClosedPunished,
        CollaborativelyClosed,
        Cancelled,;
    },
    Channel
);

convertible_enum!(
    enum SignedChannelPrefix {;
        Established = 1,
        SettledOffered,
        SettledReceived,
        SettledAccepted,
        SettledConfirmed,
        Settled,
        Closing,
        CollaborativeCloseOffered,
        RenewAccepted,
        RenewOffered,
        RenewFinalized,
        RenewConfirmed,
    },
    SignedChannelStateType
);

/// Version byte of the TLV suffix appended after the serialized contract.
const TLV_SUFFIX_VERSION: u8 = 1;

/// The TLV streams a contract carries, in the order the suffix stores them.
///
/// The streams go in a suffix after the struct bytes because the structs nest:
/// a signed contract contains the accepted one, which contains the offered one.
/// Old data has no stream bytes inside the structs, and a suffix keeps it that
/// way. Old data ends where the struct ends, so contracts stored before the
/// suffix existed still load.
fn tlv_streams(contract: &Contract) -> Vec<&TlvStream> {
    match contract {
        Contract::Offered(o) | Contract::Rejected(o) => vec![&o.tlvs],
        Contract::Accepted(a) => vec![&a.offered_contract.tlvs, &a.tlvs],
        Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => vec![
            &s.accepted_contract.offered_contract.tlvs,
            &s.accepted_contract.tlvs,
            &s.tlvs,
        ],
        Contract::PreClosed(p) => vec![
            &p.signed_contract.accepted_contract.offered_contract.tlvs,
            &p.signed_contract.accepted_contract.tlvs,
            &p.signed_contract.tlvs,
        ],
        Contract::FailedAccept(f) => vec![&f.offered_contract.tlvs],
        Contract::FailedSign(f) => vec![
            &f.accepted_contract.offered_contract.tlvs,
            &f.accepted_contract.tlvs,
        ],
        Contract::Closed(_) => vec![],
    }
}

fn tlv_streams_mut(contract: &mut Contract) -> Vec<&mut TlvStream> {
    match contract {
        Contract::Offered(o) | Contract::Rejected(o) => vec![&mut o.tlvs],
        Contract::Accepted(a) => vec![&mut a.offered_contract.tlvs, &mut a.tlvs],
        Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => vec![
            &mut s.accepted_contract.offered_contract.tlvs,
            &mut s.accepted_contract.tlvs,
            &mut s.tlvs,
        ],
        Contract::PreClosed(p) => vec![
            &mut p.signed_contract.accepted_contract.offered_contract.tlvs,
            &mut p.signed_contract.accepted_contract.tlvs,
            &mut p.signed_contract.tlvs,
        ],
        Contract::FailedAccept(f) => vec![&mut f.offered_contract.tlvs],
        Contract::FailedSign(f) => vec![
            &mut f.accepted_contract.offered_contract.tlvs,
            &mut f.accepted_contract.tlvs,
        ],
        Contract::Closed(_) => vec![],
    }
}

pub fn serialize_contract(contract: &Contract) -> Result<Vec<u8>, Error> {
    let serialized = match contract {
        Contract::Offered(o) | Contract::Rejected(o) => o.serialize(),
        Contract::Accepted(o) => o.serialize(),
        Contract::Signed(o) | Contract::Confirmed(o) | Contract::Refunded(o) => o.serialize(),
        Contract::FailedAccept(c) => c.serialize(),
        Contract::FailedSign(c) => c.serialize(),
        Contract::PreClosed(c) => c.serialize(),
        Contract::Closed(c) => c.serialize(),
    };
    let mut serialized = serialized.map_err(to_storage_error)?;
    let mut res = Vec::with_capacity(serialized.len() + 1);
    res.push(ContractPrefix::get_prefix(contract));
    res.append(&mut serialized);
    // Only written when a stream has records, so a contract without records
    // keeps the same bytes as before.
    let streams = tlv_streams(contract);
    if streams.iter().any(|s| !s.is_empty()) {
        res.push(TLV_SUFFIX_VERSION);
        for stream in streams {
            let bytes = stream.encode();
            BigSize(bytes.len() as u64)
                .write(&mut res)
                .map_err(to_storage_error)?;
            res.extend_from_slice(&bytes);
        }
    }
    Ok(res)
}

pub fn deserialize_contract(buff: &Vec<u8>) -> Result<Contract, Error> {
    let mut cursor = ::lightning::io::Cursor::new(buff);
    let mut prefix = [0u8; 1];
    cursor.read_exact(&mut prefix)?;
    let contract_prefix: ContractPrefix = prefix[0].try_into()?;
    let contract = match contract_prefix {
        ContractPrefix::Offered => {
            Contract::Offered(OfferedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ContractPrefix::Accepted => Contract::Accepted(
            AcceptedContract::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ContractPrefix::Signed => {
            Contract::Signed(SignedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ContractPrefix::Confirmed => {
            Contract::Confirmed(SignedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ContractPrefix::PreClosed => Contract::PreClosed(
            PreClosedContract::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ContractPrefix::Closed => {
            Contract::Closed(ClosedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ContractPrefix::FailedAccept => Contract::FailedAccept(
            FailedAcceptContract::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ContractPrefix::FailedSign => Contract::FailedSign(
            FailedSignContract::deserialize(&mut cursor).map_err(to_storage_error)?,
        ),
        ContractPrefix::Refunded => {
            Contract::Refunded(SignedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
        ContractPrefix::Rejected => {
            Contract::Rejected(OfferedContract::deserialize(&mut cursor).map_err(to_storage_error)?)
        }
    };
    let mut contract = contract;
    if (cursor.position() as usize) < buff.len() {
        let mut version = [0u8; 1];
        cursor.read_exact(&mut version)?;
        if version[0] != TLV_SUFFIX_VERSION {
            return Err(Error::StorageError(format!(
                "unknown contract TLV suffix version {}",
                version[0]
            )));
        }
        for stream in tlv_streams_mut(&mut contract) {
            let len: BigSize = Readable::read(&mut cursor).map_err(to_storage_error)?;
            let mut frame = FixedLengthReader::new(&mut cursor, len.0);
            *stream = TlvStream::read_to_end(&mut frame).map_err(to_storage_error)?;
        }
    }
    Ok(contract)
}

pub fn message_variant_name(message: &Message) -> String {
    let str = match message {
        Message::Accept(_) => "Accept",
        Message::Offer(_) => "Offer",
        Message::Sign(_) => "Sign",
        Message::Reject(_) => "Reject",
        _ => "Channel Related",
    };

    str.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream holding one record of type 65007 with `body` as its one-byte body.
    fn stream_with_record(body: u8) -> TlvStream {
        let bytes = [0xfd, 0xfd, 0xef, 0x01, body];
        TlvStream::read_to_end(&mut lightning::io::Cursor::new(bytes)).unwrap()
    }

    /// Records set on a stored contract come back on the struct they were set
    /// on. The fixtures predate the suffix, so `stored_contracts_round_trip_byte_for_byte`
    /// also proves that contracts stored without a suffix still load and write
    /// the same bytes back.
    #[test]
    fn records_survive_contract_storage() {
        let stored = include_bytes!("../../../testconfig/contract_binaries/Signed");
        let mut contract = deserialize_contract(&stored.to_vec()).unwrap();
        {
            let Contract::Signed(s) = &mut contract else {
                panic!("fixture is not a signed contract")
            };
            assert!(s.tlvs.is_empty());
            assert!(s.accepted_contract.tlvs.is_empty());
            assert!(s.accepted_contract.offered_contract.tlvs.is_empty());
            s.accepted_contract.offered_contract.tlvs = stream_with_record(1);
            s.accepted_contract.tlvs = stream_with_record(2);
            s.tlvs = stream_with_record(3);
        }

        let serialized = serialize_contract(&contract).unwrap();
        let Contract::Signed(read) = deserialize_contract(&serialized).unwrap() else {
            panic!("state changed in storage")
        };

        assert_eq!(
            read.accepted_contract.offered_contract.tlvs,
            stream_with_record(1)
        );
        assert_eq!(read.accepted_contract.tlvs, stream_with_record(2));
        assert_eq!(read.tlvs, stream_with_record(3));
    }

    /// An unknown suffix version returns an error instead of misreading the data.
    #[test]
    fn unknown_tlv_suffix_version_is_rejected() {
        let mut stored = include_bytes!("../../../testconfig/contract_binaries/Offered").to_vec();
        stored.push(99);
        assert!(deserialize_contract(&stored).is_err());
    }

    /// Every contract state, serialized by an earlier release and checked in.
    ///
    /// Each embeds at least one [`ddk_messages::oracle_msgs::OracleAnnouncement`],
    /// which is what makes these the evidence that matters for oracle serialization.
    const FIXTURES: &[(&str, &[u8])] = &[
        (
            "Offered",
            include_bytes!("../../../testconfig/contract_binaries/Offered"),
        ),
        (
            "Accepted",
            include_bytes!("../../../testconfig/contract_binaries/Accepted"),
        ),
        (
            "Signed",
            include_bytes!("../../../testconfig/contract_binaries/Signed"),
        ),
        (
            "Confirmed",
            include_bytes!("../../../testconfig/contract_binaries/Confirmed"),
        ),
        (
            "PreClosed",
            include_bytes!("../../../testconfig/contract_binaries/PreClosed"),
        ),
        (
            "Closed",
            include_bytes!("../../../testconfig/contract_binaries/Closed"),
        ),
    ];

    /// Stored contracts must survive a read and a write with every byte intact.
    ///
    /// A contract carries its oracle announcements in the embedded body form, so any
    /// change to how those are written shows up here as a diff against bytes an
    /// earlier release produced. This is what a stored `BYTEA` column would have to
    /// be migrated for; while it passes, there is nothing to migrate.
    #[test]
    fn stored_contracts_round_trip_byte_for_byte() {
        for (state, stored) in FIXTURES {
            let contract = deserialize_contract(&stored.to_vec())
                .unwrap_or_else(|e| panic!("{state} contract failed to deserialize: {e:?}"));

            let reserialized = serialize_contract(&contract)
                .unwrap_or_else(|e| panic!("{state} contract failed to serialize: {e:?}"));

            assert_eq!(
                reserialized,
                stored.to_vec(),
                "{state} contract did not round trip; the storage format changed"
            );
        }
    }

    /// The oldest offered contract we keep a fixture for still reads.
    ///
    /// Only `old/Offered` is asserted. The other fixtures under `old/` predate
    /// unrelated changes to the contract structs and have not deserialized for
    /// some time, which is a separate matter from how oracle messages are written.
    #[test]
    fn oldest_offered_contract_still_deserializes() {
        let stored = include_bytes!("../../../testconfig/contract_binaries/old/Offered");
        deserialize_contract(&stored.to_vec()).expect("oldest offered contract to deserialize");
    }

    /// The fixtures predate ddk tracking the chain hash, so they carry none.
    ///
    /// This is what makes `stored_contracts_round_trip_byte_for_byte` above
    /// meaningful for the chain hash: a contract with no stored chain hash
    /// writes exactly the bytes it was read from, so upgrading ddk leaves
    /// contracts already in a database untouched.
    #[test]
    fn stored_contracts_predating_the_chain_hash_carry_none() {
        for (state, stored) in FIXTURES {
            let contract = deserialize_contract(&stored.to_vec())
                .unwrap_or_else(|e| panic!("{state} contract failed to deserialize: {e:?}"));

            let offered = match &contract {
                Contract::Offered(o) | Contract::Rejected(o) => o,
                Contract::Accepted(a) => &a.offered_contract,
                Contract::Signed(s) | Contract::Confirmed(s) | Contract::Refunded(s) => {
                    &s.accepted_contract.offered_contract
                }
                Contract::PreClosed(p) => &p.signed_contract.accepted_contract.offered_contract,
                Contract::Closed(c) => &c.signed_contract.accepted_contract.offered_contract,
                Contract::FailedAccept(f) => &f.offered_contract,
                Contract::FailedSign(f) => &f.accepted_contract.offered_contract,
            };
            assert_eq!(
                offered.chain_hash, None,
                "{state} fixture unexpectedly carries a chain hash"
            );
        }
    }

    /// The announcement inside a stored contract is the one the oracle signed.
    ///
    /// Reading it out and writing it back as a standalone TLV record must reproduce
    /// the hex an oracle would serve, which is the round trip consumers were doing
    /// by hand.
    #[test]
    fn announcement_inside_a_stored_contract_survives_as_a_standalone_record() {
        use ddk_messages::TlvRecord;

        let stored = include_bytes!("../../../testconfig/contract_binaries/Offered");
        let Contract::Offered(offered) = deserialize_contract(&stored.to_vec()).unwrap() else {
            panic!("fixture is not an offered contract");
        };

        let announcement = &offered.contract_info[0].oracle_announcements[0];
        let tlv = announcement.to_tlv_bytes();

        assert_eq!(
            ddk_messages::oracle_msgs::OracleAnnouncement::from_tlv_bytes(&tlv).unwrap(),
            *announcement
        );
    }
}
