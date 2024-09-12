# DDK Node

[![Crate](https://img.shields.io/crates/v/ddk-node.svg?logo=rust)](https://crates.io/crates/ddk-node)

Dlc Dev Kit node example with [`ddk`](../ddk), [`lightning`](../ddk/src/transport/lightning/), [`sled`](../ddk/src/storage/sled/), and [`kormir`](../ddk/src/oracle/kormir.rs).

Node binary is available through [`crates.io`](https://crates.io/crates/ddk-node).

```
$ cargo install ddk-node
```
## Usage
The downloaded binary included the node binary and a cli interface.

```
$ ddk-node --help

Usage: ddk-node [OPTIONS]

Options:
      --log <LOG>                  Set the log level. [default: info]
  -n, --network <NETWORK>          Set the Bitcoin network for DDK [default: regtest]
  -s, --storage-dir <STORAGE_DIR>  The path where DlcDevKit will store data.
  -p, --port <LISTENING_PORT>      Listening port for network transport. [default: 1776]
      --grpc <GRPC_HOST>           Host and port the gRPC server will run on. [default: 0.0.0.0:3030]
      --esplora <ESPLORA_HOST>     Host to connect to an esplora server. [default: http://127.0.0.1:30000]
      --oracle <ORACLE_HOST>       Host to connect to an oracle server. [default: http://127.0.0.1:8082]
      --seed <SEED>                Seed config strategy ('bytes' OR 'file') [default: file]
  -h, --help                       Print help
```

```
$ ddk-cli --help

CLI for ddk-node

Usage: ddk-cli [OPTIONS] <COMMAND>

Commands:
  info            Gets information about the DDK instance
  offer-contract  Pass a contract input to send an offer
  offers          Retrieve the offers that ddk-node has received
  accept-offer    Accept a DLC offer with the contract id string
  contracts       List contracts
  wallet          Wallet commands
  peers           Get the peers connected to the node
  connect         Connect to another DDK node
  help            Print this message or the help of the given subcommand(s)

Options:
  -s, --server <SERVER>  ddk-node gRPC server to connect to. [default: http://127.0.0.1:3030]
  -h, --help             Print help
  -V, --version          Print version
```

## Development

If you are testing local changes to [`ddk`](../ddk/) or running `ddk-node` locally:

```
# Start bitcoin and esplora
$ just deps

# Start kormir server that ddk-node runs
$ just kormir

$ just node-one # Start node one 
$ just node-two # Start node two in a different terminal

$ just bc ...ARGS # Interface with the Bitcoin node.
```

To interface the nodes with the CLI, you can use `just cli-one` and `just cli-two`.

To create an enum event with `kormir`:

1. Create the event
```
curl --location 'http://localhost:8082/create-enum' \
--header 'Content-Type: application/json' \
--data '{
    "event_id": "EVENT ID",
    "outcomes": [
        ...OUTCOMES
    ],
    "event_maturity_epoch": UNIX_TIME
}'
```

2. Connect two ddk-nodes
```
$ PUBKEY=$(just cli-one info | jq -r .pubkey) && just cli-two connect $PUBKEY@127.0.0.1:1776
```

3. Offer Contract 
```
$ just cli-two offer-contract $PUBKEY # Follow the prompts and input the outcomes and payouts
```

4. Accept Contract
```
$ just cli-one offers # Select the recently created offer

$ just cli-one accept-contract <CONTRACT ID>
```
