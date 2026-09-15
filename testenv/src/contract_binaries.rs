//! Contracts serialized by an earlier release and checked in.
//!
//! Every state a contract passes through is stored once under
//! `testenv/contract_binaries/`, each written by `ddk`'s enumeration test with
//! `GENERATE_SERIALIZED_CONTRACT` set (see
//! `testenv/scripts/generate_enumeration_contract_binaries.sh`). Tests use
//! them as the evidence that stored contracts still load: a change that breaks
//! one of these breaks every database written before it.
//!
//! The bytes are compiled into this crate, so a test needs no path to the
//! workspace to reach them:
//!
//! ```no_run
//! use ddk_testenv::ContractBinary;
//!
//! let bytes = ddk_testenv::contract_binary(ContractBinary::Signed);
//! ```

use std::path::PathBuf;

/// The state a checked-in contract was serialized in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContractBinary {
    Offered,
    Accepted,
    Signed,
    Confirmed,
    PreClosed,
    Closed,
}

impl ContractBinary {
    /// Every state, in the order a contract moves through them.
    pub const ALL: [ContractBinary; 6] = [
        ContractBinary::Offered,
        ContractBinary::Accepted,
        ContractBinary::Signed,
        ContractBinary::Confirmed,
        ContractBinary::PreClosed,
        ContractBinary::Closed,
    ];

    /// The file name under `contract_binaries/`, which is also the name of the
    /// `Contract` variant the bytes deserialize to.
    pub fn name(self) -> &'static str {
        match self {
            ContractBinary::Offered => "Offered",
            ContractBinary::Accepted => "Accepted",
            ContractBinary::Signed => "Signed",
            ContractBinary::Confirmed => "Confirmed",
            ContractBinary::PreClosed => "PreClosed",
            ContractBinary::Closed => "Closed",
        }
    }
}

/// The contract in `state`, as the current format stores it.
pub fn contract_binary(state: ContractBinary) -> &'static [u8] {
    match state {
        ContractBinary::Offered => include_bytes!("../contract_binaries/Offered"),
        ContractBinary::Accepted => include_bytes!("../contract_binaries/Accepted"),
        ContractBinary::Signed => include_bytes!("../contract_binaries/Signed"),
        ContractBinary::Confirmed => include_bytes!("../contract_binaries/Confirmed"),
        ContractBinary::PreClosed => include_bytes!("../contract_binaries/PreClosed"),
        ContractBinary::Closed => include_bytes!("../contract_binaries/Closed"),
    }
}

/// An offered contract stored before `contract_flags` existed.
///
/// One byte shorter than [`ContractBinary::Offered`], and the evidence that a
/// database written before the flags byte still loads.
pub fn legacy_contract_binary() -> &'static [u8] {
    include_bytes!("../contract_binaries/legacy/Offered")
}

/// The directory the current-format binaries live in, for the generator that
/// writes new ones.
pub fn contract_binaries_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("contract_binaries")
}
