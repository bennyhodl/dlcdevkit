#!/bin/bash

while [ "`docker inspect -f {{.State.Status}} $1`" != "running" ]; do
     sleep 2;
done

docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=$BITCOIND_USER --rpcpassword=$BITCOIND_PASS createwallet ddk
docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=$BITCOIND_USER --rpcpassword=$BITCOIND_PASS --rpcwallet=ddk -generate 101
