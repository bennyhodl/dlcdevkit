use crate::{DlcDevKitDlcManager, Oracle, Storage, Transport};
use async_trait::async_trait;
use bitcoin::secp256k1::PublicKey;
use lightning_net_tokio::connect_outbound;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) mod peer_manager;
pub use peer_manager::LightningTransport;

#[async_trait]
impl Transport for LightningTransport {
    fn name(&self) -> String {
        "lightning".into()
    }

    fn public_key(&self) -> PublicKey {
        self.node_id
    }

    /// Sends a message to a peer.
    async fn send_message(&self, counterparty: PublicKey, message: dlc_messages::Message) {
        tracing::info!(message=?message, "Sending message to {}", counterparty.to_string());
        if self.peer_manager.peer_by_node_id(&counterparty).is_some() {
            self.message_handler.send_message(counterparty, message);
            self.peer_manager.process_events();
        } else {
            tracing::warn!(
                pubkey = counterparty.to_string(),
                "Not connected to counterparty. Message not sent"
            )
        }
    }

    /// Gets and clears the message queue with messages to be processed.
    /// Takes the manager to process the DLC messages that are received.
    async fn start<S: Storage, O: Oracle>(
        &self,
        mut stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> Result<(), anyhow::Error> {
        let listen_handle = self.listen(stop_signal.clone());

        let process_handle = self.process_messages(stop_signal.clone(), manager.clone());

        // Wait for either task to complete or stop signal
        tokio::select! {
            _ = stop_signal.changed() => Ok(()),
            res = listen_handle => res?,
            res = process_handle => res?,
        }
    }

    async fn connect_outbound(&self, pubkey: PublicKey, host: &str) {
        connect_outbound(self.peer_manager.clone(), pubkey, host.parse().unwrap()).await;
    }
}
