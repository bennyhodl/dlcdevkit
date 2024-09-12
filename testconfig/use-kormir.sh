#!/bin/bash

DIR=$(git rev-parse --show-toplevel 2>/dev/null)
# KOMIR
REPO_URL="https://github.com/bennyhodl/kormir.git"
FOLDER_NAME="$DIR/testconfig/kormir"
BRANCH_NAME="bitcoin-32"

KORMIR_BINARY=$DIR/testconfig/kormir-server

clone_and_build_kormir() {
  # Check if the folder exists
  if [ ! -d "$FOLDER_NAME" ]; then
    echo "Folder does not exist. Cloning from GitHub..."
    git clone -b $BRANCH_NAME $REPO_URL $FOLDER_NAME
    if [ $? -ne 0 ]; then
        echo "Failed to clone repository. Exiting."
        exit 1
    fi
  else
    echo "Folder already exists."
  fi

  # Change to the project directory
  cd $FOLDER_NAME

  # Check if it's a Rust project (look for Cargo.toml)
  if [ ! -f "Cargo.toml" ]; then
    echo "Cargo.toml not found. Are you sure this is a Rust project?"
    exit 1
  fi

  # Build the Rust project in release mode
  echo "Building kormir-server project in release mode..."
  cargo build -p kormir-server --release

  if [ $? -eq 0 ]; then
    echo "Kormir build completed successfully!"
    mv $FOLDER_NAME/target/release/kormir-server $DIR/testconfig
    rm -rf $FOLDER_NAME
  else
    echo "Kormir build failed."
    exit 1
  fi
}

# Check if the specific file exists
if [ ! -f "$KORMIR_BINARY" ]; then
  clone_and_build_kormir
fi

RUST_LOG=info KORMIR_PORT=8082 KORMIR_RELAYS=ws://localhost:8081 \
  DATABASE_URL=postgres://kormir:kormir@localhost:5433 \
  KORMIR_KEY=34d95a073eee38ecb968a0da8273926cda601802541a715c011fb340dd6d1706 \
  $KORMIR_BINARY
