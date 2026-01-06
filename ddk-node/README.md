# ddk-node

[![Crate](https://img.shields.io/crates/v/ddk-node.svg?logo=rust)](https://crates.io/crates/ddk-node)

A ready-to-go DLC node with a gRPC server and accompanying CLI built on the DlcDevKit framework.

The node uses Nostr for transport, PostgreSQL for storage, and Kormir as the oracle client.

## Installation

```
$ cargo install ddk-node
```

## Binaries

| Binary | Description |
|--------|-------------|
| `ddk-node` | The DLC node server exposing a gRPC API |
| `ddk-cli` | Command-line client to interact with `ddk-node` |

## Node Usage

```
$ ddk-node --help

Usage: ddk-node [OPTIONS]

Options:
      --log <LOG>                  Set the log level [default: info]
  -n, --network <NETWORK>          Set the Bitcoin network [default: signet]
  -s, --storage-dir <STORAGE_DIR>  Data storage path [default: ~/.ddk]
  -p, --port <PORT>                Transport listening port [default: 1776]
      --grpc <GRPC_HOST>           gRPC server host:port [default: 0.0.0.0:3030]
      --esplora <ESPLORA_HOST>     Esplora server URL [default: https://mutinynet.com/api]
      --oracle <ORACLE_HOST>       Kormir oracle URL [default: https://kormir.dlcdevkit.com]
      --seed <SEED>                Seed strategy: 'file' or 'bytes' [default: file]
      --postgres-url <URL>         PostgreSQL connection URL
  -h, --help                       Print help
```

## CLI Usage

```
$ ddk-cli --help

CLI for ddk-node

Usage: ddk-cli [OPTIONS] <COMMAND>

Commands:
  info            Get node information (pubkey, transport, oracle)
  offer-contract  Send a contract offer to a counterparty
  offers          List received contract offers
  accept-offer    Accept a DLC offer by contract ID
  contracts       List all contracts
  balance         Get wallet balance
  wallet          Wallet commands (new-address, transactions, utxos, send, sync)
  oracle          Oracle commands (announcements, create-enum, create-numeric, sign)
  peers           List connected peers
  connect         Connect to another DDK node
  sync            Sync wallet and contracts
  help            Print help

Options:
  -s, --server <SERVER>  gRPC server to connect to [default: http://127.0.0.1:3030]
  -h, --help             Print help
```

## gRPC API

The node exposes a gRPC service with the following methods:

| Method | Description |
|--------|-------------|
| `Info` | Get node info (pubkey, transport, oracle) |
| `SendOffer` | Send a DLC offer to a counterparty |
| `AcceptOffer` | Accept a received DLC offer |
| `ListOffers` | List all received contract offers |
| `ListContracts` | List all contracts |
| `NewAddress` | Generate a new wallet address |
| `WalletBalance` | Get wallet balance |
| `WalletSync` | Sync the on-chain wallet |
| `GetWalletTransactions` | Get wallet transactions |
| `ListUtxos` | List wallet UTXOs |
| `Send` | Send Bitcoin to an address |
| `ListPeers` | List connected peers |
| `ConnectPeer` | Connect to another DDK node |
| `ListOracles` | Get oracle info |
| `OracleAnnouncements` | Get oracle announcement by event ID |
| `CreateEnum` | Create an enum oracle event |
| `CreateNumeric` | Create a numeric oracle event |
| `SignAnnouncement` | Sign an oracle announcement |

## Development

```bash
# Start bitcoin, esplora, and postgres
$ just deps

# Start node one
$ just node-one

# Start node two (in another terminal)
$ just node-two

# Use the CLI
$ just cli-one info
$ just cli-two info

# Connect nodes
$ PUBKEY=$(just cli-one info | jq -r .pubkey)
$ just cli-two connect $PUBKEY@127.0.0.1:1776

# Create and accept a contract
$ just cli-two offer-contract $PUBKEY
$ just cli-one offers
$ just cli-one accept-offer <CONTRACT_ID>
```

## License

This project is licensed under the MIT License.
