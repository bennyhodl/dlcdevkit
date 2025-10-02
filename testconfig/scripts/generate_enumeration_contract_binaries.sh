#!/bin/bash

set -e

# Contract states we want to capture from the enumeration test
CONTRACT_STATES=("OfferedContract" "AcceptedContract" "SignedContract" "ConfirmedContract" "PreClosedContract" "ClosedContract")

# Destination folder at top level
DEST=${PWD}/testconfig/contract_binaries/

# Create the destination directory if it doesn't exist
mkdir -p ${DEST}

echo "Starting enumeration contract binary generation..."

./testconfig/scripts/wait_for_electrs.sh

# Run the enumeration test with serialization enabled
echo "Running enumeration test with contract serialization..."
GENERATE_SERIALIZED_CONTRACT=1 cargo test -p ddk --test enumeration enumeration_contract -- --ignored

# Check that generated files are in the contract_binaries folder
echo "Checking generated contract files in ${DEST}..."

for STATE in "${CONTRACT_STATES[@]}"
do
    # The test generates files with full state names (e.g., "OfferedContract")
    # But we want them without the "Contract" suffix (e.g., "Offered")
    FILE_NAME=${STATE/Contract/}
    if [ -f "${DEST}${STATE}" ]; then
        echo "✓ Found ${STATE} in contract_binaries"
        # Rename to remove "Contract" suffix
        mv "${DEST}${STATE}" "${DEST}${FILE_NAME}"
        echo "✓ Renamed to ${FILE_NAME}"
    else
        echo "⚠ Warning: ${STATE} not found in contract_binaries"
    fi
done

echo "Contract binary generation complete! Files saved to ${DEST}"