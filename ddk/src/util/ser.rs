use ddk_manager::channel::signed_channel::SignedChannelStateType;
use ddk_manager::channel::Channel;
use ddk_manager::contract::Contract;
use ddk_manager::error::Error;
use ddk_messages::Message;

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

pub use ddk_manager::contract::ser::ContractPrefix;

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

pub fn serialize_contract(contract: &Contract) -> Result<Vec<u8>, Error> {
    contract.serialize()
}

pub fn deserialize_contract(buff: &[u8]) -> Result<Contract, Error> {
    Contract::deserialize(buff)
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
    use ddk_messages::tlv_stream::TlvStream;

    /// A stream holding one record of type 65007 with `body` as its one-byte body.
    fn stream_with_record(body: u8) -> TlvStream {
        let bytes = [0xfd, 0xfd, 0xef, 0x01, body];
        TlvStream::read_to_end(&mut lightning::io::Cursor::new(bytes)).unwrap()
    }

    /// Records set on a stored contract come back on the struct they were set on.
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

    /// An unknown blob version returns an error instead of misreading the data.
    #[test]
    fn unknown_stored_contract_version_is_rejected() {
        let legacy = include_bytes!("../../../testconfig/contract_binaries/Offered");
        let mut stored = vec![0u8, 99];
        stored.extend_from_slice(legacy);
        assert!(deserialize_contract(&stored).is_err());
    }

    /// A closed contract wraps the signed one, and its streams must survive
    /// storage like every other state's.
    #[test]
    fn closed_contract_keeps_its_streams() {
        let stored = include_bytes!("../../../testconfig/contract_binaries/Closed");
        let mut contract = deserialize_contract(&stored.to_vec()).unwrap();
        {
            let Contract::Closed(c) = &mut contract else {
                panic!("fixture is not a closed contract")
            };
            c.signed_contract.accepted_contract.offered_contract.tlvs = stream_with_record(1);
            c.signed_contract.accepted_contract.tlvs = stream_with_record(2);
            c.signed_contract.tlvs = stream_with_record(3);
        }

        let serialized = serialize_contract(&contract).unwrap();
        let Contract::Closed(read) = deserialize_contract(&serialized).unwrap() else {
            panic!("state changed in storage")
        };

        assert_eq!(
            read.signed_contract.accepted_contract.offered_contract.tlvs,
            stream_with_record(1)
        );
        assert_eq!(
            read.signed_contract.accepted_contract.tlvs,
            stream_with_record(2)
        );
        assert_eq!(read.signed_contract.tlvs, stream_with_record(3));
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

    /// Contracts stored by an earlier release load, and writing them back
    /// only upgrades the envelope: the struct bytes stay intact.
    ///
    /// A contract carries its oracle announcements in the embedded body form, so any
    /// change to how those are written shows up here as a diff against bytes an
    /// earlier release produced. The new envelope is the marker and version
    /// byte up front and one empty length-framed stream per message at the
    /// end, which every release from here on can read.
    #[test]
    fn stored_contracts_round_trip_with_struct_bytes_intact() {
        for (state, stored) in FIXTURES {
            let contract = deserialize_contract(&stored.to_vec())
                .unwrap_or_else(|e| panic!("{state} contract failed to deserialize: {e:?}"));

            let reserialized = serialize_contract(&contract)
                .unwrap_or_else(|e| panic!("{state} contract failed to serialize: {e:?}"));

            let struct_end = 3 + stored.len() - 1;
            assert_eq!(&reserialized[..2], &[0, 1], "{state}: marker and version");
            assert_eq!(reserialized[2], stored[0], "{state}: state prefix");
            assert_eq!(
                &reserialized[3..struct_end],
                &stored[1..],
                "{state} struct bytes changed; stored contracts would need a migration"
            );
            assert!(
                reserialized[struct_end..].iter().all(|b| *b == 0),
                "{state}: trailing streams should be empty"
            );

            let reread = deserialize_contract(&reserialized)
                .unwrap_or_else(|e| panic!("{state} upgraded blob failed to deserialize: {e:?}"));
            assert_eq!(
                serialize_contract(&reread).unwrap(),
                reserialized,
                "{state} upgraded blob is not stable"
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
    /// This is what makes `stored_contracts_round_trip_with_struct_bytes_intact`
    /// above meaningful for the chain hash: a contract with no stored chain
    /// hash writes exactly the struct bytes it was read from, so upgrading ddk
    /// does not disturb the contract data already in a database.
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
