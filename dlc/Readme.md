# ddk-dlc

[![Crate](https://img.shields.io/crates/v/ddk-dlc.svg?logo=rust)](https://crates.io/crates/ddk-dlc)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-dlc&color=informational)](https://docs.rs/ddk-dlc)

Low-level primitives for creating, signing, and verifying Bitcoin transactions used in Discreet Log Contract (DLC) protocols.

This crate handles all DLC transaction types including funding, contract execution transactions (CETs), refund transactions, and DLC channel operations.

## Transaction Creation

```rust
use ddk_dlc::{create_dlc_transactions, PartyParams};

// Create complete set of DLC transactions
let dlc_txs = create_dlc_transactions(
    &offer_params,
    &accept_params,
    &payouts,
    refund_lock_time,
    fee_rate,
    fund_lock_time,
    cet_lock_time,
    fund_output_serial_id,
)?;

// Access individual transactions
let fund_tx = dlc_txs.fund;
let cets = dlc_txs.cets;
let refund_tx = dlc_txs.refund;
```

## Adaptor Signatures

```rust
use ddk_dlc::{create_cet_adaptor_sig_from_oracle_info, sign_cet};

// Create adaptor signature for CET
let adaptor_sig = create_cet_adaptor_sig_from_oracle_info(
    &secp,
    &cet,
    &adaptor_info,
    &funding_script_pubkey,
    fund_output_value,
    &secret_key,
)?;

// Sign CET with oracle attestation
let signed_cet = sign_cet(
    &secp,
    &cet,
    &adaptor_sig,
    &oracle_signatures,
    &funding_script_pubkey,
    fund_output_value,
    &own_secret_key,
)?;
```

## Key Functions

| Function | Description |
|----------|-------------|
| `create_dlc_transactions` | Create complete set of DLC transactions |
| `create_fund_transaction` | Create 2-of-2 multisig funding transaction |
| `create_cet` / `create_cets` | Create contract execution transaction(s) |
| `create_refund_transaction` | Create refund transaction |
| `create_cet_adaptor_sig_from_oracle_info` | Create adaptor signature for CET |
| `sign_cet` | Sign CET with oracle attestation |
| `verify_cet_adaptor_sig_from_oracle_info` | Verify adaptor signature |

## Channel Operations

The `channel` module provides functions for DLC channels:

- `create_channel_transactions` - Create DLC channel transactions with revocation
- `create_buffer_transaction` - Create buffer transaction for revocation
- `create_settle_transaction` - Create settle transaction
- `create_collaborative_close_transaction` - Create collaborative close

## Features

| Feature | Description |
|---------|-------------|
| `std` | Standard library support (default) |
| `no-std` | No standard library for embedded/WASM |
| `use-serde` | Serde serialization support |

## License

This project is licensed under the MIT License.
