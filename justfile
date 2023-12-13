env:
  nigiri start
  RUST_LOG=info /Users/ben/ernest/nostr-rs-relay/target/release/nostr-rs-relay -d /Users/ben/ernest/nostr-relay-db

relay:
  RUST_LOG=info /Users/ben/ernest/nostr-rs-relay/target/release/nostr-rs-relay -d /Users/ben/ernest/nostr-relay-db
  
