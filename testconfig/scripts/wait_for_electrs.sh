#!/bin/bash

until $(curl --output /dev/null --silent --fail http://localhost:30000/blocks/tip/height); do
    printf 'waiting for electrs to start'
    docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=$BITCOIND_USER --rpcpassword=$BITCOIND_PASS -rpcwallet=$RPC_WALLET getblockcount
    docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=$BITCOIND_USER --rpcpassword=$BITCOIND_PASS -rpcwallet=$RPC_WALLET -generate 1
    sleep 5
done