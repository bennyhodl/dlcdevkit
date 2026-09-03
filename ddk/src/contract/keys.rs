//! Deterministic contract funding-key derivation.
//!
//! A DLC contract's funding key (the key controlling its 2-of-2 output, its CET
//! adaptor signatures, and its refund signature) is a pure, deterministic
//! function of a `keys_id`, which is itself a pure function of the contract's
//! temporary id. Nothing is stored: given a master extended private key and a
//! contract's temporary id, the exact funding key is recomputed on demand.
//!
//! # Derivation schemes
//!
//! Two schemes exist. New contracts always use V1; V0 stays supported so that
//! contracts funded by earlier releases keep working after an upgrade.
//!
//! ```text
//! V1 (from 2.0.0-rc.3)
//!   keys_id  = SHA256( fingerprint || temporary_contract_id || "CONTRACT_SIGNER_KEY_ID_V1" )[0..24]
//!              || "DDKKEYv1"
//!   l1,l2,l3 = keys_id[0..4], keys_id[4..8], keys_id[8..12], each mod 3400
//!   base_sk  = BIP32 private child at  m/420'/0'/0'/l1'/l2'/l3'
//!   sk       = SHA256( "CONTRACT_KEY_V1" || base_sk || l1 || l2 || l3 )
//!
//! V0 (all earlier releases)
//!   keys_id  = SHA256( fingerprint || temporary_contract_id || "CONTRACT_SIGNER_KEY_ID_V0" )
//!   l1,l2,l3 = as above
//!   base_sk  = BIP32 private child at  m/420'/0'/0'/l1/l2/l3        (not hardened)
//!   sk       = SHA256( fingerprint || base_sk || l1 || l2 || l3 )
//! ```
//!
//! In V1 every level of the BIP32 path is hardened. A leaked contract key
//! together with any ancestor extended public key does not give an attacker the
//! parent private key, so the security of the scheme is the security of the
//! master extended private key, exactly as in a plain BIP32 wallet. The final
//! hash is defense in depth only: it is not load-bearing.
//!
//! A `keys_id` carries its scheme: V1 ids end with the 8-byte marker
//! `DDKKEYv1`, and any other id is V0. The manager stores the `keys_id` with
//! each contract, so [`ContractKeyProvider::funding_secret_key_for_keys_id`]
//! (and the [`ddk_manager::ContractSignerProvider`] impl) derive the right key
//! for old and new contracts alike. See [`KeyScheme`].
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
use zeroize::Zeroize;

use super::error::ContractError;
use super::types::DlcInputSigningKey;

/// Range of child numbers per hierarchical level. `3400^3 ≈ 39.3` billion paths
/// — large enough to avoid collisions across millions of contracts, small
/// enough for practical disaster recovery.
const CHILD_NUMBER_RANGE: u32 = 3_400;

/// Base derivation path for contract keys.
const DLC_BASE_PATH: &str = "m/420'/0'/0'";

/// Domain-separation tag for the V0 keys-id hash.
const KEYS_ID_TAG_V0: &[u8] = b"CONTRACT_SIGNER_KEY_ID_V0";

/// Domain-separation tag for the V1 keys-id hash.
const KEYS_ID_TAG_V1: &[u8] = b"CONTRACT_SIGNER_KEY_ID_V1";

/// Trailing marker that identifies a V1 `keys_id`. Occupies bytes `24..32`,
/// which the derivation never reads. A V0 id is a full SHA-256 output, so the
/// chance that it ends with this marker by accident is 2^-64.
const KEYS_ID_V1_MARKER: &[u8; 8] = b"DDKKEYv1";

/// Domain-separation tag for the V1 hardening hash over the BIP32 child key.
const HARDEN_TAG_V1: &[u8] = b"CONTRACT_KEY_V1";

/// The derivation scheme behind a `keys_id` or funding key.
///
/// New contracts use [`KeyScheme::V1`]. [`KeyScheme::V0`] is the scheme of
/// releases before `2.0.0-rc.3`. It exists only to *unlock* contracts funded by
/// those releases: the library selects it automatically when a stored
/// `keys_id` has no V1 marker ([`KeyScheme::of_keys_id`]) or when a published
/// funding pubkey only matches under V0
/// ([`ContractKeyProvider::funding_secret_key_for_pubkey`]). Naming
/// `KeyScheme::V0` in code is a deprecation diagnostic, and a hard error inside
/// the `ddk` and `ddk-manager` crates, so no new contract can be created on V0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyScheme {
    /// Non-hardened contract levels, fingerprint-keyed final hash.
    ///
    /// Unlock-only. Do not derive new keys on this scheme; use
    /// [`KeyScheme::CURRENT`], or one of the resolvers that pick the scheme
    /// from stored data.
    #[deprecated(
        since = "2.0.0-rc.3",
        note = "V0 only unlocks contracts funded before 2.0.0-rc.3. Do not name it: \
                use `funding_secret_key_for_keys_id` or `funding_secret_key_for_pubkey`, \
                which select the scheme from the stored keys_id or the published pubkey."
    )]
    V0,
    /// Hardened contract levels, domain-tagged final hash.
    V1,
}

impl KeyScheme {
    /// The scheme new contracts use.
    pub const CURRENT: KeyScheme = KeyScheme::V1;

    /// Every scheme, newest first. Recovery code that has only a temporary id
    /// and a funding public key tries these in order.
    // The one sanctioned place that lists V0: resolvers walk this to unlock.
    #[allow(deprecated)]
    pub const ALL: [KeyScheme; 2] = [KeyScheme::V1, KeyScheme::V0];

    /// Reads the scheme a `keys_id` was produced under.
    // Unlock path: a stored id without the V1 marker was made by an old release.
    #[allow(deprecated)]
    pub fn of_keys_id(keys_id: &[u8; 32]) -> KeyScheme {
        if &keys_id[24..32] == KEYS_ID_V1_MARKER {
            KeyScheme::V1
        } else {
            KeyScheme::V0
        }
    }
}

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

    /// The `keys_id` for a new contract, a deterministic function of its
    /// temporary id under the current scheme ([`KeyScheme::CURRENT`]).
    pub fn keys_id(&self, temporary_contract_id: [u8; 32]) -> [u8; 32] {
        self.keys_id_with_scheme(temporary_contract_id, KeyScheme::CURRENT)
    }

    /// The `keys_id` for a contract under a specific scheme.
    ///
    /// Prefer [`keys_id`](Self::keys_id). Passing `KeyScheme::V0` is a
    /// deprecation diagnostic; recovery tools that must rebuild an old id
    /// should go through [`funding_secret_key_for_pubkey`](Self::funding_secret_key_for_pubkey).
    // Dispatches on both schemes, so it must be able to name V0.
    #[allow(deprecated)]
    pub fn keys_id_with_scheme(
        &self,
        temporary_contract_id: [u8; 32],
        scheme: KeyScheme,
    ) -> [u8; 32] {
        let tag = match scheme {
            KeyScheme::V0 => KEYS_ID_TAG_V0,
            KeyScheme::V1 => KEYS_ID_TAG_V1,
        };
        let mut input = Vec::with_capacity(4 + 32 + tag.len());
        input.extend_from_slice(self.fingerprint.as_bytes());
        input.extend_from_slice(&temporary_contract_id);
        input.extend_from_slice(tag);
        let mut keys_id = sha256::Hash::hash(&input).to_byte_array();
        if scheme == KeyScheme::V1 {
            keys_id[24..32].copy_from_slice(KEYS_ID_V1_MARKER);
        }
        keys_id
    }

    /// The funding secret key for a `keys_id`.
    ///
    /// The scheme is read from the id itself ([`KeyScheme::of_keys_id`]), so a
    /// `keys_id` stored by an earlier release derives the same key it always
    /// did, and a new id derives a V1 key.
    // Dispatches on both schemes, so it must be able to name V0.
    #[allow(deprecated)]
    pub fn funding_secret_key_for_keys_id(
        &self,
        keys_id: [u8; 32],
    ) -> Result<SecretKey, ContractError> {
        let scheme = KeyScheme::of_keys_id(&keys_id);
        let (level_1, level_2, level_3) = hierarchical_indices(keys_id);
        let path = self.hierarchical_derivation_path(scheme, level_1, level_2, level_3)?;
        let mut child = self
            .xprv
            .derive_priv(&self.secp, &path)
            .map_err(|e| ContractError::Bip32(e.to_string()))?;
        let hardened = match scheme {
            KeyScheme::V0 => harden_v0(
                &self.fingerprint,
                &child.private_key,
                level_1,
                level_2,
                level_3,
            ),
            KeyScheme::V1 => harden_v1(&child.private_key, level_1, level_2, level_3),
        };
        // The BIP32 child is a real secret in the wallet's subtree. It is only an
        // intermediate value here, so wipe it as soon as the final key exists.
        child.private_key.non_secure_erase();
        hardened
    }

    /// The funding secret key for a new contract, from its temporary id, under
    /// the current scheme.
    pub fn funding_secret_key(
        &self,
        temporary_contract_id: [u8; 32],
    ) -> Result<SecretKey, ContractError> {
        self.funding_secret_key_with_scheme(temporary_contract_id, KeyScheme::CURRENT)
    }

    /// The funding secret key for a contract under a specific scheme.
    ///
    /// Prefer [`funding_secret_key`](Self::funding_secret_key). Passing
    /// `KeyScheme::V0` is a deprecation diagnostic.
    pub fn funding_secret_key_with_scheme(
        &self,
        temporary_contract_id: [u8; 32],
        scheme: KeyScheme,
    ) -> Result<SecretKey, ContractError> {
        self.funding_secret_key_for_keys_id(self.keys_id_with_scheme(temporary_contract_id, scheme))
    }

    /// Recovers the funding secret key of an existing contract from its
    /// temporary id and the funding public key that was published for it.
    ///
    /// Tries every scheme, newest first, and returns the key whose public key
    /// equals `funding_pubkey`. This is the entry point for anything that holds
    /// wire messages but no stored `keys_id`, because the messages do not say
    /// which scheme was current when the contract was made.
    pub fn funding_secret_key_for_pubkey(
        &self,
        temporary_contract_id: [u8; 32],
        funding_pubkey: &PublicKey,
    ) -> Result<SecretKey, ContractError> {
        for scheme in KeyScheme::ALL {
            let secret_key = self.funding_secret_key_with_scheme(temporary_contract_id, scheme)?;
            if secret_key.public_key(&self.secp) == *funding_pubkey {
                return Ok(secret_key);
            }
        }
        Err(ContractError::Key(format!(
            "no key scheme derives funding pubkey {funding_pubkey} for this temporary contract id"
        )))
    }

    /// The funding public key for a new contract — publish this in the offer or
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
    ///
    /// `prior_funding_pubkey` is this party's funding public key from the
    /// previous contract's offer or accept message. It selects the scheme the
    /// previous contract was made under, so contracts funded before
    /// `2.0.0-rc.3` splice correctly. The call fails if no scheme reproduces it.
    pub fn dlc_input_signing_key(
        &self,
        prior_temporary_contract_id: [u8; 32],
        prior_funding_pubkey: &PublicKey,
        input_serial_id: u64,
    ) -> Result<DlcInputSigningKey, ContractError> {
        Ok(DlcInputSigningKey {
            input_serial_id,
            prior_funding_secret_key: self
                .funding_secret_key_for_pubkey(prior_temporary_contract_id, prior_funding_pubkey)?,
        })
    }

    /// V1: `m/420'/0'/0'/l1'/l2'/l3'`. All three contract levels are hardened
    /// so a leaked child key plus an ancestor xpub cannot recover the parent key.
    /// V0: `m/420'/0'/0'/l1/l2/l3`, kept only to derive keys of old contracts.
    // Dispatches on both schemes, so it must be able to name V0.
    #[allow(deprecated)]
    fn hierarchical_derivation_path(
        &self,
        scheme: KeyScheme,
        level_1: u32,
        level_2: u32,
        level_3: u32,
    ) -> Result<DerivationPath, ContractError> {
        let child = |index: u32| {
            match scheme {
                KeyScheme::V0 => ChildNumber::from_normal_idx(index),
                KeyScheme::V1 => ChildNumber::from_hardened_idx(index),
            }
            .map_err(|e| ContractError::Key(format!("invalid derivation index: {e}")))
        };
        Ok(self
            .dlc_path
            .extend([child(level_1)?, child(level_2)?, child(level_3)?]))
    }
}

/// V1: `SHA256( "CONTRACT_KEY_V1" || base_sk || l1 || l2 || l3 )`.
///
/// Defense in depth over the hardened BIP32 child: a dump of the subtree still
/// requires one SHA-256 preimage per contract. The hash input holds the child
/// secret, so it is wiped before returning.
fn harden_v1(
    base_key: &SecretKey,
    level_1: u32,
    level_2: u32,
    level_3: u32,
) -> Result<SecretKey, ContractError> {
    harden(HARDEN_TAG_V1, base_key, level_1, level_2, level_3)
}

/// V0: `SHA256( fingerprint || base_sk || l1 || l2 || l3 )`. Kept only to
/// derive keys of contracts funded before `2.0.0-rc.3`.
fn harden_v0(
    fingerprint: &Fingerprint,
    base_key: &SecretKey,
    level_1: u32,
    level_2: u32,
    level_3: u32,
) -> Result<SecretKey, ContractError> {
    harden(fingerprint.as_bytes(), base_key, level_1, level_2, level_3)
}

fn harden(
    prefix: &[u8],
    base_key: &SecretKey,
    level_1: u32,
    level_2: u32,
    level_3: u32,
) -> Result<SecretKey, ContractError> {
    let mut base_bytes = base_key.secret_bytes();
    let mut input = Vec::with_capacity(prefix.len() + 32 + 12);
    input.extend_from_slice(prefix);
    input.extend_from_slice(&base_bytes);
    input.extend_from_slice(&level_1.to_be_bytes());
    input.extend_from_slice(&level_2.to_be_bytes());
    input.extend_from_slice(&level_3.to_be_bytes());
    let digest = sha256::Hash::hash(&input).to_byte_array();
    input.zeroize();
    base_bytes.zeroize();
    SecretKey::from_slice(&digest)
        .map_err(|e| ContractError::Key(format!("invalid derived key: {e}")))
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

    fn derive_signer_key_id(&self, temp_id: [u8; 32]) -> [u8; 32] {
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
// The tests name V0 on purpose: they pin the legacy scheme so old contracts
// keep unlocking. Production code must not do this.
#[allow(deprecated)]
mod tests {
    use super::*;
    use ddk_manager::{ContractSigner, ContractSignerProvider};

    const TEMP_A: [u8; 32] = [0xA1; 32];
    const TEMP_B: [u8; 32] = [0xB2; 32];
    const MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// Test vectors for `MNEMONIC` (no passphrase, regtest) and `TEMP_A`.
    ///
    /// The V0 values were produced by the V0 code shipped in `2.0.0-rc.2`. They
    /// pin the pre-hardening scheme so `v0_reference` below is known to be a
    /// faithful re-implementation, and so old contracts keep deriving the same
    /// key. The V1 values pin the current scheme against accidental drift.
    const V0_KEYS_ID: &str = "a7649d1c927f2b8af024e9a1d3b59c84da975629a2c3805f16afa19090ab6115";
    const V0_FUNDING_PK: &str =
        "03aa70703dca189d6df5cc73408d2e96f4c3f3a761f4889653984b8a694956a35a";
    const V1_KEYS_ID: &str = "5604be211579c6a4c28fa5e61ce059ae4f4716c2bee2da0f44444b4b45597631";
    const V1_FUNDING_PK: &str =
        "03c952c4c3a4f7b69ab23bbc8d1d6f1a1487beca22edeb32c3116f82ac8aca7159";

    fn provider() -> ContractKeyProvider {
        ContractKeyProvider::from_mnemonic(MNEMONIC, None, Network::Regtest).unwrap()
    }

    fn pk(hex_str: &str) -> PublicKey {
        PublicKey::from_str(hex_str).unwrap()
    }

    /// The V0 scheme, re-implemented verbatim from `2.0.0-rc.2`: V0 keys-id
    /// tag, non-hardened contract levels, and the wallet fingerprint (not a
    /// tag) in the final hash. Independent of the production V0 code path.
    fn v0_reference(keys: &ContractKeyProvider, temp_id: [u8; 32]) -> ([u8; 32], SecretKey) {
        let mut input = Vec::new();
        input.extend_from_slice(keys.fingerprint.as_bytes());
        input.extend_from_slice(&temp_id);
        input.extend_from_slice(b"CONTRACT_SIGNER_KEY_ID_V0");
        let keys_id = sha256::Hash::hash(&input).to_byte_array();

        let (l1, l2, l3) = hierarchical_indices(keys_id);
        let path = keys.dlc_path.extend([
            ChildNumber::from_normal_idx(l1).unwrap(),
            ChildNumber::from_normal_idx(l2).unwrap(),
            ChildNumber::from_normal_idx(l3).unwrap(),
        ]);
        let base = keys
            .xprv
            .derive_priv(&keys.secp, &path)
            .unwrap()
            .private_key;

        let mut input = Vec::new();
        input.extend_from_slice(keys.fingerprint.as_bytes());
        input.extend_from_slice(&base.secret_bytes());
        input.extend_from_slice(&l1.to_be_bytes());
        input.extend_from_slice(&l2.to_be_bytes());
        input.extend_from_slice(&l3.to_be_bytes());
        let sk = SecretKey::from_slice(sha256::Hash::hash(&input).as_ref()).unwrap();
        (keys_id, sk)
    }

    #[test]
    fn v0_reference_matches_the_shipped_v0_vectors() {
        let keys = provider();
        let (keys_id, sk) = v0_reference(&keys, TEMP_A);
        assert_eq!(hex::encode(keys_id), V0_KEYS_ID);
        assert_eq!(sk.public_key(&keys.secp).to_string(), V0_FUNDING_PK);
    }

    #[test]
    fn v1_vectors_are_pinned() {
        let keys = provider();
        assert_eq!(hex::encode(keys.keys_id(TEMP_A)), V1_KEYS_ID);
        assert_eq!(
            keys.funding_pubkey(TEMP_A).unwrap().to_string(),
            V1_FUNDING_PK
        );
        assert_eq!(KeyScheme::CURRENT, KeyScheme::V1);
        assert_eq!(
            keys.keys_id_with_scheme(TEMP_A, KeyScheme::V1),
            keys.keys_id(TEMP_A)
        );
    }

    #[test]
    fn v0_scheme_is_still_derivable_from_a_temp_id() {
        // Backwards compatibility: the production V0 path must reproduce the
        // keys that 2.0.0-rc.2 produced, from the temp id alone.
        let keys = provider();
        let keys_id = keys.keys_id_with_scheme(TEMP_A, KeyScheme::V0);
        assert_eq!(hex::encode(keys_id), V0_KEYS_ID);
        assert_eq!(
            keys.funding_secret_key_with_scheme(TEMP_A, KeyScheme::V0)
                .unwrap()
                .public_key(&keys.secp),
            pk(V0_FUNDING_PK)
        );
        for temp_id in [TEMP_A, TEMP_B, [0u8; 32], [0xFF; 32]] {
            let (v0_keys_id, v0_sk) = v0_reference(&keys, temp_id);
            assert_eq!(keys.keys_id_with_scheme(temp_id, KeyScheme::V0), v0_keys_id);
            assert_eq!(
                keys.funding_secret_key_with_scheme(temp_id, KeyScheme::V0)
                    .unwrap(),
                v0_sk
            );
        }
    }

    #[test]
    fn a_stored_v0_keys_id_derives_the_v0_key() {
        // Backwards compatibility for the manager path: a keys_id persisted by
        // an earlier release is fed straight to derive_contract_signer and must
        // give the key that funded the contract.
        let keys = provider();
        let stored: [u8; 32] = hex::decode(V0_KEYS_ID).unwrap().try_into().unwrap();
        assert_eq!(KeyScheme::of_keys_id(&stored), KeyScheme::V0);
        assert_eq!(
            keys.funding_secret_key_for_keys_id(stored)
                .unwrap()
                .public_key(&keys.secp),
            pk(V0_FUNDING_PK)
        );
        let signer = keys.derive_contract_signer(stored).unwrap();
        assert_eq!(
            signer.get_public_key(&keys.secp).unwrap(),
            pk(V0_FUNDING_PK)
        );
    }

    #[test]
    fn a_new_keys_id_derives_the_v1_key() {
        let keys = provider();
        let keys_id = keys.derive_signer_key_id(TEMP_A);
        assert_eq!(KeyScheme::of_keys_id(&keys_id), KeyScheme::V1);
        assert_eq!(&keys_id[24..32], KEYS_ID_V1_MARKER);
        let signer = keys.derive_contract_signer(keys_id).unwrap();
        assert_eq!(
            signer.get_public_key(&keys.secp).unwrap(),
            pk(V1_FUNDING_PK)
        );
    }

    #[test]
    fn v1_keys_differ_from_v0_keys_for_the_same_temp_id() {
        // Pins the migration boundary: the two schemes never collide, so a
        // key can always be attributed to exactly one of them.
        let keys = provider();
        for temp_id in [TEMP_A, TEMP_B, [0u8; 32], [0xFF; 32]] {
            let (v0_keys_id, v0_sk) = v0_reference(&keys, temp_id);
            assert_ne!(keys.keys_id(temp_id), v0_keys_id);
            assert_ne!(keys.funding_secret_key(temp_id).unwrap(), v0_sk);
        }
    }

    #[test]
    fn funding_secret_key_for_pubkey_resolves_either_scheme() {
        let keys = provider();
        let v1 = keys
            .funding_secret_key_for_pubkey(TEMP_A, &pk(V1_FUNDING_PK))
            .unwrap();
        assert_eq!(v1, keys.funding_secret_key(TEMP_A).unwrap());
        let v0 = keys
            .funding_secret_key_for_pubkey(TEMP_A, &pk(V0_FUNDING_PK))
            .unwrap();
        assert_eq!(
            v0,
            keys.funding_secret_key_with_scheme(TEMP_A, KeyScheme::V0)
                .unwrap()
        );
        // A pubkey from another contract (or another wallet) is an error, not a
        // silently wrong key.
        let other = keys.funding_pubkey(TEMP_B).unwrap();
        assert!(matches!(
            keys.funding_secret_key_for_pubkey(TEMP_A, &other),
            Err(ContractError::Key(_))
        ));
    }

    #[test]
    fn dlc_input_signing_key_recovers_the_prior_key_under_either_scheme() {
        let keys = provider();
        let v1 = keys
            .dlc_input_signing_key(TEMP_A, &pk(V1_FUNDING_PK), 900)
            .unwrap();
        assert_eq!(v1.input_serial_id, 900);
        assert_eq!(
            v1.prior_funding_secret_key,
            keys.funding_secret_key(TEMP_A).unwrap()
        );
        let v0 = keys
            .dlc_input_signing_key(TEMP_A, &pk(V0_FUNDING_PK), 901)
            .unwrap();
        assert_eq!(
            v0.prior_funding_secret_key.public_key(&keys.secp),
            pk(V0_FUNDING_PK)
        );
        assert!(keys
            .dlc_input_signing_key(TEMP_A, &keys.funding_pubkey(TEMP_B).unwrap(), 902)
            .is_err());
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
    fn v1_derivation_path_is_base_plus_three_hardened_levels() {
        let keys = provider();
        let (l1, l2, l3) = hierarchical_indices([1u8; 32]);
        let path = keys
            .hierarchical_derivation_path(KeyScheme::V1, l1, l2, l3)
            .unwrap();
        assert_eq!(path.len(), 6);
        assert_eq!(path[0], ChildNumber::from_hardened_idx(420).unwrap());
        assert_eq!(path[1], ChildNumber::from_hardened_idx(0).unwrap());
        assert_eq!(path[2], ChildNumber::from_hardened_idx(0).unwrap());
        assert_eq!(path[3], ChildNumber::from_hardened_idx(l1).unwrap());
        assert_eq!(path[4], ChildNumber::from_hardened_idx(l2).unwrap());
        assert_eq!(path[5], ChildNumber::from_hardened_idx(l3).unwrap());
        assert!(path.into_iter().all(|child| child.is_hardened()));
    }

    #[test]
    fn v0_derivation_path_keeps_normal_contract_levels() {
        let keys = provider();
        let (l1, l2, l3) = hierarchical_indices([1u8; 32]);
        let path = keys
            .hierarchical_derivation_path(KeyScheme::V0, l1, l2, l3)
            .unwrap();
        assert_eq!(path[3], ChildNumber::from_normal_idx(l1).unwrap());
        assert_eq!(path[4], ChildNumber::from_normal_idx(l2).unwrap());
        assert_eq!(path[5], ChildNumber::from_normal_idx(l3).unwrap());
    }

    #[test]
    fn hardened_levels_block_the_xpub_walk_up() {
        use bitcoin::bip32::Xpub;
        // With hardened contract levels, the xpub at the base path cannot
        // derive the contract child public keys at all: the attack in the audit
        // (child sk + parent xpub => parent sk) has no non-hardened step to use.
        let keys = provider();
        let base_xpub = Xpub::from_priv(
            &keys.secp,
            &keys.xprv.derive_priv(&keys.secp, &keys.dlc_path).unwrap(),
        );
        let (l1, l2, l3) = hierarchical_indices(keys.keys_id(TEMP_A));
        let hardened = [
            ChildNumber::from_hardened_idx(l1).unwrap(),
            ChildNumber::from_hardened_idx(l2).unwrap(),
            ChildNumber::from_hardened_idx(l3).unwrap(),
        ];
        assert!(base_xpub.derive_pub(&keys.secp, &hardened).is_err());
    }

    #[test]
    fn hardening_is_deterministic_and_sensitive() {
        let base = SecretKey::from_slice(&[0x42; 32]).unwrap();
        assert_eq!(
            harden_v1(&base, 100, 200, 300).unwrap(),
            harden_v1(&base, 100, 200, 300).unwrap()
        );
        assert_ne!(harden_v1(&base, 100, 200, 300).unwrap(), base);
        assert_ne!(
            harden_v1(&base, 100, 200, 300).unwrap(),
            harden_v1(&base, 100, 200, 301).unwrap()
        );
        assert_ne!(
            harden_v1(&base, 100, 200, 300).unwrap(),
            harden_v1(&base, 100, 201, 300).unwrap()
        );
        assert_ne!(
            harden_v1(&base, 100, 200, 300).unwrap(),
            harden_v1(&base, 101, 200, 300).unwrap()
        );
    }

    #[test]
    fn hardening_is_domain_tagged() {
        // The V1 hash is over the tag, not the fingerprint, and is exactly
        // SHA256(tag || base || l1 || l2 || l3).
        let base = SecretKey::from_slice(&[0x42; 32]).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(b"CONTRACT_KEY_V1");
        expected.extend_from_slice(&[0x42; 32]);
        expected.extend_from_slice(&1u32.to_be_bytes());
        expected.extend_from_slice(&2u32.to_be_bytes());
        expected.extend_from_slice(&3u32.to_be_bytes());
        let expected = SecretKey::from_slice(sha256::Hash::hash(&expected).as_ref()).unwrap();
        assert_eq!(harden_v1(&base, 1, 2, 3).unwrap(), expected);
        let fingerprint = provider().fingerprint;
        assert_ne!(
            harden_v0(&fingerprint, &base, 1, 2, 3).unwrap(),
            harden_v1(&base, 1, 2, 3).unwrap()
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
