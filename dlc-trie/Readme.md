# ddk-trie

[![Crate](https://img.shields.io/crates/v/ddk-trie.svg?logo=rust)](https://crates.io/crates/ddk-trie)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-trie&color=informational)](https://docs.rs/ddk-trie)

Trie-based data structures for efficient storage and retrieval of adaptor signature information in numerical Discreet Log Contracts (DLCs).

This crate enables DLCs to handle continuous outcome ranges (e.g., prices, scores) by decomposing numeric values into digit paths, minimizing the number of adaptor signatures required.

## Key Structures

| Structure | Description |
|-----------|-------------|
| `DigitTrie<T>` | Base trie indexed by digit paths (decomposed numeric values) |
| `MultiTrie<T>` | Trie of tries for multi-oracle t-of-n threshold schemes |
| `MultiOracleTrie` | Optimized for oracles that must exactly agree |
| `MultiOracleTrieWithDiff` | Allows differences between oracle outcomes within bounds |

## How It Works

1. **Digit Decomposition**: Numeric outcomes (e.g., price = 1234) are decomposed into digit paths (`[1, 2, 3, 4]` in base 10)

2. **Prefix Compression**: Ranges sharing common prefixes are covered by a single trie node. For example, outcomes 1000-1999 can be covered by prefix `[1]`

3. **Efficient Signatures**: The `DlcTrie` trait provides:
   - `generate()` - Build trie structure from range payouts
   - `sign()` - Create adaptor signatures for all paths
   - `verify()` - Verify adaptor signatures

## Example

```rust
use ddk_trie::{DlcTrie, OracleNumericInfo};

let oracle_info = OracleNumericInfo {
    base: 2,
    nb_digits: vec![20], // 20-bit precision
};

// Generate trie from payouts
let trie_info = trie.generate(&payouts, &oracle_info)?;

// Sign all paths
let signatures = trie.sign(&secp, &funding_info, &secret_key)?;

// Verify signatures
trie.verify(&secp, &funding_info, &adaptor_sigs, &public_key)?;
```

## Features

| Feature | Description |
|---------|-------------|
| `std` | Standard library support (default) |
| `no-std` | No standard library for embedded/WASM |
| `parallel` | Parallel signature generation/verification using rayon |
| `use-serde` | Serde serialization support |

## License

This project is licensed under the MIT License.
