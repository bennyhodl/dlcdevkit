//! Errors returned by the stateless contract API.

use thiserror::Error;

/// Errors returned by the stateless contract functions.
///
/// Variants carry plain strings so they can cross FFI and binding boundaries
/// without exposing internal library error types.
#[derive(Debug, Error)]
pub enum ContractError {
    /// The offer message, or data used to build one, is invalid.
    #[error("invalid offer: {0}")]
    InvalidOffer(String),
    /// The accept message, or data used to build one, is invalid.
    #[error("invalid accept: {0}")]
    InvalidAccept(String),
    /// The sign message, or data used to build one, is invalid.
    #[error("invalid sign: {0}")]
    InvalidSign(String),
    /// A funding input is malformed or references missing data.
    #[error("invalid funding input: {0}")]
    InvalidFundingInput(String),
    /// The PSBT does not match the funding transaction rebuilt from the wire messages.
    #[error("PSBT mismatch: {0}")]
    PsbtMismatch(String),
    /// A PSBT input that must be signed does not have a finalized witness.
    #[error("PSBT input {input_index} does not have a finalized witness")]
    MissingFinalizedInput {
        /// The index of the input in the funding transaction.
        input_index: usize,
    },
    /// The script type of a funding input is not supported for signing.
    #[error("PSBT input {input_index} has an unsupported script type")]
    UnsupportedScriptType {
        /// The index of the input in the funding transaction.
        input_index: usize,
    },
    /// A funding witness spends a script type DDK cannot verify (see
    /// [`ddk_dlc::verify_funding_witness`]), so the contract cannot be
    /// verified as a whole.
    #[error(
        "the funding witness of input serial id {input_serial_id} spends a script type that \
         cannot be verified"
    )]
    UnverifiableFundingWitness {
        /// The serial id of the funding input.
        input_serial_id: u64,
    },
    /// A funding witness is longer than the `max_witness_len` its input
    /// declared. The funding fee was computed from the declared length, so the
    /// longer witness pays less than the agreed fee rate.
    #[error(
        "the funding witness of input serial id {input_serial_id} is {witness_len} bytes, over \
         its declared maximum of {max_witness_len}"
    )]
    FundingWitnessTooLong {
        /// The serial id of the funding input.
        input_serial_id: u64,
        /// The serialized size of the witness.
        witness_len: usize,
        /// The size the funding input declared.
        max_witness_len: usize,
    },
    /// A signature produced for this party, by a signer outside the process
    /// or with a key passed in, is missing, does not verify against the
    /// transaction it was requested for, uses a sighash other than
    /// `SIGHASH_ALL`, or comes in the wrong number.
    #[error("invalid signature: {0}")]
    InvalidSignature(String),
    /// An oracle attestation is malformed, forged, or does not correspond to
    /// the announcement of the oracle it claims to come from.
    #[error("invalid attestation: {0}")]
    InvalidAttestation(String),
    /// No contract outcome corresponds to the supplied oracle attestations, so
    /// there is no CET to sign.
    #[error("no contract outcome matches the given attestations")]
    NoMatchingOutcome,
    /// Descriptor parsing, derivation, or signing failed.
    #[error("descriptor error: {0}")]
    Descriptor(String),
    /// A wallet implementation failed to sign a funding input.
    #[error("wallet error: {0}")]
    Wallet(String),
    /// BIP32 key derivation failed.
    #[error("BIP32 error: {0}")]
    Bip32(String),
    /// A DLC transaction or signature operation failed.
    #[error("DLC error: {0}")]
    Dlc(String),
    /// Contract funding-key derivation failed (bad mnemonic, missing private
    /// key, or an invalid derived key).
    #[error("contract key error: {0}")]
    Key(String),
}

impl From<bitcoin::bip32::Error> for ContractError {
    fn from(error: bitcoin::bip32::Error) -> Self {
        ContractError::Bip32(error.to_string())
    }
}

impl From<ddk_dlc::Error> for ContractError {
    fn from(error: ddk_dlc::Error) -> Self {
        ContractError::Dlc(error.to_string())
    }
}

impl From<ddk_manager::error::Error> for ContractError {
    fn from(error: ddk_manager::error::Error) -> Self {
        ContractError::Dlc(error.to_string())
    }
}

impl From<ddk_manager::contract::transactions::BuildError> for ContractError {
    fn from(error: ddk_manager::contract::transactions::BuildError) -> Self {
        use ddk_manager::contract::transactions::BuildError;
        match error {
            BuildError::Offer(message) => ContractError::InvalidOffer(message),
            BuildError::Accept(message) => ContractError::InvalidAccept(message),
            BuildError::Construction(error) => error.into(),
        }
    }
}
