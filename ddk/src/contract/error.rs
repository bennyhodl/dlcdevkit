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
