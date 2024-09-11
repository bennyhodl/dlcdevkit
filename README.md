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

fn main() -> Result<(), Error> {
    let transport = Arc::new(LightningTransport::new("lightning-transport", Network::Regtest));
    let storage = Arc::new(SledStorageProvider::new("<storage path>")?);
    let oracle_client = Arc::new(P2PDOracleClient::new("<oracle host>")?);

    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name("dlcdevkit")
        .set_esplora_url(ddk::ESPLORA_HOST)
        .set_network(bitcoin::Network::Regtest)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .expect("could not build ddk");

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().expect("ddk did not start");
}
```

## Crates
Ready-to-go clients for developing applications:
* [`ddk`](./ddk/) - Contains DLC management w/ [rust-dlc](https://github.com/p2pderivatives/rust-dlc) and the internal wallet w/ [bdk](https://github.com/bitcoindevkit/bdk).

### Storage
* [`filestore`](./ddk/src/storage/sled.rs) - file storage for DLC contracts
* `sqlite` - coming soon...

### Transport
* [`tcp (lightning p2p)`](./ddk/src/transport/lightning/) - Tcp listener with the [ldk peer manager](https://lightningdevkit.org/introduction/peer-management/)
* [`nostr`](./ddk/src/transport/nostr/) - NIP04 encrypted transport

### Oracle Clients
* [`P2PDerivatives`](./ddk/src/oracle/p2p_derivatives.rs)
* [`kormir`](./ddk/src/oracle/kormir.rs)

## Development

Start by cloning the repo and opening the directory:
```
git clone https://github.com/bennyhodl/dlcdevkit.git
cd dlcdevkit
```

Next you will need to create and configure a testconfig directory, modifying the path in the commands below to point to your local /dlcdevkit/ directory:
```
# Create the testconfig directory and expected sub-directories
mkdir -p /path/to/your/dlcdevkit/testconfig/oracle/certs/db

# Generate a private key via openssl
openssl genrsa -out db.key 2048
# Generate a self-signed certificate via openssl
openssl req -new -x509 -key db.key -out db.crt -days 365 -subj "/CN=localhost"

# Move the key and cert into the testconfig directory
mv db.crt /path/to/your/dlcdevkit/testconfig/oracle/certs/db/
mv db.key /path/to/your/dlcdevkit/testconfig/oracle/certs/db/
```

Next you will need to create the oracledb.dockerfile for your testconfig.

Finally, build and spin up your containers via
```
docker-compose up -d --build
```

# Alias for interacting w/ bitcoin node
source alias
bc -generate