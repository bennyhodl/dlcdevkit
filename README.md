# DLC Dev Kit

[![Crate](https://img.shields.io/crates/v/ddk.svg?logo=rust)](https://crates.io/crates/ddk)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk&color=informational)](https://docs.rs/ddk)
![Crates.io Total Downloads](https://img.shields.io/crates/d/ddk)

A ready-to-go [Discreet Log Contract](https://github.com/discreetlogcontracts/dlcspecs) node library built using [BDK](https://github.com/bitcoindevkit/bdk).

DLC Dev Kit is a self-custodial DLC node in library form. Its central goal is to provide a small, simple, and straightforward interface that enables users to easily set up and run a DLC node with an integrated on-chain wallet. While minimalism is at its core, DDK aims to be sufficiently modular and configurable to be useful for a variety of use cases.

## Getting Started

The primary abstraction of the library is the [`DlcDevKit`](https://docs.rs/ddk/latest/ddk/struct.DlcDevKit.html), which can be retrieved by setting up and configuring a [`Builder`](https://docs.rs/ddk/latest/ddk/builder/struct.Builder.html) to your liking and calling `finish()`. `DlcDevKit` can then be controlled via commands such as `start`, `stop`, `send_dlc_offer`, `accept_dlc_offer`, etc.

```rust
use ddk::builder::{Builder, SeedConfig};
use ddk::logger::{LogLevel, Logger};
use ddk::oracle::kormir::KormirOracleClient;
use ddk::storage::sled::SledStorage;
use ddk::transport::lightning::LightningTransport;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[tokio::main]
async fn main() -> Result<(), ddk::error::Error> {
    let logger = Arc::new(Logger::console("ddk-example".to_string(), LogLevel::Info));

    let transport = Arc::new(LightningTransport::new(&[0u8; 32], 1776, logger.clone())?);
    let storage = Arc::new(SledStorage::new("/tmp/ddk-example", logger.clone())?);
    let oracle = Arc::new(KormirOracleClient::new("http://localhost:8082", None, logger.clone()).await?);

    let mut builder: Builder<LightningTransport, SledStorage, KormirOracleClient> = Builder::new();
    builder.set_seed_bytes(SeedConfig::Random)?;
    builder.set_esplora_host("https://mutinynet.com/api".to_string());
    builder.set_transport(transport);
    builder.set_storage(storage);
    builder.set_oracle(oracle);
    builder.set_logger(logger);

    let ddk: ApplicationDdk = builder.finish().await?;

    ddk.start()?;

    // ... open contracts, accept offers, etc.

    ddk.stop()?;
    Ok(())
}
```

## Modularity

DDK is designed with a pluggable architecture, allowing you to choose or implement your own components:

- **Transport**: Communication layer for DLC messages between peers. Implementations include Lightning Network gossip and Nostr protocol messaging.
- **Storage**: Persistence backend for contracts and wallet data. Implementations include Sled (embedded) and PostgreSQL.
- **Oracle**: External data source for contract attestations. Implementations include HTTP and Nostr-based oracle clients.

You can create a custom DDK instance by implementing the required traits defined in [`ddk/src/lib.rs`](./ddk/src/lib.rs).

## Crates

| Crate | Description | |
|-------|-------------|---------|
| [`ddk`](./ddk) | The main DDK library with an integrated BDK wallet for building DLC applications. | [![Crate](https://img.shields.io/crates/v/ddk.svg)](https://crates.io/crates/ddk) |
| [`ddk-node`](./ddk-node) | A ready-to-go DLC node with a gRPC server and accompanying CLI. | [![Crate](https://img.shields.io/crates/v/ddk-node.svg)](https://crates.io/crates/ddk-node) |
| [`ddk-payouts`](./payouts) | Functions to build payout curves for DLC contracts. | [![Crate](https://img.shields.io/crates/v/ddk-payouts.svg)](https://crates.io/crates/ddk-payouts) |
| [`ddk-manager`](./ddk-manager) | Core DLC contract creation and state machine management. | [![Crate](https://img.shields.io/crates/v/ddk-manager.svg)](https://crates.io/crates/ddk-manager) |
| [`ddk-dlc`](./dlc) | Low-level DLC transaction creation, signing, and verification. | [![Crate](https://img.shields.io/crates/v/ddk-dlc.svg)](https://crates.io/crates/ddk-dlc) |
| [`ddk-messages`](./dlc-messages) | Serialization and structs for the DLC protocol messages. | [![Crate](https://img.shields.io/crates/v/ddk-messages.svg)](https://crates.io/crates/ddk-messages) |
| [`ddk-trie`](./dlc-trie) | Data structures for storage and retrieval of numerical DLCs. | [![Crate](https://img.shields.io/crates/v/ddk-trie.svg)](https://crates.io/crates/ddk-trie) |
| [`kormir`](./kormir) | Oracle implementation for creating and attesting to DLC events. | [![Crate](https://img.shields.io/crates/v/kormir.svg)](https://crates.io/crates/kormir) |

## Development

A bitcoin node, esplora server, and oracle server are required to run DDK. Developers can spin up a development environment with the `justfile` provided.

```
$ just deps
```

To run your own [Kormir](https://github.com/bennyhodl/kormir) oracle server for development, see the Kormir repository.

See the [ddk-node README](./ddk-node/README.md) for more development information.

## Language Bindings

For Node.js and React Native bindings, see [ddk-ffi](https://github.com/bennyhodl/ddk-ffi).

## Resources

- [DLC Dev Kit Blog](https://dlcdevkit.com) - Guides and API walkthroughs
- [DLC Dev Kit: Beyond](https://benschroth.com/blog/dlcdevkit-beyond/) - Deep dive into the project
- [What is a Discreet Log Contract?](https://river.com/learn/terms/d/discreet-log-contract-dlc/) - Learn about DLCs
- [DLC Specifications](https://github.com/discreetlogcontracts/dlcspecs) - Protocol specification
- [rust-dlc](https://github.com/p2pderivatives/rust-dlc) - The original rust-dlc implementation

## License

This project is licensed under the [MIT License](LICENSE).
