#!/bin/bash

set -e

# Contract states we want to capture from the enumeration test
CONTRACT_STATES=("OfferedContract" "AcceptedContract" "SignedContract" "ConfirmedContract" "PreClosedContract" "ClosedContract")

# Destination folder at top level
DEST=${PWD}/contract_binaries/

# Create the destination directory if it doesn't exist
mkdir -p ${DEST}

echo "Starting enumeration contract binary generation..."

# Start Docker services
docker-compose up -d
./testconfig/scripts/wait_for_electrs.sh

# Run the enumeration test with serialization enabled
echo "Running enumeration test with contract serialization..."
GENERATE_SERIALIZED_CONTRACT=1 cargo test -p ddk --test enumeration enumeration_contract

# Check that generated files are in the contract_binaries folder
echo "Checking generated contract files in ${DEST}..."

for STATE in "${CONTRACT_STATES[@]}"
do
    # The generated files should be directly in contract_binaries with state names
    FILE_NAME=${STATE/Contract/}
    if [ -f "${DEST}${FILE_NAME}" ]; then
        echo "✓ Found ${FILE_NAME} in contract_binaries"
        # Rename to include "Contract" suffix for consistency
        mv "${DEST}${FILE_NAME}" "${DEST}${STATE}"
        echo "✓ Renamed to ${STATE}"
    else
        echo "⚠ Warning: ${FILE_NAME} not found in contract_binaries"
    fi
done

# Stop Docker services
docker-compose down -v

echo "Contract binary generation complete! Files saved to ${DEST}"