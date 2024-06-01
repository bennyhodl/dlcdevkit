# DLC Dev Kit

[![Crate](https://img.shields.io/crates/v/dlcdevkit.svg?logo=rust)](https://crates.io/crates/ldk-node)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=dlcdevkit&color=informational)](https://docs.rs/dlcdevkit)
![Crates.io Total Downloads](https://img.shields.io/crates/d/dlcdevkit)

> :warning: `dlcdevkit` is alpha software and should not be used with real money. API is subject to change.

Application tooling to get started with [DLCs](https://github.com/discreetlogcontracts/dlcspecs) build with [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and [bdk](https://github.com/bitcoindevkit/bdk).

Build DLC application by plugging in your own transport, storage, and oracle clients.

## Get Started
```
$ cargo add ddk
```

```rust
use ddk::builder::DdkBuilder;
use ddk::{DdkOracle, DdkStorage, DdkTransport};
use std::sync::Arc;

#[derive(Clone)]
pub struct MockTransport;

#[async_trait]
impl DdkTransport for MockTransport {
    async fn listen(&self) {
        println!("Listening with MockTransport")
    }
    async fn handle_dlc_message(&self, _manager: &Arc<Mutex<DlcDevKitDlcManager>>) {
        println!("Handling DLC messages with MockTransport")
    }
}

#[derive(Clone)]
struct MockStorage;
impl DdkStorage for MockStorage {}

#[derive(Clone)]
struct MockOracle;
impl DdkOracle for MockOracle {}

type ApplicationDdk = ddk::DlcDevKit<MockTransport, MockStorage, MockOracle>;

#[tokio::main]
async fn main() {
    let transport = Arc::new(MockTransport {});
    let storage = Arc::new(MockStorage {});
    let oracle_client = Arc::new(MockOracle {});

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name("ddk")
        .set_esplora_url("https://mempool.space/api")
        .set_network(bitcoin::Network::Regtest)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .await
        .unwrap();

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().await
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

