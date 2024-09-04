bc *args:
  - docker exec -it ernest-bitcoin bitcoin-cli --rpcport=18443 --rpcuser=ernest --rpcpassword=ernest -rpcwallet=ddk {{args}}

node-one:
  - cargo run --bin ddk-node

node-two:
  - cargo run --bin ddk-node -- --port 1777 --grpc 0.0.0.0:3031 --storage-dir ~/.ddk/node-two

cli-one *args:
  - cargo run --bin ddk-cli {{args}}

cli-two *args:
  - cargo run --bin ddk-cli -- --server http://127.0.0.1:3031 {{args}}
