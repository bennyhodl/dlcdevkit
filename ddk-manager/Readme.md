# ddk-manager

[![Crate](https://img.shields.io/crates/v/ddk-manager.svg?logo=rust)](https://crates.io/crates/ddk-manager)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-manager&color=informational)](https://docs.rs/ddk-manager)

Core DLC contract creation and state machine management for Discreet Log Contracts.

This crate provides the `Manager` component for creating, processing, and managing DLCs and DLC channels. It handles the full lifecycle from offer through settlement/closure, including both on-chain contracts and off-chain channels with support for renewals, settlements, and collaborative/unilateral closes.

## Contract States

| State | Description |
|-------|-------------|
| `Offered` | Contract has been proposed |
| `Accepted` | Counter party accepted the offer |
| `Signed` | Signatures have been exchanged |
| `Confirmed` | Funding transaction confirmed on-chain |
| `PreClosed` | CET broadcast but not fully confirmed |
| `Closed` | Contract fully settled |
| `Refunded` | Refund transaction was broadcast |

## Key Traits

Users must implement these traits for their specific backends:

| Trait | Purpose |
|-------|---------|
| `Storage` | Persist and retrieve contracts, channels, and chain state |
| `Wallet` | Address generation, UTXO management, PSBT signing |
| `Blockchain` | Transaction broadcasting, block fetching, confirmations |
| `Oracle` | Fetch oracle announcements and attestations |
| `ContractSignerProvider` | Derive contract signing keys |

## Manager API

```rust
// Contract lifecycle
manager.send_offer(&contract_input, counterparty).await?;
manager.accept_contract_offer(&contract_id).await?;
manager.on_dlc_message(&message, counterparty).await?;

// Channel operations  
manager.offer_channel(&contract_input, counterparty).await?;
manager.settle_offer(&channel_id, payout).await?;
manager.renew_offer(&channel_id, &contract_input).await?;

// Periodic maintenance
manager.periodic_check(false).await?;
```

## Features

| Feature | Description |
|---------|-------------|
| `std` | Standard library support (default) |
| `parallel` | Parallel processing in ddk-trie |
| `use-serde` | Serde serialization support |

## License

This project is licensed under the MIT License.
