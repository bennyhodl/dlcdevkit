# DLC Dev Kit

[![Crate](https://img.shields.io/crates/v/ddk.svg?logo=rust)](https://crates.io/crates/ddk)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=ddk&color=informational)](https://docs.rs/ddk)
![Crates.io Total Downloads](https://img.shields.io/crates/d/ddk)

> :warning: `dlcdevkit` is alpha software and should not be used with real money. API is subject to change.

Application tooling to get started with [DLCs](https://github.com/discreetlogcontracts/dlcspecs) build with [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and [bdk](https://github.com/bitcoindevkit/bdk).

Build DLC application by plugging in your own transport, storage, and oracle clients.

## Get Started
```
$ cargo add ddk
```

```rust
use ddk::config::DdkConfig;
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::oracle::P2PDOracleClient;
use bitcoin::Network;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn main() {
    let config = DdkConfig::default();

    let transport = Arc::new(LightningTransport::new(&config.seed_config, PORT, Network::Regtest));
    let storage = Arc::new(SledStorageProvider::new("<storage path>")?);
    let oracle_client = Arc::new(P2PDOracleClient::new("<oracle host>")?);

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_config(config)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .expect("could not build ddk node");

    ddk.start().expect("ddk could not start");
}
```

## Crates
Ready-to-go clients for developing applications:
[`ddk`](./ddk/) - DLC management with an internal BDK wallet.
[`ddk-node`](./ddk-node/) - A ready-to-go node with an accompanying cli.
[`payouts`](./payouts/) - Functions to build DLC contracts.

You can create a custom DDK instance by implementing the required traits for storage and transport. DDK traits are defined in [ddk/src/lib.rs](./ddk/src/lib.rs). The traits are super traits from what is required in `bdk` and `rust-dlc`.

To quickly get started building a DDK application, there are pre-built components.

### Storage
[`sled`](./ddk/src/storage/sled) - A simple file based storage using [sled](https://crates.io/crates/sled)

### Transport
[`LDK Peer Manager`](./ddk/src/transport/lightning/) - Communication over Lightning gossip using [`rust-dlc's implementation`](https://github.com/p2pderivatives/rust-dlc/blob/master/dlc-messages/src/message_handler.rs)
[`nostr`](./ddk/src/transport/nostr/) - DLC communication from the [NIP-88 spec](https://github.com/nostr-protocol/nips/pull/919)

### Oracle Clients
[`P2PDerivatives`](./ddk/src/oracle/p2p_derivatives.rs) - Spot price futures on the Bitcoin price [repo](https://github.com/p2pderivatives/p2pderivatives-oracle)
[`kormir`](./ddk/src/oracle/kormir.rs) - Enumeration based oracle with server and nostr support [repo](https://github.com/benthecarman/kormir)

### Examples
* [`bella`](./bella) - Example client built with [`tauri`](https://tauri.app) to test `dlcdevkit`
* [`payouts`](./payouts) - example payout curves for DLC applications

## Development

A bitcoin node, esplora server, and oracle server are required to run DDK. Developers can spin up a development environment with the `justfile` provided.

```
# Starts a regtest bitcoin node, esplora server, nostr relay (optional), p2p derivatives oracle, and a kormir oracle.
$ just deps
```

Go to the README in [ddk-node](./ddk-node/README.md) to start the project's DDK node example.
