# Contract Key Derivation

Scheme versions: **V1** is current (from `2.0.0-rc.3`). **V0** (all earlier
releases) stays fully supported. A node upgraded in place keeps signing,
settling, and splicing its V0 contracts, and makes V1 keys for new contracts.

Code: `ddk/src/contract/keys.rs` (`ContractKeyProvider`, `KeyScheme`).
`DlcDevKitWallet` delegates to the same type, so the manager path and the
stateless API derive identical keys.

## Purpose

Each DLC contract has one *funding key*. It controls the 2-of-2 funding output,
signs the CET adaptor signatures, and signs the refund transaction. DDK does not
store funding keys. It derives each key on demand from two inputs:

- the wallet's master extended private key (`xprv`)
- the contract's temporary contract id (`temp_id`, 32 random bytes from the
  `OfferDlc`)

The same inputs always give the same key. A wallet restored from its seed can
re-derive every contract key it ever used.

## Derivation

```text
fingerprint = BIP32 fingerprint of the master xprv (4 bytes)

V1 (current)
  keys_id   = SHA256( fingerprint || temp_id || "CONTRACT_SIGNER_KEY_ID_V1" )[0..24] || "DDKKEYv1"
  l1        = u32_be( keys_id[0..4] )  mod 3400
  l2        = u32_be( keys_id[4..8] )  mod 3400
  l3        = u32_be( keys_id[8..12] ) mod 3400
  base_sk   = private key at  m/420'/0'/0'/l1'/l2'/l3'
  sk        = SHA256( "CONTRACT_KEY_V1" || base_sk || u32_be(l1) || u32_be(l2) || u32_be(l3) )

V0 (releases before 2.0.0-rc.3)
  keys_id   = SHA256( fingerprint || temp_id || "CONTRACT_SIGNER_KEY_ID_V0" )
  l1,l2,l3  = as above
  base_sk   = private key at  m/420'/0'/0'/l1/l2/l3          (levels NOT hardened)
  sk        = SHA256( fingerprint || base_sk || u32_be(l1) || u32_be(l2) || u32_be(l3) )
```

`sk` is the funding secret key. Its public key is published in the offer or
accept message as `funding_pubkey`.

### Step 1: keys id

The `keys_id` is the value the manager stores with each contract. It contains no
secret. The fingerprint makes the id different in each wallet for the same
`temp_id`. The tag names the scheme version.

**The `keys_id` carries its scheme.** A V1 id ends with the 8-byte marker
`DDKKEYv1` in bytes `24..32`, which the derivation never reads. Any id without
the marker is a V0 id. `KeyScheme::of_keys_id` makes this decision, and
`funding_secret_key_for_keys_id` (used by `derive_contract_signer`) derives with
the scheme it finds. This is what keeps stored contracts working after an
upgrade: the manager feeds the stored id back in and gets the key that funded
the contract. A V0 id is a full SHA-256 output, so the chance that it ends with
the marker by accident is 2^-64.

`ContractSignerProvider::derive_signer_key_id(temp_id)` returns the V1 id. It
takes only the temporary id. Both parties of a contract call it with the same
`temp_id` and get unrelated keys, because their fingerprints and master keys
differ.

### Step 2: three hardened levels

The first 12 bytes of the `keys_id` give three child indices in the range
`0..3400`. The three levels give `3400^3`, about 39.3 billion, distinct paths.
For one million contracts the probability of two contracts on the same path is
about 0.0013 %. Two contracts on the same path get the same funding key, so a
path collision is a key collision. The space is large enough that this does not
occur in practice and small enough that a full scan is possible (see
[Disaster recovery](#disaster-recovery)).

In V1, **every level of the path is hardened**, including the three contract
levels. This is the security boundary of the scheme:

- A leaked contract key together with any extended public key of the tree does
  not give the parent private key. Non-hardened BIP32 derivation would allow
  this walk-up; hardened derivation does not.
- No extended public key can derive contract public keys. Watch-only derivation
  of funding keys was never possible in V0 either, because of the final hash, so
  hardening the levels removes no function.

The rule for operators is the standard BIP32 rule: protect the master `xprv`.
There are no additional invariants.

V0 contract levels are not hardened. The V0 code path exists only to derive the
keys of contracts that were funded under V0. New contracts never use it.

**Explicit V0 use is rejected at compile time.** `KeyScheme::V0` carries
`#[deprecated]`. Naming it is a deprecation diagnostic for downstream crates,
and the `ddk` and `ddk-manager` crates deny that lint, so inside the library it
is a hard error. The only code allowed to name V0 is the scheme dispatch in
`keys.rs` and its tests. Everything else reaches V0 through the resolvers that
read the scheme from stored data: `funding_secret_key_for_keys_id` (marker) and
`funding_secret_key_for_pubkey` (published pubkey). The manager never names a
scheme at all: it calls `derive_signer_key_id`, which is always V1, and
`derive_contract_signer`, which follows the stored id.

### Step 3: domain-tagged hash

The final SHA-256 over the hardened child is defense in depth. It is not
load-bearing. An attacker who obtains a dump of the `m/420'/0'/0'` subtree still
needs one SHA-256 preimage per contract. The V1 tag `CONTRACT_KEY_V1` names the
construction. The wallet fingerprint is not part of the V1 hash: it is public and
adds nothing. V0 used the fingerprint in this position.

The intermediate `base_sk` is a real secret of the wallet's key tree. The code
wipes it, and the hash input buffer that contains it, as soon as `sk` exists.

## API

| Need | Call |
|---|---|
| Key id for a new contract | `keys_id(temp_id)` |
| Key id under a given scheme | `keys_id_with_scheme(temp_id, scheme)` |
| Key from a stored key id (any scheme) | `funding_secret_key_for_keys_id(keys_id)` |
| Key for a new contract | `funding_secret_key(temp_id)` / `funding_pubkey(temp_id)` |
| Key under a given scheme | `funding_secret_key_with_scheme(temp_id, scheme)` |
| Key of an existing contract from its messages | `funding_secret_key_for_pubkey(temp_id, &funding_pubkey)` |
| Splice key for a prior contract | `dlc_input_signing_key(prior_temp_id, &prior_funding_pubkey, serial_id)` |
| Which scheme made a key id | `KeyScheme::of_keys_id(&keys_id)` |

`funding_secret_key_for_pubkey` and `dlc_input_signing_key` try every scheme,
newest first, and return the key whose public key matches. They fail if no
scheme matches. The stateless API keeps only wire messages, and the messages do
not say which scheme was current when the contract was made, so the published
funding pubkey is the selector. A splice of a V0 contract therefore works with
the prior offer or accept message alone.

## Disaster recovery

Recovery has three tiers. Each tier uses the same derivation as normal
operation, so recovered keys are identical to the original keys.

1. **Stored contract.** The manager stores the `keys_id` with each contract.
   `derive_contract_signer(keys_id)` returns the key directly, for V0 and V1
   ids alike.
2. **Known temporary id and funding pubkey.** Given the `OfferDlc` and
   `AcceptDlc`, `funding_secret_key_for_pubkey(temp_id, &funding_pubkey)`
   returns the key under whichever scheme made it.
3. **Full scan.** Given only the seed and a target `funding_pubkey`, iterate
   `l1, l2, l3` over `0..3400` each, derive `base_sk`, apply the final hash with
   those indices, and compare the public key. Run the scan once per scheme:
   V1 with hardened levels and the V1 tag, then V0 with normal levels and the
   fingerprint. Each scan covers 39.3 billion paths and completes in about one
   week on a modern multi-core machine. It scales linearly with cores.

## Migration from V0

V1 changed three things: the keys-id tag and marker, hardened contract levels,
and the domain tag in place of the fingerprint in the final hash. A V1 provider
never reproduces a V0 key from a V1 id, and the two schemes never collide, so
every key belongs to exactly one scheme.

Nothing needs to be done at upgrade time:

- Contracts **funded before** the upgrade to `2.0.0-rc.3` have V0 ids in
  storage and keep deriving on **V0**.
- Contracts **offered or accepted after** the upgrade get V1 ids and derive on
  **V1**.
- Splices from a V0 contract into a new V1 contract work through
  `dlc_input_signing_key`, which selects V0 from the prior funding pubkey.

Tests in `keys.rs` pin this behaviour: `v0_reference_matches_the_shipped_v0_vectors`
pins the exact `2.0.0-rc.2` output for a fixed mnemonic against an independent
re-implementation, `a_stored_v0_keys_id_derives_the_v0_key` covers the manager
path, and `v1_keys_differ_from_v0_keys_for_the_same_temp_id` pins the boundary.

## Test vectors

Mnemonic `abandon abandon abandon abandon abandon abandon abandon abandon abandon
abandon abandon about`, no passphrase, regtest, `temp_id = 0xA1 * 32`:

| Scheme | `keys_id` | `funding_pubkey` |
|---|---|---|
| V0 | `a7649d1c927f2b8af024e9a1d3b59c84da975629a2c3805f16afa19090ab6115` | `03aa70703dca189d6df5cc73408d2e96f4c3f3a761f4889653984b8a694956a35a` |
| V1 | `5604be211579c6a4c28fa5e61ce059ae4f4716c2bee2da0f44444b4b45597631` | `03c952c4c3a4f7b69ab23bbc8d1d6f1a1487beca22edeb32c3116f82ac8aca7159` |

The V1 id ends in `44444b4b45597631`, the ASCII bytes of `DDKKEYv1`.
