mod messages;
mod relay_handler;

pub use relay_handler::NostrDlc;

use crate::{DlcDevKitDlcManager, Oracle, Storage, Transport};
use bitcoin::secp256k1::PublicKey as BitcoinPublicKey;
use dlc_messages::Message;
use nostr_rs::PublicKey;
use std::sync::Arc;

#[async_trait::async_trait]
impl Transport for NostrDlc {
    fn name(&self) -> String {
        "nostr".to_string()
    }

    async fn listen(&self) {
        self.listen().await.expect("Did not start nostr listener.");
    }

    /// Get messages that have not been processed yet.
    async fn receive_messages<S: Storage, O: Oracle>(
        &self,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) {
        self.receive_dlc_messages(manager).await
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
