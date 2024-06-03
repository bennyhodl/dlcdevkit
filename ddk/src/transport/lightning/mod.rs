use std::{sync::Arc, time::Duration};

use crate::{ddk::DlcDevKitDlcManager, DdkTransport, DlcDevKit};
use async_trait::async_trait;
use lightning_net_tokio::setup_inbound;

pub(crate) mod peer_manager;
use peer_manager::DlcDevKitPeerManager;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[async_trait]
impl DdkTransport for DlcDevKitPeerManager {
    fn name(&self) -> String {
        "lightning".into()
    }

    async fn listen(&self) {
        let peer_manager_connection_handler = self.peer_manager();

        let listener = TcpListener::bind("0.0.0.0:9002")
            .await
            .expect("Coldn't get port.");

        loop {
            let peer_mgr = peer_manager_connection_handler.clone();
            let (tcp_stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
            });
        }
    }

    async fn receive_dlc_message(&self, dlc_manager: &Arc<Mutex<DlcDevKitDlcManager>>) {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            println!("timer tick");
            let message_handler = self.message_handler();
            let peer_manager = self.peer_manager();
            let messages = message_handler.get_and_clear_received_messages();
            for (node_id, message) in messages {
                let mut man = dlc_manager.lock().await;
                println!("Checking msg lock");
                let resp = man
                    .on_dlc_message(&message, node_id)
                    .expect("Error processing message");

                if let Some(msg) = resp {
                    message_handler.send_message(node_id, msg);
                }

                if message_handler.has_pending_messages() {
                    peer_manager.process_events();
                }
            }
        }
    }
}
