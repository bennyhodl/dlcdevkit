# ddk-manager

[![Crate](https://img.shields.io/crates/v/ddk-manager.svg?logo=rust)](https://crates.io/crates/ddk-manager)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk-manager&color=informational)](https://docs.rs/ddk-manager)

Core DLC contract creation and state machine management for Discreet Log Contracts.

This crate provides the `Manager` component for creating, processing, and managing DLCs. It handles the full on-chain contract lifecycle from offer through settlement or closure.

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
| `Storage` | Persist and retrieve contracts |
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

// Periodic maintenance
manager.periodic_check().await?;
```

## Features

| Feature | Description |
|---------|-------------|
| `std` | Standard library support (default) |
| `parallel` | Parallel processing in ddk-trie |
| `use-serde` | Serde serialization support |

## License

This project is licensed under the MIT License.
