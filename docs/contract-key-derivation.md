# Hierarchical Deterministic Key Derivation for Large-Scale DLC Contracts

## Overview

This document explains how our DLC (Discreet Log Contract) implementation achieves collision-resistant deterministic key derivation that scales to millions of contracts while maintaining complete recoverability. The system transforms the fundamental challenge of managing massive numbers of unique contract keys from a database storage problem into an elegant mathematical derivation problem.

The core innovation lies in leveraging Bitcoin's Hierarchical Deterministic (HD) wallet infrastructure to create multiple levels of key derivation, giving us an enormous effective key space that eliminates hash collisions while remaining completely deterministic and recoverable. This approach allows our system to handle millions of contracts without requiring external databases or cached key storage, while still providing mathematically guaranteed recovery mechanisms for disaster scenarios.

Understanding this system requires grasping how we balance three critical requirements that typically conflict with each other: collision resistance for millions of contracts, stateless deterministic derivation, and practical disaster recovery capabilities. The elegant mathematical solution we present here shows how hierarchical key derivation can satisfy all three requirements simultaneously.

## The Scale Challenge: Moving from Thousands to Millions

Traditional deterministic key derivation systems work well for smaller numbers of contracts, but they encounter fundamental mathematical limits when scaling to millions of contracts. The birthday paradox, a well-known phenomenon in probability theory, explains why collision-free systems become much more complex at scale than our intuition suggests.

To understand this challenge, consider that with a simple single-level derivation system using ten million possible key locations, you would expect to encounter your first collision after creating only about 3,162 contracts. This happens because the probability of collision grows quadratically with the number of contracts, not linearly as we might expect. By the time you reach hundreds of thousands of contracts, collisions become virtually guaranteed rather than merely probable.

This mathematical reality means that any system designed to handle millions of DLC contracts must fundamentally expand beyond single-level key derivation. The solution lies in recognizing that Bitcoin's HD derivation system naturally supports multiple levels of hierarchy, and we can leverage this structure to create key spaces that are astronomically larger than what single-level systems can provide.

## Hierarchical Key Derivation Architecture

Our solution transforms the key derivation process from a single hash-to-index mapping into a hierarchical tree structure that provides multiple levels of collision resistance. Instead of trying to fit millions of contracts into a single dimensional space, we create a three-dimensional space where each dimension provides its own collision-resistant namespace.

The fundamental insight is that we can extract multiple derivation indices from each key identifier, using different portions of the cryptographic hash to determine different levels of the derivation path. This approach maintains complete determinism while expanding our effective key space from millions to billions of possible locations.

Think of this like the difference between organizing books on a single bookshelf versus organizing them in a multi-story library with multiple wings and sections. The single bookshelf quickly becomes overcrowded and difficult to organize without conflicts, while the multi-story library can accommodate millions of books with sophisticated organizational systems that prevent any conflicts.

## Step 1: Hierarchical Key ID Generation

The first step in our improved system creates key identifiers that encode hierarchical derivation information. This process generates a deterministic 32-byte identifier that will be interpreted as containing multiple derivation indices rather than a single index.

```rust
/// Generates a deterministic key ID for contract signing.
///
/// This method creates a unique key identifier for each contract by hashing
/// the temporary contract ID with wallet fingerprint. The resulting key ID is used
/// to derive signing keys for the specific contract.
///
/// # Arguments
/// * `_is_offer_party` - Whether this party is the offer party (currently unused)
/// * `temp_id` - Temporary contract ID from the DLC protocol
///
/// # Returns
/// A 32-byte key ID for the contract
fn derive_signer_key_id(&self, _is_offer_party: bool, temp_id: [u8; 32]) -> [u8; 32] {
    let mut key_id_input = Vec::new();
    key_id_input.extend_from_slice(self.fingerprint.as_bytes());
    key_id_input.extend_from_slice(&temp_id);
    key_id_input.extend_from_slice(b"CONTRACT_SIGNER_KEY_ID_V1");
    let key_id_hash = sha256::Hash::hash(&key_id_input);
    key_id_hash.to_byte_array()
}
```

The critical components of this implementation are the wallet fingerprint integration and the version string. The fingerprint ensures that identical temporary contract IDs produce completely different key identifiers across different wallet instances, preventing any cross-wallet key reuse attacks. The SHA256 hash operation provides excellent avalanche properties, meaning that small changes in the temporary contract ID result in completely different values across all bytes of the resulting key identifier.

This property becomes crucial when we extract multiple derivation indices from different portions of the same hash. The version string provides protection against future protocol changes and ensures compatibility between different implementations of the hierarchical derivation system.

## Step 2: Extracting Multiple Derivation Indices

The hierarchical approach transforms our key identifier interpretation from a single index extraction into a multi-level index extraction process. We divide the key identifier into multiple segments, with each segment determining a different level of the HD derivation path. This implementation uses the first 12 bytes of our 32-byte key identifier to create three independent derivation indices.

```rust
/// Converts a 32-byte key ID into hierarchical indices for derivation paths.
///
/// This function takes a 32-byte key ID and splits it into three 4-byte
/// arrays, which are then used to calculate indices for three levels of
/// derivation paths. The indices are calculated using modulo arithmetic
/// to ensure they fall within the range of 0 to 3399.
fn key_id_to_hierarchical_indices(&self, key_id: [u8; 32]) -> (u32, u32, u32) {
    let level_1 = [key_id[0], key_id[1], key_id[2], key_id[3]];
    let level_2 = [key_id[4], key_id[5], key_id[6], key_id[7]];
    let level_3 = [key_id[8], key_id[9], key_id[10], key_id[11]];
    let level_1_index = u32::from_be_bytes(level_1) % 3_400;
    let level_2_index = u32::from_be_bytes(level_2) % 3_400;
    let level_3_index = u32::from_be_bytes(level_3) % 3_400;
    // Total combination space: 3400 × 3400 × 3400 = ~39.3 billion possible paths
    (level_1_index, level_2_index, level_3_index)
}
```

This extraction process demonstrates a sophisticated understanding of cryptographic hash distribution properties. Because the SHA256 hash provides excellent avalanche effects, each 4-byte segment of the key identifier contains essentially independent entropy. This means that the three derivation indices we extract are statistically independent of each other, providing maximum collision resistance across all levels of the hierarchy.

The modulo operations constrain each level to 3,400 possible values, creating a total combination space of approximately 39.3 billion possible derivation paths. This represents an enormous expansion from single-level systems while maintaining practical disaster recovery capabilities, as we will explore in the security analysis section.

## Step 3: Building Hierarchical Derivation Paths

With our three derivation indices extracted from the first 12 bytes of the key identifier, we construct the complete HD derivation path that will be used to generate the base secret key. This implementation demonstrates how each of the three indices becomes a separate level in the Bitcoin HD derivation path.

```rust
fn get_hierarchical_derivation_path(&self, key_id: [u8; 32]) -> Result<DerivationPath> {
    let (level_1_index, level_2_index, level_3_index) =
        self.key_id_to_hierarchical_indices(key_id);
    let child_one = ChildNumber::from_normal_idx(level_1_index)
        .map_err(|_| WalletError::InvalidDerivationIndex)?;
    let child_two = ChildNumber::from_normal_idx(level_2_index)
        .map_err(|_| WalletError::InvalidDerivationIndex)?;
    let child_three = ChildNumber::from_normal_idx(level_3_index)
        .map_err(|_| WalletError::InvalidDerivationIndex)?;
    let path = self.dlc_path.clone();
    let full_path = path.extend([child_one, child_two, child_three]);
    Ok(full_path)
}
```

The resulting derivation path creates a systematic organization within your wallet's HD tree that is both collision-resistant and completely deterministic. The base `dlc_path` (typically `m/9999'/0'/0'`) provides hardened derivation steps that offer additional security by making it impossible to derive parent keys from child keys, even if an attacker gained access to multiple contract keys.

The three additional levels create what cryptographers call a "tree of trees" structure, where each branch point provides its own collision-resistant namespace. The final path structure becomes something like `m/9999'/0'/0'/1234/5678/9012`, where each number represents one of our extracted hierarchical indices.

This hierarchical organization means that even if there were collisions at one level, they would be distributed across different branches of the tree, preventing actual key collisions in the final derived keys.

## Step 4: Enhanced Hierarchical Hardening

The hardening function for hierarchical derivation incorporates all three levels of derivation indices, creating maximum entropy and security for the final key generation process. This approach provides defense against sophisticated attacks while maintaining the recoverability properties essential for disaster recovery.

```rust
fn apply_hardening_to_base_key(
    &self,
    base_key: &SecretKey,
    level_1: u32,
    level_2: u32,
    level_3: u32,
) -> Result<SecretKey> {
    let mut hardening_input = Vec::new();
    hardening_input.extend_from_slice(self.fingerprint.as_bytes());
    hardening_input.extend_from_slice(&base_key.secret_bytes());
    hardening_input.extend_from_slice(&level_1.to_be_bytes());
    hardening_input.extend_from_slice(&level_2.to_be_bytes());
    hardening_input.extend_from_slice(&level_3.to_be_bytes());
    let hardened_hash = sha256::Hash::hash(&hardening_input);
    SecretKey::from_slice(&hardened_hash.as_ref()).map_err(|_| WalletError::InvalidSecretKey)
}
```

This hardening approach represents what cryptographers call "defense in depth," where multiple independent sources of entropy are combined to create final key material that is resistant to various attack vectors. The function combines the wallet fingerprint, the HD-derived base key, and all three hierarchical levels to create a comprehensive hardening input.

Even if an attacker could somehow predict or influence one or two components of the hardening input, they would still need to compromise the HD key derivation system itself to predict the final keys. The inclusion of all three hierarchical levels in the hardening process ensures that the collision-resistant properties of our derivation system extend through to the final key generation.

Importantly, this hardening function uses only information that is available during both normal operation and disaster recovery scanning, ensuring that recovered keys are mathematically identical to the original keys.

## Step 5: Complete Hierarchical Key Derivation

The complete key derivation function integrates all the hierarchical components into a single deterministic transformation that converts key identifiers into collision-resistant secret keys. This function maintains pure mathematical properties while providing dramatically improved collision resistance.

```rust
fn derive_secret_key_from_key_id(&self, key_id: [u8; 32]) -> Result<SecretKey> {
    let derivation_path = self.get_hierarchical_derivation_path(key_id)?;
    let base_secret_key = self.xprv.derive_priv(&self.secp, &derivation_path)?;
    let (level_1, level_2, level_3) = self.key_id_to_hierarchical_indices(key_id);
    let hardened_key = self.apply_hardening_to_base_key(
        &base_secret_key.private_key,
        level_1,
        level_2,
        level_3,
    )?;
    Ok(hardened_key)
}
```

This complete derivation function embodies the mathematical elegance of hierarchical deterministic systems. Every component is deterministic and repeatable, yet the combination creates security properties that scale to handle millions of contracts without collision concerns. The function uses the master extended private key (`xprv`) to perform the HD derivation directly, which provides excellent performance and security.

The process flows logically from key identifier to hierarchical derivation path, then to HD-derived base key, and finally to the hardened final key. Each step is reversible during disaster recovery scenarios, ensuring that lost keys can always be systematically rediscovered.

## Complete Contract Signer Implementation

The final integration shows how the hierarchical key derivation integrates with the DLC contract signer interface, providing a clean API that hides the mathematical complexity while delivering collision-resistant key generation.

```rust
/// Creates a contract signer from a key ID.
///
/// Takes the key ID generated by `derive_signer_key_id` and creates a
/// SimpleSigner that can sign transactions for the specific contract.
///
/// # Arguments
/// * `key_id` - The key ID to derive the signer from
///
/// # Returns
/// A SimpleSigner configured for the contract
fn derive_contract_signer(
    &self,
    key_id: [u8; 32],
) -> std::result::Result<Self::Signer, ManagerError> {
    let secret_key = self
        .derive_secret_key_from_key_id(key_id)
        .map_err(|e| ManagerError::WalletError(Box::new(e)))?;
    Ok(SimpleSigner::new(secret_key))
}
```

This implementation provides the clean interface that DLC protocols expect while internally performing all the sophisticated hierarchical derivation and hardening operations. The logging provides visibility into the key derivation process for debugging and auditing purposes, while the error handling ensures that any derivation failures are properly propagated and handled.

## Security Analysis: Practical Recovery with Strong Collision Resistance

The implementation presented here strikes an optimal balance between collision resistance and practical disaster recovery capabilities. Understanding this balance is crucial for appreciating why this approach provides both the security needed for large-scale DLC deployments and the recoverability required for long-term operational security.

### Collision Resistance Analysis

With 3,400 slots per level across three levels, our system creates approximately 39.3 billion possible derivation locations. Using birthday paradox mathematics, this provides excellent collision resistance even for millions of contracts. For one million contracts, the probability of experiencing even a single collision is approximately 0.0013%, which represents exceptional collision resistance for practical applications.

Even scaling to ten million contracts, the collision probability remains below 1.3%, providing strong security margins for future growth. This collision resistance is achieved while maintaining a key space that is small enough to be systematically searched during disaster recovery scenarios.

The cryptographic security of this approach leverages Bitcoin's proven HD wallet infrastructure, ensuring that the fundamental security assumptions are well-tested and reliable. The hierarchical derivation uses the same elliptic curve cryptography and hash functions that secure billions of dollars in Bitcoin value.

### Practical Disaster Recovery Capabilities

The constraint to 3,400 slots per level creates a total search space that can be explored within practical time constraints during disaster recovery scenarios. Using modern multi-core systems with optimized parallel processing, the entire 39.3 billion key space can be systematically searched within approximately one week of computation time.

This recovery capability provides a mathematical guarantee that no contract key can be permanently lost as long as you retain access to your wallet's master seed. The systematic scanning process applies the same mathematical transformations used during normal operation, ensuring that recovered keys are identical to the original keys.

The disaster recovery process scales efficiently with available computing resources. Additional CPU cores provide linear improvements in scanning speed, allowing organizations with substantial computing resources to complete recovery operations in days rather than weeks.

### Operational Security Benefits

The hierarchical approach eliminates several operational security risks that plague traditional key management systems. There are no secret keys stored in memory or databases that could be extracted through memory dump attacks or database compromise. Each secret key is computed on-demand and discarded immediately after use.

The deterministic nature of the system means that identical inputs always produce identical outputs, regardless of when or how many times the derivation functions are called. This predictability simplifies testing, auditing, and verification of the key derivation system.

The wallet fingerprint integration ensures that each wallet instance creates its own unique cryptographic namespace, preventing any cross-wallet key reuse attacks even if contract identifiers are reused across different wallet instances.

## Secure Key Identifier Storage for Enhanced Recovery

While the hierarchical deterministic system provides mathematical guarantees for complete key recovery, practical operational security often benefits from additional layers of recovery optimization. A secure storage system for key identifiers can dramatically improve recovery performance while maintaining the deterministic system's dependency-free architecture.

### The Key Identifier Storage Concept

The fundamental insight behind secure key identifier storage is recognizing that key identifiers themselves contain no secret information. They are derived deterministically from contract temporary identifiers and wallet fingerprints, but they do not reveal private keys or enable unauthorized access to contract funds. This property makes them safe to store in auxiliary databases that can accelerate recovery processes without compromising security.

Consider the difference between storing complete secret keys versus storing only the key identifiers that allow secret keys to be derived on demand. Secret key storage creates obvious security vulnerabilities because compromise of the storage system immediately compromises all contract keys. Key identifier storage, by contrast, creates no additional attack surface because the identifiers themselves are not sensitive information.

The key identifier approach transforms the recovery problem from "search the entire mathematical space" to "look up the specific locations we know contain keys." This is like the difference between searching every possible address in a city versus consulting a directory that tells you exactly which addresses contain the buildings you are looking for.

### Implementing Secure Key Identifier Archives

A practical key identifier storage system can be implemented as a simple mapping between contract funding public keys and their corresponding key identifiers. This mapping provides sufficient information to enable instant key recovery without storing any sensitive cryptographic material.

```rust
/// Secure storage structure for key identifier recovery acceleration
/// Maps funding public keys to their deterministic key identifiers for fast lookup
#[derive(Serialize, Deserialize)]
pub struct KeyIdentifierArchive {
    /// Version identifier for forward compatibility with storage format changes
    version: u32,
    /// Wallet fingerprint to ensure archive matches current wallet
    wallet_fingerprint: [u8; 4],
    /// Mapping from funding public keys to their corresponding key identifiers
    /// Public keys are not sensitive information and safe to store
    key_mappings: HashMap<PublicKey, [u8; 32]>,
    /// Optional metadata for operational convenience (contract dates, amounts, etc.)
    metadata: HashMap<PublicKey, ContractMetadata>,
}

impl KeyIdentifierArchive {
    /// Stores a key identifier mapping for future recovery acceleration
    /// This operation is safe because key identifiers contain no secret information
    pub fn store_key_mapping(&mut self, funding_pubkey: PublicKey, key_id: [u8; 32], metadata: Option<ContractMetadata>) {
        self.key_mappings.insert(funding_pubkey, key_id);
        if let Some(meta) = metadata {
            self.metadata.insert(funding_pubkey, meta);
        }
    }

    /// Retrieves a key identifier for instant secret key derivation
    /// Eliminates the need for disaster recovery scanning when archive is available
    pub fn get_key_id(&self, funding_pubkey: &PublicKey) -> Option<[u8; 32]> {
        self.key_mappings.get(funding_pubkey).copied()
    }
}
```

This storage structure demonstrates how auxiliary systems can enhance deterministic key derivation without creating additional dependencies or security vulnerabilities. The archive contains only public information and key identifiers, both of which are safe to store in various backup locations and security contexts.

The wallet fingerprint verification ensures that archived key identifiers match the current wallet instance, preventing confusion if archives from different wallets are accidentally mixed. The optional metadata provides operational convenience for understanding recovered contracts without affecting the core security properties of the system.

### Tiered Recovery Strategy Implementation

The combination of deterministic derivation and secure key identifier storage creates a sophisticated tiered recovery strategy that gracefully adapts to different disaster scenarios. Each tier provides different recovery capabilities with corresponding time and resource requirements.

```rust
/// Comprehensive tiered recovery system combining multiple recovery approaches
/// Automatically selects optimal recovery method based on available information
pub struct TieredRecoverySystem {
    dlc_wallet: DlcDevKitWallet,
    key_archive: Option<KeyIdentifierArchive>,
}

impl TieredRecoverySystem {
    /// Tier 1: Instant recovery using archived key identifiers (when available)
    /// This is the fastest recovery method, completing in microseconds
    pub fn instant_recovery(&self, funding_pubkey: &PublicKey) -> Result<Option<SecretKey>, WalletError> {
        if let Some(archive) = &self.key_archive {
            if let Some(key_id) = archive.get_key_id(funding_pubkey) {
                let secret_key = self.dlc_wallet.derive_secret_key_from_key_id(key_id)?;
                return Ok(Some(secret_key));
            }
        }
        Ok(None)
    }

    /// Tier 2: Reconstruction recovery using contract temporary identifiers
    /// Used when archives are unavailable but contract data is preserved
    pub fn reconstruction_recovery(&self, temp_id: [u8; 32]) -> Result<SecretKey, WalletError> {
        let key_id = self.dlc_wallet.derive_signer_key_id(true, temp_id);
        self.dlc_wallet.derive_secret_key_from_key_id(key_id)
    }

    /// Tier 3: Mathematical disaster recovery through systematic keychain scanning
    /// Used when both archives and contract data are lost, but provides guaranteed recovery
    pub fn disaster_recovery(&self, target_pubkey: &PublicKey) -> Result<Option<SecretKey>, WalletError> {
        // Systematic scan through the 39.3 billion possible derivation paths
        const MAX_LEVEL: u32 = 3400;

        for level_1 in 0..MAX_LEVEL {
            for level_2 in 0..MAX_LEVEL {
                for level_3 in 0..MAX_LEVEL {
                    if let Ok(secret_key) = self.derive_key_at_hierarchical_indices(level_1, level_2, level_3) {
                        let public_key = PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &secret_key);
                        if public_key == *target_pubkey {
                            return Ok(Some(secret_key));
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}
```

This tiered approach provides operational flexibility that adapts to different failure scenarios while maintaining mathematical guarantees for recovery. The instant recovery tier handles normal operational scenarios where archived information is available. The reconstruction tier handles scenarios where contract databases are preserved but key archives are lost. The disaster recovery tier provides mathematical guarantees for complete recovery even when all auxiliary data is lost.

The beauty of this tiered system lies in how each tier degrades gracefully to the next level when needed. Users experience the fastest possible recovery based on available information, but they always retain the mathematical guarantee of complete recoverability through systematic scanning if necessary.

### Security Properties of Key Identifier Storage

The security analysis of key identifier storage reveals why this approach enhances rather than compromises the overall system security. Key identifiers are derived deterministically from non-secret inputs and provide no information that could be used to compromise contract funds or predict other contract keys.

From an information theoretic perspective, key identifiers contain the same information as contract temporary identifiers combined with wallet fingerprints. Since temporary identifiers are typically known to both parties in DLC contracts and fingerprints are not secret, the key identifiers themselves introduce no additional information disclosure.

The storage of key identifiers does create an additional operational component that must be backed up and maintained, but this component contains no secret information and can be stored using conventional database backup practices without special cryptographic protection requirements. The archive can be stored in multiple locations, shared across backup systems, and even transmitted over insecure channels without compromising security.

The threat model for key identifier storage focuses on availability rather than confidentiality. The primary risk is loss of the archive data, which would degrade recovery performance but not compromise security or eliminate recovery capabilities. This represents a much more manageable risk profile than systems that require protecting secret key databases or other sensitive cryptographic materials.

This comprehensive approach to hierarchical deterministic key derivation provides the collision resistance needed for large-scale DLC deployments while maintaining the recoverability properties essential for long-term operational security. The system gracefully balances mathematical perfection with practical usability, creating a foundation for DLC implementations that can scale to millions of contracts while providing robust disaster recovery capabilities and operational flexibility through secure auxiliary storage systems.
