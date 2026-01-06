# ddk

[![Crate](https://img.shields.io/crates/v/ddk.svg?logo=rust)](https://crates.io/crates/ddk)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk&color=informational)](https://docs.rs/ddk)

The main DLC Dev Kit library for building DLC applications with an integrated BDK wallet.

DDK provides a high-level, pluggable framework with an actor-based architecture for thread-safe, lock-free DLC operations. It integrates wallet management, transport layers, storage backends, and oracle clients into a unified API.

## Usage

```
$ cargo add ddk
```

## Example

```rust
use ddk::builder::{Builder, SeedConfig};
use ddk::oracle::kormir::KormirOracleClient;
use ddk::storage::sled::SledStorage;
use ddk::transport::lightning::LightningTransport;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[tokio::main]
async fn main() -> Result<(), ddk::error::Error> {
    let transport = Arc::new(LightningTransport::new(&[0u8; 32], 1776, logger.clone())?);
    let storage = Arc::new(SledStorage::new("/tmp/ddk-example", logger.clone())?);
    let oracle = Arc::new(KormirOracleClient::new("http://localhost:8082", None, logger.clone()).await?);

    let mut builder: Builder<LightningTransport, SledStorage, KormirOracleClient> = Builder::new();
    builder.set_seed_bytes(SeedConfig::Random)?;
    builder.set_transport(transport);
    builder.set_storage(storage);
    builder.set_oracle(oracle);

    let ddk: ApplicationDdk = builder.finish().await?;

    ddk.start()?;

    // Send a DLC offer
    let offer = ddk.send_dlc_offer(&contract_input, counterparty, announcements).await?;

    // Accept a DLC offer  
    let (contract_id, counter_party, accept) = ddk.accept_dlc_offer(contract_id).await?;

    // Get wallet balance
    let balance = ddk.balance().await?;

    ddk.stop()?;
    Ok(())
}
```

## Key Types

| Type | Description |
|------|-------------|
| [`DlcDevKit`](https://docs.rs/ddk/latest/ddk/struct.DlcDevKit.html) | Main entry point managing the DLC runtime, wallet, and background tasks |
| [`Builder`](https://docs.rs/ddk/latest/ddk/builder/struct.Builder.html) | Builder pattern for constructing `DlcDevKit` instances |
| [`Transport`](https://docs.rs/ddk/latest/ddk/trait.Transport.html) | Trait for DLC message communication between peers |
| [`Storage`](https://docs.rs/ddk/latest/ddk/trait.Storage.html) | Trait for contract and wallet data persistence |
| [`Oracle`](https://docs.rs/ddk/latest/ddk/trait.Oracle.html) | Trait for oracle client implementations |

## Features

| Feature | Description |
|---------|-------------|
| `lightning` | Lightning Network transport using LDK |
| `nostr` | Nostr protocol transport |
| `sled` | Sled embedded database storage |
| `postgres` | PostgreSQL storage |
| `kormir` | Kormir HTTP oracle client |
| `p2pderivatives` | P2P Derivatives oracle client |
| `nostr-oracle` | Nostr-based oracle client |

## License

This project is licensed under the MIT License.
