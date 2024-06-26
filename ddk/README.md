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
use ddk::config::DdkConfig;
use ddk::builder::DdkBuilder;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use std::env::current_dir;
use std::sync::Arc;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn main() -> Result<()> {
    let mut config = DdkConfig::default();
    config.storage_path = current_dir()?;

    let transport = Arc::new(LightningTransport::new(&config.seed_config, config.network)?);
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().expect("No storage."),
    )?);

    let oracle_client = Arc::new(P2PDOracleClient::new(ddk::ORACLE_HOST).expect("no oracle"));

    let mut builder = DdkBuilder::new();
    builder.set_config(config);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().expect("couldn't start ddk");
}
```

## Crates
Ready-to-go clients for developing applications:
* `ddk` - Contains DLC management w/ [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and the internal wallet w/ [bdk](https://github.com/bitcoindevkit/bdk).

### Storage
* `filestore` - flat file store for DLC contracts
* `sqlite` - sqlite store for DLC contracts

### Transport
* [`tcp (lightning p2p)`](https://github.com/bennyhodl/dlcdevkit/tree/master/ddk/src/transport/lightning) - Tcp listener with the [ldk peer manager](https://lightningdevkit.org/introduction/peer-management/)
* [`nostr`](https://github.com/bennyhodl/dlcdevkit/tree/master/ddk/src/transport/nostr) - NIP04 encrypted transport

### Oracle Clients
* [`P2PDerivatives`](https://github.com/bennyhodl/dlcdevkit/blob/master/ddk/src/oracle/p2p_derivatives.rs) 
* [`kormir`](https://github.com/bennyhodl/dlcdevkit/blob/master/ddk/src/oracle/kormir.rs)
