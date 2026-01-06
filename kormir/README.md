# kormir

[![Crate](https://img.shields.io/crates/v/kormir.svg?logo=rust)](https://crates.io/crates/kormir)
[![Documentation](https://img.shields.io/static/v1?logo=read-the-docs&label=docs.rs&message=kormir&color=informational)](https://docs.rs/kormir)

A DLC oracle implementation for creating announcements and signing attestations.

Kormir supports both enumeration and numeric events, with optional Nostr protocol integration for publishing oracle data.

## Usage

```rust
use kormir::{Oracle, OracleAnnouncement, Storage};

// Create oracle from extended private key
let oracle = Oracle::from_xpriv(storage, xpriv)?;

// Create an enumeration event
let announcement = oracle.create_enum_event(
    "game-result",
    vec!["team_a".to_string(), "team_b".to_string(), "draw".to_string()],
    maturity_epoch,
).await?;

// Sign the outcome
let attestation = oracle.sign_enum_event("game-result", "team_a").await?;
```

## Numeric Events

```rust
// Create a numeric event (base 2)
let announcement = oracle.create_numeric_event(
    "btc-price",
    Some(20),        // num_digits
    Some(false),     // is_signed
    Some(0),         // precision
    "USD".to_string(),
    maturity_epoch,
).await?;

// Sign with numeric outcome
let attestation = oracle.sign_numeric_event("btc-price", 50000).await?;
```

## Storage Trait

Implement the `Storage` trait for your backend:

```rust
#[async_trait]
pub trait Storage {
    async fn get_next_nonce_indexes(&self, num: usize) -> Result<Vec<u32>, Error>;
    async fn save_announcement(&self, announcement: OracleAnnouncement, indexes: Vec<u32>) -> Result<String, Error>;
    async fn save_signatures(&self, event_id: String, sigs: Vec<(String, Signature)>) -> Result<OracleEventData, Error>;
    async fn get_event(&self, event_id: String) -> Result<Option<OracleEventData>, Error>;
}
```

A `MemoryStorage` implementation is provided for testing.

## Nostr Integration

With the `nostr` feature, publish announcements and attestations to Nostr:

```rust
use kormir::nostr_events::{create_announcement_event, create_attestation_event};

// Get Nostr keys from oracle
let keys = oracle.nostr_keys();

// Create Nostr event for announcement (Kind 88)
let event = create_announcement_event(&keys, &announcement)?;

// Create Nostr event for attestation (Kind 89)
let event = create_attestation_event(&keys, &attestation, &announcement_event_id)?;
```

## Features

| Feature | Description |
|---------|-------------|
| `nostr` | Nostr protocol integration for publishing oracle data |

## Running a Kormir Server

See the [kormir repository](https://github.com/bennyhodl/kormir) for the HTTP server implementation.

## License

This project is licensed under the MIT License.
