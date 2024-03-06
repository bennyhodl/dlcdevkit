env:
  nigiri start
  RUST_LOG=info /Users/ben/ernest/nostr-rs-relay/target/release/nostr-rs-relay -d /Users/ben/ernest/nostr-relay-db

relay:
  RUST_LOG=info /Users/ben/ernest/relays/nostr-rs-relay/target/release/nostr-rs-relay -d /Users/ben/ernest/relays/nostr-relay-db -c /Users/ben/ernest/relays/config.toml

oracle:
  docker run --name kormir -e POSTGRES_PASSWORD=kormir -e POSTGRES_USER=kormir -d -p 5432:5432 postgres
  RUST_LOG=info KORMIR_RELAYS=ws://localhost:8081 DATABASE_URL=postgres://kormir:kormir@localhost:5432 KORMIR_KEY=24fb734c68ff159f649a6f78c32e244c8475227acf2820abfc5121fd8b724054 /Users/ben/ernest/oracles/kormir/target/release/kormir-server 
