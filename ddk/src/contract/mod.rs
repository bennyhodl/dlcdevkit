//! Stateless DLC contract lifecycle.
//!
//! This module completes a DLC using only wire messages, explicit party data,
//! and PSBTs. There is no contract manager, no persisted contract state, no
//! storage backend, and no blockchain client: every operation rebuilds and
//! validates what it needs from the [`OfferDlc`](ddk_messages::OfferDlc),
//! [`AcceptDlc`](ddk_messages::AcceptDlc), and [`SignDlc`](ddk_messages::SignDlc)
//! messages, which are the authoritative state.
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
//! │
//! │ ... the oracles attest, or the refund locktime passes ...
//! │
//! └─ sign_cet / sign_refund ──► Transaction   sign_cet / sign_refund ──► Transaction
//!    broadcast via chain client               broadcast via chain client
//! ```
//!
//! Either party can settle on its own, and neither needs the other's
//! cooperation to do it: the counterparty's half of the 2-of-2 spend was
//! committed in the messages it already sent.
//!
//! Each party retains only the three wire messages, its DLC funding secret key,
//! and access to the keys of its funding inputs. Everything else — the funding
//! transaction, the CETs, the refund transaction, the contract id, the adaptor
//! information — is rebuilt from those messages whenever it is needed.
//!
//! The `SignDlc` matters to each side differently: the accepting party settles
//! with the signatures it carries, while for the offering party it only
//! confirms that the three messages describe one contract.
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
//! # Splicing
//!
//! A new contract can spend a previous contract's 2-of-2 funding output as an
//! input (a *splice*), which is how rollovers and collateral changes are
//! expressed. Only the offering party may contribute a splice input. Build it
//! from the previous contract's messages with
//! [`create_dlc_splice_input`](crate::contract::create_dlc_splice_input) and
//! place it in the offering party's funding inputs. Signing the prior 2-of-2
//! additionally requires each party's *previous-contract* funding secret key,
//! supplied to [`sign_accept_spliced`](crate::contract::sign_accept_spliced)
//! (offering party) and
//! [`finalize_sign_spliced`](crate::contract::finalize_sign_spliced) (accepting
//! party) as [`DlcInputSigningKey`](crate::contract::DlcInputSigningKey) values.
//!
//! # Settlement
//!
//! A funded contract ends in one of two transactions, both of which spend the
//! 2-of-2 funding output and are built entirely from the wire messages:
//!
//! | Outcome | Function | Counterparty's half comes from |
//! |---------|----------|-------------------------------|
//! | the oracles attest | [`sign_cet`](crate::contract::sign_cet) | its CET adaptor signature, decrypted with the oracle signatures |
//! | nobody attests | [`sign_refund`](crate::contract::sign_refund) | its refund signature, sent with the accept or sign message |
//!
//! Both take the settling party's DLC funding secret key, which supplies this
//! party's half of the 2-of-2 spend and identifies which side is settling — so
//! the same call works for either party. [`sign_cet`](crate::contract::sign_cet)
//! additionally takes the oracle attestations, each paired with the index of
//! its oracle in the contract's announcements; it selects the matching CET,
//! verifies the attestations against the announcements they claim to come from,
//! and returns the signed transaction.
//!
//! Neither function enforces *when* a transaction may be broadcast. CETs carry
//! the offer's `cet_locktime` and the refund its `refund_locktime`; the chain
//! enforces those, and deciding which settlement path to take is the caller's
//! policy.
//!
//! Settling is the most expensive operation in the module: selecting a CET
//! means reconstructing the contract's adaptor information, which for a
//! large numeric contract is the same order of work as accepting it. That is
//! the cost of keeping no state.
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
mod keys;
mod psbt;
mod settle;
mod sign;
mod splice;
mod types;

#[cfg(test)]
mod tests;

pub use accept::{accept_offer, create_dlc_transactions};
pub use create::{create_offer, validate_offer};
pub use error::ContractError;
pub use finalize::{finalize_sign, finalize_sign_spliced};
pub use keys::ContractKeyProvider;
pub use psbt::create_funding_psbt;
pub use settle::{sign_cet, sign_refund};
pub use sign::{sign_accept, sign_accept_spliced};
pub use splice::{create_dlc_splice_input, DLC_INPUT_MAX_WITNESS_LEN};
pub use types::{
    chain_hash_from_network, funding_input, AcceptOfferParams, AcceptResult, CreateOfferParams,
    DescriptorInput, DlcInputSigningKey, InputDerivation, Party, PartyParams, SignResult,
};

/// The current DLC protocol version used by DDK.
pub const PROTOCOL_VERSION: u32 = 1;
