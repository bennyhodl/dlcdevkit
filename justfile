set dotenv-load
set dotenv-path := ".env" 


deps:
  - docker compose up -d

bc *args:
  - docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=$BITCOIND_USER --rpcpassword=$BITCOIND_PASS -rpcwallet=$RPC_WALLET {{args}}

echo:
  - echo $DATABASE_URL

node-one:
  - cargo run --bin ddk-node -- --network regtest --esplora $ESPLORA_HOST --name node-one --postgres-url postgres://$POSTGRES_USER:$POSTGRES_PASS@$POSTGRES_HOST/ddk_one --log debug

node-two:
  - cargo run --bin ddk-node -- --network regtest --esplora $ESPLORA_HOST --port 1777 --grpc 0.0.0.0:3031 --storage-dir ~/.ddk/node-two --name node-two --postgres-url postgres://$POSTGRES_USER:$POSTGRES_PASS@$POSTGRES_HOST/ddk_two --log debug

cli-one *args:
  - cargo run --bin ddk-cli {{args}}

cli-two *args:
  - cargo run --bin ddk-cli -- --server http://127.0.0.1:3031 {{args}}

up:
  - DATABASE_URL=$DATABASE_URL sqlx migrate run --source ddk/src/storage/postgres/migrations

down:
  - DATABASE_URL=$DATABASE_URL sqlx migrate revert --source ddk/src/storage/postgres/migrations
