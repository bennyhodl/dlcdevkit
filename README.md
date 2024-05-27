# DLC Dev Kit

[![Crate](https://img.shields.io/crates/v/dlcdevkit.svg?logo=rust)](https://crates.io/crates/ldk-node)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=dlcdevkit&color=informational)](https://docs.rs/dlcdevkit)
![Crates.io Total Downloads](https://img.shields.io/crates/d/dlcdevkit)

> :warning: `dlcdevkit` is alpha software and should not be used with real money. API is subject to change.

Application tooling to get started with [DLCs](https://github.com/discreetlogcontracts/dlcspecs) build with [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and [bdk](https://github.com/bitcoindevkit/bdk).

Build DLC application by plugging in your own transport, storage, and oracle clients.

## Get Started

```rust
use dlcdevkit::Builder;
use dlcdevkit::storage::FileStore;
use dlcdevkit::transport::TcpTransport;
use dlcdevkit::oracle::P2PDerivativesOracle;

#[tokio::main]
async fn main() {
    let mut builder = Builder::new();

    let storage = FileStore::new();
    let transport = TcpTransport::new();
    let oracle = P2PDerivativesOracle::new();

    builder.set_espolora_url("https://mempool.space");
    builder.set_storage(storage);
    builder.set_transport(transport);
    builder.set_oracle(oracle);
    
    let ddk = builder.finish().unwrap();

    ddk.start().unwrap();
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

