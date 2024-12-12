use crate::{DlcDevKitDlcManager, Oracle, Storage, Transport};
use async_trait::async_trait;
use bitcoin::secp256k1::PublicKey;
use lightning_net_tokio::{connect_outbound, setup_inbound};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpListener;

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

    /// Creates a TCP listener and accepts incoming connection spawning a tokio thread.
    async fn listen(&self) {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.listening_port))
            .await
            .expect("Coldn't get port.");

        loop {
            let peer_mgr = self.peer_manager.clone();
            let (tcp_stream, socket) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                tracing::info!(connection = socket.to_string(), "Received connection.");
                setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
            });
        }
    }

    /// Sends a message to a peer.
    ///
    /// TODO: Assert that we are connected to the peer before sending.
    fn send_message(&self, counterparty: PublicKey, message: dlc_messages::Message) {
        self.message_handler.send_message(counterparty, message)
    }

    /// Gets and clears the message queue with messages to be processed.
    /// Takes the manager to process the DLC messages that are received.
    async fn receive_messages<S: Storage, O: Oracle>(
        &self,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        loop {
            timer.tick().await;
            let messages = self.message_handler.get_and_clear_received_messages();

            for (counter_party, message) in messages {
                tracing::info!(
                    counter_party = counter_party.to_string(),
                    "Processing DLC message"
                );

                match manager.on_dlc_message(&message, counter_party).await {
                    Err(e) => {
                        tracing::error!(error =? e, "On message error.")
                    }
                    Ok(contract) => {
                        if let Some(msg) = contract {
                            tracing::info!("Responding to message received.");
                            tracing::debug!(message=?msg);
                            self.message_handler.send_message(counter_party, msg);
                        }
                    }
                };
            }

            if self.message_handler.has_pending_messages() {
                self.peer_manager.process_events()
            }
        }
    }

    async fn connect_outbound(&self, pubkey: PublicKey, host: &str) {
        connect_outbound(self.peer_manager.clone(), pubkey, host.parse().unwrap()).await;
    }
}
