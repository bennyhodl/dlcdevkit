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
use dlc_messages::Message;
use lightning::io::Read;

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
