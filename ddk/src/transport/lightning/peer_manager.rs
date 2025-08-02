use bitcoin::{key::rand::Fill, secp256k1::PublicKey};
use dlc_messages::message_handler::MessageHandler as DlcMessageHandler;
use lightning::{
    ln::peer_handler::{
        ErroringMessageHandler, IgnoringMessageHandler, MessageHandler,
        PeerManager as LdkPeerManager,
    },
    sign::{KeysManager, NodeSigner},
    util::logger::{Level, Logger, Record},
};
use lightning_net_tokio::{setup_inbound, SocketDescriptor};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle, time::interval};

use crate::{ddk::DlcDevKitDlcManager, error::TransportError, Oracle, Storage};

pub struct DlcDevKitLogger;

/// TODO: make a logging struct for the crate.
impl Logger for DlcDevKitLogger {
    fn log(&self, record: Record) {
        match record.level {
            Level::Info => tracing::info!("{}", record.args),
            Level::Warn => tracing::warn!("{}", record.args),
            Level::Debug => tracing::debug!("{}", record.args),
            Level::Error => tracing::error!("{}", record.args),
            _ => tracing::trace!("{}", record.args),
        }
    }
}

/// Peer manager that only recognizes DLC messages.
pub type LnPeerManager = LdkPeerManager<
    SocketDescriptor,
    Arc<ErroringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<DlcDevKitLogger>,
    Arc<DlcMessageHandler>,
    Arc<KeysManager>,
>;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct PeerInformation {
    pub pubkey: String,
    pub host: String,
}

/// BOLT-8 LightningTransport to manage TCP connections to communicate
/// DLC contracts with another party.
pub struct LightningTransport {
    /// Manages the connections to other DLC peers.
    pub peer_manager: Arc<LnPeerManager>,
    /// Handles the message queue.
    pub message_handler: Arc<DlcMessageHandler>,
    /// Our nodes id.
    pub node_id: PublicKey,
    /// Listening port for the TCP connection.
    pub listening_port: u16,
}

impl LightningTransport {
    pub fn new(
        seed_bytes: &[u8; 32],
        listening_port: u16,
    ) -> Result<LightningTransport, TransportError> {
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| TransportError::Init(e.to_string()))?;
        let key_signer = KeysManager::new(seed_bytes, time.as_secs(), time.as_nanos() as u32);
        let node_id = key_signer
            .get_node_id(lightning::sign::Recipient::Node)
            .map_err(|_| TransportError::Init("Could not get node id.".to_string()))?;

        let dlc_message_handler = Arc::new(DlcMessageHandler::new());
        let message_handler = MessageHandler {
            chan_handler: Arc::new(ErroringMessageHandler::new()),
            route_handler: Arc::new(IgnoringMessageHandler {}),
            onion_message_handler: Arc::new(IgnoringMessageHandler {}),
            custom_message_handler: dlc_message_handler.clone(),
        };

        let mut ephmeral_data = [0u8; 32];
        ephmeral_data
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .map_err(|e| TransportError::Init(e.to_string()))?;

        Ok(LightningTransport {
            peer_manager: Arc::new(LnPeerManager::new(
                message_handler,
                time.as_secs() as u32,
                &ephmeral_data,
                Arc::new(DlcDevKitLogger {}),
                Arc::new(key_signer),
            )),
            message_handler: dlc_message_handler,
            node_id,
            listening_port,
        })
    }

    pub fn listen(
        &self,
        stop_signal: watch::Receiver<bool>,
    ) -> JoinHandle<Result<(), TransportError>> {
        let listening_port = self.listening_port;
        let mut listen_stop = stop_signal.clone();
        let peer_manager = Arc::clone(&self.peer_manager);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listening_port))
                .await
                .map_err(|e| TransportError::Listen(e.to_string()))?;

            tracing::info!(
                addr =? listener.local_addr().unwrap(),
                "Starting lightning peer manager listener."
            );
            loop {
                tokio::select! {
                    _ = listen_stop.changed() => {
                        if *listen_stop.borrow() {
                            tracing::warn!("Stop signal for lightning connection manager.");
                            break;
                        }
                    },
                    accept_result = listener.accept() => {
                        println!("acceptting connection");
                        match accept_result {
                            Ok((tcp_stream, socket)) => {
                                let peer_mgr = Arc::clone(&peer_manager);
                                tokio::spawn(async move {
                                    tracing::info!(
                                        connection = socket.to_string(),
                                        "Received connection."
                                    );
                                    setup_inbound(peer_mgr, tcp_stream.into_std().unwrap()).await;
                                });
                            }
                            Err(e) => {
                                println!("error accepting connection: {}", e);
                                tracing::error!("Error accepting connection: {}", e);
                            }
                        }
                    }
                }
            }
            Ok::<_, TransportError>(())
        })
    }

    pub fn process_messages<S: Storage, O: Oracle>(
        &self,
        stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> JoinHandle<Result<(), TransportError>> {
        let mut message_stop = stop_signal.clone();
        let message_manager = Arc::clone(&manager);
        let peer_manager = Arc::clone(&self.peer_manager);
        let message_handler = Arc::clone(&self.message_handler);
        tokio::spawn(async move {
            let mut message_interval = interval(Duration::from_secs(20));
            loop {
                tokio::select! {
                    _ = message_stop.changed() => {
                        if *message_stop.borrow() {
                            tracing::warn!("Stop signal for lightning message processor.");
                            break;
                        }
                    },
                    _ = message_interval.tick() => {
                        if message_handler.has_pending_messages() {
                            tracing::info!("There are pending messages to be sent.");
                            peer_manager.process_events();
                        }
                        let messages = message_handler.get_and_clear_received_messages();
                        for (counter_party, message) in messages {
                            tracing::info!(
                                counter_party = counter_party.to_string(),
                                "Processing DLC message"
                            );
                            match message_manager.on_dlc_message(&message, counter_party).await {
                                Ok(Some(message)) => {
                                    if peer_manager.peer_by_node_id(&counter_party).is_some() {
                                        tracing::info!(message=?message, "Sending message to {}", counter_party.to_string());
                                        message_handler.send_message(counter_party, message);
                                        peer_manager.process_events();
                                    } else {
                                        tracing::warn!(
                                            pubkey = counter_party.to_string(),
                                            "Not connected to counterparty. Message not sent"
                                        )
                                    }
                                }
                                Ok(None) => (),
                                Err(e) => {
                                    tracing::error!(
                                        error=e.to_string(),
                                        counterparty=counter_party.to_string(),
                                        message=?message,
                                        "Could not process dlc message."
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Ok::<_, TransportError>(())
        })
    }

    pub fn list_peers(&self) -> Vec<PeerInformation> {
        self.peer_manager
            .list_peers()
            .into_iter()
            .map(|p| PeerInformation {
                pubkey: p.counterparty_node_id.to_string(),
                host: p.socket_address.unwrap().to_string(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::Transport;
    use dlc_messages::{Message, OfferDlc};

    use super::*;

    fn get_offer() -> OfferDlc {
        let offer_string = include_str!("../../../../ddk-manager/test_inputs/offer_contract.json");
        let offer: OfferDlc =
            serde_json::from_str(&offer_string).expect("to be able to parse offer");
        offer
    }

    #[tokio::test]
    async fn send_offer_test() {
        let mut seed_bytes = [0u8; 32];
        seed_bytes
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let mut bob_seed_bytes = [0u8; 32];
        bob_seed_bytes
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let alice = LightningTransport::new(&seed_bytes, 1776).unwrap();
        let bob = LightningTransport::new(&bob_seed_bytes, 1777).unwrap();

        let (sender, receiver) = watch::channel(false);
        alice.listen(receiver.clone());
        bob.listen(receiver.clone());

        tokio::time::sleep(Duration::from_secs(2)).await;

        bob.connect_outbound(alice.public_key(), "127.0.0.1:1776")
            .await;

        let mut connected = false;
        let mut retries = 0;

        while !connected {
            if retries > 10 {
                sender.send(true).unwrap();
                panic!("Bob could not connect to alice.")
            }
            if alice
                .peer_manager
                .peer_by_node_id(&bob.public_key())
                .is_some()
            {
                connected = true
            }
            retries += 1;
            tokio::time::sleep(Duration::from_millis(100)).await
        }

        let offer = get_offer();
        bob.send_message(alice.public_key(), Message::Offer(offer.clone()))
            .await;

        let mut received = false;
        let mut retries = 0;

        while !received {
            if retries > 10 {
                sender.send(true).unwrap();
                panic!("Alice did not receive the offer.")
            }
            if alice
                .message_handler
                .get_and_clear_received_messages()
                .len()
                > 0
            {
                received = true
            }
            retries += 1;
            tokio::time::sleep(Duration::from_millis(100)).await
        }

        sender.send(true).unwrap();
    }
}
