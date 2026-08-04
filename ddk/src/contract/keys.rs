//! Deterministic contract funding-key derivation.
//!
//! A DLC contract's funding key (the key controlling its 2-of-2 output, its CET
//! adaptor signatures, and its refund signature) is a pure, deterministic
//! function of a `keys_id`, which is itself a pure function of the contract's
//! temporary id. Nothing is stored: given a master extended private key and a
//! contract's temporary id, the exact funding key is recomputed on demand.
//!
//! This is the mechanism the stateless splice API needs. To spend a previous
//! contract's funding output the caller must supply that contract's funding
//! secret key ([`DlcInputSigningKey`]); [`ContractKeyProvider::dlc_input_signing_key`]
//! re-derives it from the previous contract's temporary id.
//!
//! [`ContractKeyProvider`] is the standalone form of the derivation implemented
//! by [`crate::wallet::DlcDevKitWallet`], which delegates to it — so keys are
//! interchangeable between the stateful manager path and the stateless API.
//! It implements [`ddk_manager::ContractSignerProvider`], so it can also drive a
//! manager directly.

use std::str::FromStr;

use bdk_wallet::miniscript::descriptor::{Descriptor, DescriptorPublicKey, DescriptorSecretKey};
use bitcoin::bip32::{ChildNumber, DerivationPath, Fingerprint, Xpriv};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{All, PublicKey, Secp256k1, SecretKey};
use bitcoin::Network;
use ddk_manager::SimpleSigner;

use super::error::ContractError;
use super::types::DlcInputSigningKey;

/// Range of child numbers per hierarchical level. `3400^3 ≈ 39.3` billion paths
/// — large enough to avoid collisions across millions of contracts, small
/// enough for practical disaster recovery.
const CHILD_NUMBER_RANGE: u32 = 3_400;

/// Base derivation path for contract keys.
const DLC_BASE_PATH: &str = "m/420'/0'/0'";

/// Domain-separation tag for the keys-id hash. Must stay in lockstep with
/// [`crate::wallet::DlcDevKitWallet`] or keys derived by one will not match the
/// other.
const KEYS_ID_TAG: &[u8] = b"CONTRACT_SIGNER_KEY_ID_V0";

/// Deterministically derives DLC contract funding keys from a master extended
/// private key.
///
/// Construct one from whatever key material the consumer holds — a raw
/// [`Xpriv`](bitcoin::bip32::Xpriv), a BIP39 mnemonic, a seed, or an output
/// descriptor carrying a private key — and the funding keys for every contract
/// follow deterministically. The secret keys never leave this type; callers ask
/// for a [`funding_pubkey`](Self::funding_pubkey) to publish, or a
/// [`dlc_input_signing_key`](Self::dlc_input_signing_key) to splice.
#[derive(Clone)]
pub struct ContractKeyProvider {
    xprv: Xpriv,
    fingerprint: Fingerprint,
    secp: Secp256k1<All>,
    dlc_path: DerivationPath,
}

impl ContractKeyProvider {
    /// Builds a provider from a master extended private key.
    pub fn from_xprv(xprv: Xpriv) -> Self {
        let secp = Secp256k1::new();
        let fingerprint = xprv.fingerprint(&secp);
        let dlc_path = DerivationPath::from_str(DLC_BASE_PATH).expect("valid base path");
        Self {
            xprv,
            fingerprint,
            secp,
            dlc_path,
        }
    }

    /// Builds a provider from a raw seed (for example the 64 bytes produced by
    /// [`convert_mnemonic_to_seed`](Self::from_mnemonic)).
    pub fn from_seed(seed: &[u8], network: Network) -> Result<Self, ContractError> {
        let xprv = Xpriv::new_master(network, seed)
            .map_err(|e| ContractError::Key(format!("invalid seed: {e}")))?;
        Ok(Self::from_xprv(xprv))
    }

    /// Builds a provider from a BIP39 mnemonic (with optional passphrase).
    pub fn from_mnemonic(
        mnemonic: &str,
        passphrase: Option<&str>,
        network: Network,
    ) -> Result<Self, ContractError> {
        let mnemonic = bip39::Mnemonic::from_str(mnemonic)
            .map_err(|e| ContractError::Key(format!("invalid mnemonic: {e}")))?;
        let seed = mnemonic.to_seed(passphrase.unwrap_or(""));
        Self::from_seed(&seed, network)
    }

    /// Builds a provider from an output descriptor that carries an extended
    /// private key (for example `wpkh(xprv.../84h/1h/0h/0/*)`). The descriptor's
    /// extended private key is used as the master key; the descriptor's own path
    /// and wildcard are not applied to contract-key derivation. Watch-only
    /// descriptors are rejected.
    pub fn from_descriptor(descriptor: &str) -> Result<Self, ContractError> {
        let secp = Secp256k1::new();
        let (_, key_map) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor)
            .map_err(|e| ContractError::Descriptor(e.to_string()))?;
        let xprv = key_map
            .values()
            .find_map(|secret| match secret {
                DescriptorSecretKey::XPrv(xkey) => Some(xkey.xkey),
                _ => None,
            })
            .ok_or_else(|| {
                ContractError::Descriptor(
                    "descriptor does not contain an extended private key".to_string(),
                )
            })?;
        Ok(Self::from_xprv(xprv))
    }

    /// The `keys_id` for a contract, a deterministic function of its temporary id.
    pub fn keys_id(&self, temporary_contract_id: [u8; 32]) -> [u8; 32] {
        let mut input = Vec::with_capacity(4 + 32 + KEYS_ID_TAG.len());
        input.extend_from_slice(self.fingerprint.as_bytes());
        input.extend_from_slice(&temporary_contract_id);
        input.extend_from_slice(KEYS_ID_TAG);
        sha256::Hash::hash(&input).to_byte_array()
    }

    /// The funding secret key for a `keys_id`.
    pub fn funding_secret_key_for_keys_id(
        &self,
        keys_id: [u8; 32],
    ) -> Result<SecretKey, ContractError> {
        let (level_1, level_2, level_3) = hierarchical_indices(keys_id);
        let path = self.hierarchical_derivation_path(level_1, level_2, level_3)?;
        let base_key = self
            .xprv
            .derive_priv(&self.secp, &path)
            .map_err(|e| ContractError::Bip32(e.to_string()))?
            .private_key;
        self.harden(&base_key, level_1, level_2, level_3)
    }

    /// The funding secret key for a contract, from its temporary id.
    pub fn funding_secret_key(
        &self,
        temporary_contract_id: [u8; 32],
    ) -> Result<SecretKey, ContractError> {
        self.funding_secret_key_for_keys_id(self.keys_id(temporary_contract_id))
    }

    /// The funding public key for a contract — publish this in the offer or
    /// accept message ([`PartyParams::funding_pubkey`](super::PartyParams::funding_pubkey)).
    pub fn funding_pubkey(
        &self,
        temporary_contract_id: [u8; 32],
    ) -> Result<PublicKey, ContractError> {
        Ok(self
            .funding_secret_key(temporary_contract_id)?
            .public_key(&self.secp))
    }

    /// Re-derives the previous contract's funding secret key and wraps it as a
    /// [`DlcInputSigningKey`] for the splice input identified by `input_serial_id`.
    /// Pass the result to [`sign_accept_spliced`](super::sign_accept_spliced) or
    /// [`finalize_sign_spliced`](super::finalize_sign_spliced).
    pub fn dlc_input_signing_key(
        &self,
        prior_temporary_contract_id: [u8; 32],
        input_serial_id: u64,
    ) -> Result<DlcInputSigningKey, ContractError> {
        Ok(DlcInputSigningKey {
            input_serial_id,
            prior_funding_secret_key: self.funding_secret_key(prior_temporary_contract_id)?,
        })
    }

    fn hierarchical_derivation_path(
        &self,
        level_1: u32,
        level_2: u32,
        level_3: u32,
    ) -> Result<DerivationPath, ContractError> {
        let child = |index: u32| {
            ChildNumber::from_normal_idx(index)
                .map_err(|e| ContractError::Key(format!("invalid derivation index: {e}")))
        };
        Ok(self
            .dlc_path
            .extend([child(level_1)?, child(level_2)?, child(level_3)?]))
    }

    fn harden(
        &self,
        base_key: &SecretKey,
        level_1: u32,
        level_2: u32,
        level_3: u32,
    ) -> Result<SecretKey, ContractError> {
        let mut input = Vec::new();
        input.extend_from_slice(self.fingerprint.as_bytes());
        input.extend_from_slice(&base_key.secret_bytes());
        input.extend_from_slice(&level_1.to_be_bytes());
        input.extend_from_slice(&level_2.to_be_bytes());
        input.extend_from_slice(&level_3.to_be_bytes());
        SecretKey::from_slice(sha256::Hash::hash(&input).as_ref())
            .map_err(|e| ContractError::Key(format!("invalid derived key: {e}")))
    }
}

/// Splits the first 12 bytes of a `keys_id` into three level indices.
fn hierarchical_indices(keys_id: [u8; 32]) -> (u32, u32, u32) {
    let level = |offset: usize| {
        u32::from_be_bytes([
            keys_id[offset],
            keys_id[offset + 1],
            keys_id[offset + 2],
            keys_id[offset + 3],
        ]) % CHILD_NUMBER_RANGE
    };
    (level(0), level(4), level(8))
}

impl ddk_manager::ContractSignerProvider for ContractKeyProvider {
    type Signer = SimpleSigner;

    fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
        self.keys_id(temp_id)
    }

    fn derive_contract_signer(
        &self,
        key_id: [u8; 32],
    ) -> Result<Self::Signer, ddk_manager::error::Error> {
        let secret_key = self
            .funding_secret_key_for_keys_id(key_id)
            .map_err(|e| ddk_manager::error::Error::InvalidParameters(e.to_string()))?;
        Ok(SimpleSigner::new(secret_key))
    }

    fn get_secret_key_for_pubkey(
        &self,
        _pubkey: &PublicKey,
    ) -> Result<SecretKey, ddk_manager::error::Error> {
        unreachable!("get_secret_key_for_pubkey is only used for channels")
    }

    fn get_new_secret_key(&self) -> Result<SecretKey, ddk_manager::error::Error> {
        unreachable!("get_new_secret_key is only used for channels")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMP_A: [u8; 32] = [0xA1; 32];
    const TEMP_B: [u8; 32] = [0xB2; 32];
    const MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn provider() -> ContractKeyProvider {
        ContractKeyProvider::from_mnemonic(MNEMONIC, None, Network::Regtest).unwrap()
    }

    #[test]
    fn derivation_is_deterministic_and_recoverable() {
        // A fresh provider over the same key material recomputes the same key.
        let a = provider().funding_secret_key(TEMP_A).unwrap();
        let b = provider().funding_secret_key(TEMP_A).unwrap();
        assert_eq!(a, b);
        // Different contracts get different keys.
        assert_ne!(a, provider().funding_secret_key(TEMP_B).unwrap());
    }

    #[test]
    fn funding_pubkey_matches_secret_key() {
        let keys = provider();
        let secp = Secp256k1::new();
        let sk = keys.funding_secret_key(TEMP_A).unwrap();
        assert_eq!(keys.funding_pubkey(TEMP_A).unwrap(), sk.public_key(&secp));
    }

    #[test]
    fn dlc_input_signing_key_carries_the_recovered_prior_key() {
        let keys = provider();
        let signing_key = keys.dlc_input_signing_key(TEMP_A, 900).unwrap();
        assert_eq!(signing_key.input_serial_id, 900);
        assert_eq!(
            signing_key.prior_funding_secret_key,
            keys.funding_secret_key(TEMP_A).unwrap()
        );
    }

    #[test]
    fn constructors_agree() {
        let mnemonic = bip39::Mnemonic::from_str(MNEMONIC).unwrap();
        let seed = mnemonic.to_seed("");
        let from_seed = ContractKeyProvider::from_seed(&seed, Network::Regtest).unwrap();
        let from_xprv =
            ContractKeyProvider::from_xprv(Xpriv::new_master(Network::Regtest, &seed).unwrap());
        assert_eq!(
            provider().funding_secret_key(TEMP_A).unwrap(),
            from_seed.funding_secret_key(TEMP_A).unwrap()
        );
        assert_eq!(
            from_seed.funding_secret_key(TEMP_A).unwrap(),
            from_xprv.funding_secret_key(TEMP_A).unwrap()
        );
    }

    #[test]
    fn hierarchical_indices_are_deterministic_and_bounded() {
        let key_id = [
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x01, 0x02, 0x03, 0x04,
            0x05, 0x06, 0x07, 0x08,
        ];
        assert_eq!(hierarchical_indices(key_id), hierarchical_indices(key_id));
        let (l1, l2, l3) = hierarchical_indices(key_id);
        assert_eq!(
            l1,
            u32::from_be_bytes([0x12, 0x34, 0x56, 0x78]) % CHILD_NUMBER_RANGE
        );
        assert_eq!(
            l2,
            u32::from_be_bytes([0x9A, 0xBC, 0xDE, 0xF0]) % CHILD_NUMBER_RANGE
        );
        assert_eq!(
            l3,
            u32::from_be_bytes([0x11, 0x22, 0x33, 0x44]) % CHILD_NUMBER_RANGE
        );
        assert!(l1 < CHILD_NUMBER_RANGE && l2 < CHILD_NUMBER_RANGE && l3 < CHILD_NUMBER_RANGE);
        assert_eq!(hierarchical_indices([0u8; 32]), (0, 0, 0));
    }

    #[test]
    fn hierarchical_indices_distribute_across_the_range() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for i in 0..1000u32 {
            let mut key_id = [0u8; 32];
            key_id[0..4].copy_from_slice(&i.to_be_bytes());
            key_id[4..8].copy_from_slice(&i.wrapping_mul(7919).to_be_bytes());
            key_id[8..12].copy_from_slice(&i.wrapping_mul(104729).to_be_bytes());
            seen.insert(hierarchical_indices(key_id));
        }
        assert!(seen.len() > 900, "distribution too poor: {}", seen.len());
    }

    #[test]
    fn derivation_path_is_base_plus_three_levels() {
        use bitcoin::bip32::ChildNumber;
        let keys = provider();
        let (l1, l2, l3) = hierarchical_indices([1u8; 32]);
        let path = keys.hierarchical_derivation_path(l1, l2, l3).unwrap();
        assert_eq!(path.len(), 6);
        assert_eq!(path[0], ChildNumber::from_hardened_idx(420).unwrap());
        assert_eq!(path[1], ChildNumber::from_hardened_idx(0).unwrap());
        assert_eq!(path[2], ChildNumber::from_hardened_idx(0).unwrap());
        assert_eq!(path[3], ChildNumber::from_normal_idx(l1).unwrap());
        assert_eq!(path[4], ChildNumber::from_normal_idx(l2).unwrap());
        assert_eq!(path[5], ChildNumber::from_normal_idx(l3).unwrap());
    }

    #[test]
    fn hardening_is_deterministic_and_sensitive() {
        let keys = provider();
        let base = SecretKey::from_slice(&[0x42; 32]).unwrap();
        assert_eq!(
            keys.harden(&base, 100, 200, 300).unwrap(),
            keys.harden(&base, 100, 200, 300).unwrap()
        );
        assert_ne!(keys.harden(&base, 100, 200, 300).unwrap(), base);
        assert_ne!(
            keys.harden(&base, 100, 200, 300).unwrap(),
            keys.harden(&base, 100, 200, 301).unwrap()
        );
        assert_ne!(
            keys.harden(&base, 100, 200, 300).unwrap(),
            keys.harden(&base, 100, 201, 300).unwrap()
        );
        assert_ne!(
            keys.harden(&base, 100, 200, 300).unwrap(),
            keys.harden(&base, 101, 200, 300).unwrap()
        );
    }

    #[test]
    fn from_descriptor_requires_a_private_key() {
        // Watch-only descriptor (xpub) has no private key to derive from.
        let secp = Secp256k1::new();
        let xpub = bitcoin::bip32::Xpub::from_priv(
            &secp,
            &Xpriv::new_master(Network::Regtest, &[7u8; 32]).unwrap(),
        );
        let watch_only = format!("wpkh({xpub}/0/*)");
        assert!(matches!(
            ContractKeyProvider::from_descriptor(&watch_only),
            Err(ContractError::Descriptor(_))
        ));
    }
}
