mod relay_handler;

pub use relay_handler::NostrDlc;
use tokio::sync::watch;

use crate::nostr;
use crate::{DlcDevKitDlcManager, Oracle, Storage, Transport};
use async_trait::async_trait;
use dlc::secp256k1_zkp::PublicKey as BitcoinPublicKey;
use dlc_messages::Message;
use std::sync::Arc;

#[async_trait]
impl Transport for NostrDlc {
    fn name(&self) -> String {
        "nostr".to_string()
    }

    fn public_key(&self) -> BitcoinPublicKey {
        nostr::nostr_to_bitcoin_pubkey(&self.keys.public_key())
    }

    /// Get messages that have not been processed yet.
    async fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> Result<(), anyhow::Error> {
        let listen_handle = self.start(stop_signal.clone(), manager);

        // Wait for either task to complete or stop signal
        tokio::select! {
            _ = stop_signal.changed() => Ok(()),
            res = listen_handle => res?,
        }
    }
    /// Send a message to a specific counterparty.
    async fn send_message(&self, counterparty: BitcoinPublicKey, message: Message) {
        let nostr_counterparty = nostr::bitcoin_to_nostr_pubkey(&counterparty);
        tracing::info!(
            bitcoin_pk = counterparty.to_string(),
            nostr_pk = nostr_counterparty.to_string(),
            "Sending nostr message."
        );
        let event =
            nostr::messages::create_dlc_msg_event(nostr_counterparty, None, message, &self.keys)
                .unwrap();
        match self.client.send_event(&event).await {
            Err(e) => tracing::error!(error = e.to_string(), "Failed to send nostr event."),
            Ok(e) => tracing::info!(event_id = e.val.to_string(), "Sent DLC message event."),
        }
    }
    /// Connect to a relay.
    async fn connect_outbound(&self, _pubkey: BitcoinPublicKey, host: &str) {
        match self.client.add_relay(host).await {
            Ok(_) => tracing::info!(host, "Added relay."),
            Err(e) => tracing::error!(host, error = e.to_string(), "Could not add relay."),
        }
    }
}
