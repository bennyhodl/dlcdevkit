deps:
  - docker compose up -d

bc *args:
  - docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=ddk --rpcpassword=ddk -rpcwallet=ddk {{args}}

node-one:
  - cargo run --bin ddk-node -- --network regtest --esplora http://127.0.0.1:30000 --name node-one --postgres-url postgres://dlcdevkit:dlcdevkit@localhost:5433/ddk_one

node-two:
  - cargo run --bin ddk-node -- --network regtest --esplora http://127.0.0.1:30000 --port 1777 --grpc 0.0.0.0:3031 --storage-dir ~/.ddk/node-two --name node-two --postgres-url postgres://dlcdevkit:dlcdevkit@localhost:5433/ddk_two

cli-one *args:
  - cargo run --bin ddk-cli {{args}}

cli-two *args:
  - cargo run --bin ddk-cli -- --server http://127.0.0.1:3031 {{args}}

up:
  - DATABASE_URL=postgres://dlcdevkit:dlcdevkit@localhost:5433/ddk_one sqlx migrate run --source ddk/src/storage/postgres/migrations

down:
  - DATABASE_URL=postgres://dlcdevkit:dlcdevkit@localhost:5433/ddk_one sqlx migrate revert --source ddk/src/storage/postgres/migrations
