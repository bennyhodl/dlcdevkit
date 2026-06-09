//! Stateless DLC contract lifecycle.
//!
//! This module completes a DLC using only wire messages, explicit party data,
//! and PSBTs. There is no contract manager, no persisted contract state, no
//! storage backend, and no blockchain client: every operation rebuilds and
//! validates what it needs from the [`OfferDlc`](ddk_messages::OfferDlc) and
//! [`AcceptDlc`](ddk_messages::AcceptDlc) messages, which are the
//! authoritative state.
//!
//! # Lifecycle
//!
//! ```text
//! offer party                                accept party
//! -----------                                ------------
//! create_offer ──────────── OfferDlc ──────► accept_offer ─┐
//!                                                          │ AcceptResult
//! ┌──────────────────────── AcceptDlc ◄─────────────────────┘
//! │ create_funding_psbt
//! │ sign own inputs (signing::*)
//! │ sign_accept ──────────── SignDlc ──────► create_funding_psbt
//! │                                          sign own inputs (signing::*)
//! │                                          finalize_sign ──► Transaction
//! │                                          broadcast via chain client
//! ```
//!
//! Between messages each party only needs to retain:
//!
//! | Party | After | Must retain |
//! |-------|-------|-------------|
//! | offer | `create_offer` | the `OfferDlc`, its DLC funding secret key, and access to the keys of its funding inputs |
//! | accept | `accept_offer` | the `OfferDlc`, the `AcceptDlc`, its DLC funding secret key, and access to the keys of its funding inputs |
//! | offer | `sign_accept` | nothing further; the CETs and refund transaction are rebuilt from the messages whenever needed |
//!
//! # PSBT as the signing boundary
//!
//! Funding inputs are regular wallet UTXOs, and wallets speak PSBT. The
//! funding PSBT built by [`create_funding_psbt`](crate::contract::create_funding_psbt) carries everything a signer
//! needs (`witness_utxo`, `non_witness_utxo`, redeem scripts, sighash type)
//! and never contains private key material. [`sign_accept`](crate::contract::sign_accept) and
//! [`finalize_sign`](crate::contract::finalize_sign) verify that a returned PSBT spends exactly the funding
//! transaction rebuilt from the messages — input count, outpoints, outputs,
//! locktime, and sequences — before extracting witnesses, so a signer cannot
//! mutate the transaction.
//!
//! Four funding sources produce those witnesses through the same lifecycle
//! (see [`signing`](crate::contract::signing)):
//!
//! | Source | How |
//! |--------|-----|
//! | DDK wallet | [`signing::sign_funding_psbt_with_wallet`](crate::contract::signing::sign_funding_psbt_with_wallet) with any [`ddk_manager::Wallet`] |
//! | Raw xpriv | [`signing::sign_funding_psbt_with_xpriv`](crate::contract::signing::sign_funding_psbt_with_xpriv) with per-input BIP32 paths |
//! | Private descriptor | [`signing::sign_funding_psbt_with_descriptor`](crate::contract::signing::sign_funding_psbt_with_descriptor) with per-input indexes |
//! | External / hardware signer | serialize the PSBT, sign and finalize externally, deserialize |
//!
//! # DLC funding keys versus wallet input keys
//!
//! Each party uses two kinds of keys. The *DLC funding key*
//! ([`PartyParams::funding_pubkey`](crate::contract::PartyParams::funding_pubkey) and the `funding_secret_key` arguments) is
//! a single secp256k1 key that controls the 2-of-2 funding output, the CET
//! adaptor signatures, and the refund signature. The *wallet input keys*
//! control the UTXOs spent into the funding transaction and never touch DLC
//! cryptography — they only sign the funding PSBT. A hardware wallet can hold
//! the input keys (PSBT exchange) while the application holds the DLC funding
//! key.
//!
//! # Script support
//!
//! Built-in signers support native P2WPKH and P2SH-P2WPKH funding inputs;
//! descriptor signing supports `wpkh()` and `sh(wpkh())`, with or without a
//! wildcard. Unsupported script types fail with
//! [`ContractError::UnsupportedScriptType`](crate::contract::ContractError::UnsupportedScriptType) rather than producing incomplete
//! signatures. External signers can fund with any script type they can
//! finalize themselves.
//!
//! # Broadcasting and storage stay with the caller
//!
//! [`finalize_sign`](crate::contract::finalize_sign) returns a fully signed [`bitcoin::Transaction`];
//! broadcast it with the chain client of your choice (for example
//! [`ddk_manager::Blockchain::send_transaction`] implemented by
//! [`crate::chain::EsploraClient`]). Persisting messages for later execution
//! is likewise the caller's responsibility.
//!
//! Lower-level operations (raw witnesses, adaptor signatures, contract ids)
//! live in [`advanced`](crate::contract::advanced).

pub mod advanced;
pub mod signing;

mod accept;
mod context;
mod create;
mod error;
mod finalize;
mod psbt;
mod sign;
mod types;

#[cfg(test)]
mod tests;

pub use accept::{accept_offer, create_dlc_transactions};
pub use create::{create_offer, validate_offer};
pub use error::ContractError;
pub use finalize::finalize_sign;
pub use psbt::create_funding_psbt;
pub use sign::sign_accept;
pub use types::{
    chain_hash_from_network, funding_input, AcceptOfferParams, AcceptResult, CreateOfferParams,
    DescriptorInput, InputDerivation, Party, PartyParams, SignResult,
};

/// The current DLC protocol version used by DDK.
pub const PROTOCOL_VERSION: u32 = 1;
