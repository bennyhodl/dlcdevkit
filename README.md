# DLC Dev Kit

[![Crate](https://img.shields.io/crates/v/dlcdevkit.svg?logo=rust)](https://crates.io/crates/ddk)
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
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::oracle::P2PDOracleClient;
use ddk::Network;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let transport = Arc::new(LightningTransport::new("lightning-transport", Network::Regtest));
    let storage = Arc::new(SledStorageProvider::new("<storage path>")?);
    let oracle_client = tokio::task::spawn_blocking(move || Arc::new(P2PDOracleClient::new("<oracle host>")?)).await?;

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name("dlcdevkit")
        .set_esplora_url(ddk::ESPLORA_HOST)
        .set_network(bitcoin::Network::Regtest)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .await?;

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().await?;
}
```

## Crates
Ready-to-go clients for developing applications:
* `ddk` - Contains DLC management w/ [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and the internal wallet w/ [bdk](https://github.com/bitcoindevkit/bdk).

### Storage
* `filestore` - **crate soon™️**
* `sqlite` - **crate soon™️**

### Transport
* `tcp (lightning p2p)` - Tcp listener with the [ldk peer manager](https://lightningdevkit.org/introduction/peer-management/)
* `nostr` - NIP04 encrypted transport

### Oracle
* `P2PDerivatives` - **crate soon™️**
* `kormir` - **crate soon™️**

### Examples
* [`bella`](https://github.com/bennyhodl/dlcdevkit/bella) - Example client built with [`tauri`](https://tauri.app) to test `dlcdevkit`
* [`payouts`](https://github.com/bennyhodl/dlcdevkit/payouts) - example payout curves for DLC applications

## Development

Running the example client [`bella`](https://github.com/bennyhodl/dlcdevkit/bella) requires running a bitcoin node, esplora server, & oracle. Dependencies can be started with the `docker-compose.yaml` file.

```
git clone git@github.com:bennyhodl/dlcdevkit.git
cd dlcdevkit

docker-compose up -d --build

# Alias for interacting w/ bitcoin node
source alias
bc -generate

cd bella && pnpm install && pnpm tauri dev
```

