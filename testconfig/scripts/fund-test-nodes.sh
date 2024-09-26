NODE_ONE_PUBKEY=$(just cli-one info | jq -r '.pubkey')

just cli-two connect $NODE_ONE_PUBKEY@127.0.0.1:1776

NODE_ONE_ADDRESS=$(just cli-one wallet new-address | jq -r '.address')
NODE_TWO_ADDRESS=$(just cli-two wallet new-address | jq -r '.address')

just bc sendtoaddress $NODE_ONE_ADDRESS 1.0
just bc sendtoaddress $NODE_TWO_ADDRESS 1.0

just bc -generate 5

