//! An in-process nostr relay, replacing the `nostr-rs-relay` docker service.

use nostr_relay_builder::prelude::*;

/// A running relay. Shuts down when dropped, so tests must hold it for as long
/// as they need it.
pub struct TestRelay {
    _relay: LocalRelay,
    url: String,
}

impl TestRelay {
    /// Starts a relay on an ephemeral port.
    pub async fn start() -> TestRelay {
        let relay = LocalRelay::new(RelayBuilder::default());
        relay.run().await.expect("failed to start the local relay");
        let url = relay.url().await.to_string();
        TestRelay { _relay: relay, url }
    }

    /// The `ws://` URL to connect to.
    pub fn url(&self) -> &str {
        &self.url
    }
}
