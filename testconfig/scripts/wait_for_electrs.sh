#!/bin/bash

until $(curl --output /dev/null --silent --fail http://localhost:30000/blocks/tip/height); do
    printf 'waiting for electrs to start'
    docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=ddk --rpcpassword=ddk -rpcwallet=ddk getblockcount
    docker exec bitcoin bitcoin-cli --rpcport=18443 --rpcuser=ddk --rpcpassword=ddk -rpcwallet=ddk -generate 1
    sleep 5
done