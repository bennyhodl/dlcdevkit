#!/bin/bash

OS=$(uname -s)

for TEST_PREFIX in "$@"; do
    TEST_BIN=$(ls ./target/debug/deps/${TEST_PREFIX}* | grep -v '\.d\|\.o')
    
    if [ "$OS" = "Darwin" ]; then  # macOS
        LIST=$(${TEST_BIN} --list --format=terse | grep -v splice | sed 's/: test$/,/' | sed "s|\([^,]*\)|\"${TEST_BIN} \1\"|g")
    else  # Linux
        LIST=$(${TEST_BIN} --list --format=terse | grep -v splice | sed 's/\: test$/,/' | sed 's@[^[:space:],]\+@"'${TEST_BIN}' &"@g')
    fi

    RES+=(${LIST})
done

# Use BSD-compatible sed syntax for the final output
echo $(echo [${RES[@]}] | sed 's/,]/]/')