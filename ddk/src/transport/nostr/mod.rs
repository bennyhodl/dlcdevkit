mod messages;
mod relay_handler;

pub use relay_handler::NostrDlc;
use tokio::sync::watch;

use crate::{DlcDevKitDlcManager, Oracle, Storage, Transport};
use async_trait::async_trait;
use bitcoin::secp256k1::PublicKey as BitcoinPublicKey;
use dlc_messages::Message;
use nostr_rs::PublicKey;
use std::sync::Arc;

#[async_trait]
impl Transport for NostrDlc {
    fn name(&self) -> String {
        "nostr".to_string()
    }

    fn public_key(&self) -> BitcoinPublicKey {
        BitcoinPublicKey::from_slice(&self.keys.public_key.serialize())
            .expect("Should not fail converting nostr key to bitcoin key.")
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
    fn send_message(&self, counterparty: BitcoinPublicKey, message: Message) {
        let public_key = PublicKey::from_slice(&counterparty.serialize())
            .expect("Should not fail converting nostr key to bitcoin key.");
        let _event = messages::create_dlc_msg_event(public_key, None, message, &self.keys);
    }
    /// Connect to another peer
    async fn connect_outbound(&self, _pubkey: BitcoinPublicKey, _host: &str) {
        todo!("Connect outbound")
    }
}
