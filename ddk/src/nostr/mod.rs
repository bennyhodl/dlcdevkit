//! # Bitcoin and Nostr Public Key Conversions
//!
//! This module handles the conversion between Bitcoin and Nostr public keys, which is necessary
//! because DLC (Discreet Log Contract) operations use Bitcoin's secp256k1 keys while Nostr
//! protocol communication uses its own public key format.
//!
//! ## Key Differences
//! - Bitcoin public keys are typically 33-byte compressed keys with a prefix byte (0x02 or 0x03)
//! - Nostr public keys are 32-byte x-only public keys (just the x coordinate)
//!
//! The conversion process involves handling these format differences while preserving the
//! cryptographic properties of the keys.

pub mod messages;

use bitcoin::key::Parity;
use bitcoin::secp256k1::PublicKey as BitcoinPublicKey;
use nostr_rs::{Kind, PublicKey};

/// Event kind for DLC protocol messages (NIP-88)
pub const DLC_MESSAGE_KIND: Kind = Kind::Custom(8_888);

/// Event kind for oracle announcements (NIP-88)
pub const ORACLE_ANNOUNCMENT_KIND: Kind = Kind::Custom(88);

/// Event kind for oracle attestations (NIP-88)
pub const ORACLE_ATTESTATION_KIND: Kind = Kind::Custom(89);

/// Converts a Bitcoin public key to a Nostr public key.
///
/// This conversion is necessary when we need to communicate DLC-related data over Nostr.
/// The function extracts the x-only public key from the Bitcoin public key format,
/// discarding the y-coordinate parity information.
///
/// # Arguments
/// * `bitcoin_pk` - A Bitcoin secp256k1 public key (33 bytes, compressed format)
///
/// # Returns
/// * `PublicKey` - A Nostr public key (32 bytes, x-only format)
///
/// # Panics
/// * If the Bitcoin public key cannot be converted to a Nostr key format
pub fn bitcoin_to_nostr_pubkey(bitcoin_pk: &BitcoinPublicKey) -> PublicKey {
    // Convert to XOnlyPublicKey first
    let (xonly, _parity) = bitcoin_pk.x_only_public_key();

    // Create nostr public key from the x-only bytes
    PublicKey::from_slice(xonly.serialize().as_slice())
        .expect("Could not convert Bitcoin key to nostr key.")
}

/// Converts a Nostr public key to a Bitcoin public key.
///
/// This conversion is needed when receiving Nostr messages that need to be used in DLC operations.
/// Since Nostr keys are x-only, we assume even y-coordinate parity when reconstructing
/// the Bitcoin public key.
///
/// # Arguments
/// * `nostr_pk` - A Nostr public key (32 bytes, x-only format)
///
/// # Returns
/// * `BitcoinPublicKey` - A Bitcoin secp256k1 public key (33 bytes, compressed format)
///
/// # Panics
/// * If the Nostr key cannot be converted to an x-only format
///
/// # Note
/// The function always assumes even y-coordinate parity when reconstructing the Bitcoin public key.
/// This is sufficient for DLC operations as the actual parity is handled within the DLC protocol.
pub fn nostr_to_bitcoin_pubkey(nostr_pk: &PublicKey) -> BitcoinPublicKey {
    let xonly = nostr_pk.xonly().expect("Could not get xonly public key.");
    BitcoinPublicKey::from_x_only_public_key(xonly, Parity::Even)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_nostr_to_bitcoin_pubkey() {
        let nostr_pk = "7622b0ca2b5ad4d7441784a97bfc50c69d09853a640ad793a4fb9d238c7e0b15";
        let bitcoin_pk = "027622b0ca2b5ad4d7441784a97bfc50c69d09853a640ad793a4fb9d238c7e0b15";
        let nostr_pk_2 = bitcoin_to_nostr_pubkey(&BitcoinPublicKey::from_str(bitcoin_pk).unwrap());
        assert_eq!(nostr_pk_2.to_string(), nostr_pk);

        let nostr = PublicKey::from_str(nostr_pk).unwrap();
        let btc_pk = nostr_to_bitcoin_pubkey(&nostr);
        assert_eq!(btc_pk.to_string(), bitcoin_pk);
    }
}
