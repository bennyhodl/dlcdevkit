use std::sync::Arc;

use crate::DdkTransport;
use async_trait::async_trait;
use bitcoin::secp256k1::PublicKey;
use dlc_messages::Message;
use lightning_net_tokio::setup_inbound;

pub(crate) mod peer_manager;
pub use peer_manager::LightningTransport;
use tokio::net::TcpListener;

use super::PeerInformation;

#[async_trait]
impl DdkTransport for LightningTransport {
    type PeerManager = Arc<super::lightning::peer_manager::LnPeerManager>;
    type MessageHandler = Arc<dlc_messages::message_handler::MessageHandler>;

    fn name(&self) -> String {
        "lightning".into()
    }

    async fn listen(&self) {
        let peer_manager_connection_handler = self.peer_manager();

        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.listening_port))
            .await
            .expect("Coldn't get port.");

        loop {
            let peer_mgr = peer_manager_connection_handler.clone();
            let (tcp_stream, socket) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                tracing::info!(connection=socket.to_string(), "Received connection.");
                setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
            });
        }
    }

    fn message_handler(&self) -> Self::MessageHandler {
        self.message_handler()
    }

    fn peer_manager(&self) -> Self::PeerManager {
        self.ln_peer_manager()
    }

    fn process_messages(&self) {
        tracing::info!("Processing lightning messages.");
        self.ln_peer_manager().process_events()
    }

    fn send_message(&self, counterparty: PublicKey, message: dlc_messages::Message) {
        self.message_handler().send_message(counterparty, message)
    }

    fn get_and_clear_received_messages(&self) -> Vec<(PublicKey, Message)> {
        self.message_handler().get_and_clear_received_messages()
    }

    fn has_pending_messages(&self) -> bool {
        self.message_handler().has_pending_messages()
    }

    async fn connect_outbound(&self, _peer: PeerInformation) {}
}
