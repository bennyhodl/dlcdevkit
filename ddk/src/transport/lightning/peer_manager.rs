use bitcoin::{key::rand::Fill, secp256k1::PublicKey};
use ddk_messages::message_handler::MessageHandler as DlcMessageHandler;
use lightning::{
    ln::peer_handler::{
        ErroringMessageHandler, IgnoringMessageHandler, MessageHandler,
        PeerManager as LdkPeerManager,
    },
    log_error, log_info, log_warn,
    sign::{KeysManager, NodeSigner},
    util::logger::Logger as LightningLogger,
};
use lightning_net_tokio::{setup_inbound, SocketDescriptor};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle, time::interval};

use crate::{ddk::DlcDevKitDlcManager, error::TransportError, logger::Logger, Oracle, Storage};

/// Peer manager that only recognizes DLC messages.
pub type LnPeerManager = LdkPeerManager<
    SocketDescriptor,
    Arc<ErroringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<Logger>,
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
    /// [`crate::logger::Logger`] instance.
    pub logger: Arc<Logger>,
}

impl LightningTransport {
    pub fn new(
        seed_bytes: &[u8; 32],
        listening_port: u16,
        logger: Arc<Logger>,
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
                logger.clone(),
                Arc::new(key_signer),
            )),
            message_handler: dlc_message_handler,
            node_id,
            listening_port,
            logger,
        })
    }

    pub fn listen(
        &self,
        stop_signal: watch::Receiver<bool>,
    ) -> JoinHandle<Result<(), TransportError>> {
        let listening_port = self.listening_port;
        let mut listen_stop = stop_signal.clone();
        let peer_manager = Arc::clone(&self.peer_manager);
        let logger = Arc::clone(&self.logger);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listening_port))
                .await
                .map_err(|e| TransportError::Listen(e.to_string()))?;

            log_info!(
                logger,
                "Starting lightning peer manager listener. address={}",
                listener.local_addr().unwrap()
            );
            let logger_clone = logger.clone();
            loop {
                tokio::select! {
                    _ = listen_stop.changed() => {
                        if *listen_stop.borrow() {
                            log_warn!(logger_clone, "Stop signal for lightning connection manager.");
                            break;
                        }
                    },
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((tcp_stream, socket)) => {
                                let peer_mgr = Arc::clone(&peer_manager);
                                let logger_clone = logger_clone.clone();
                                tokio::spawn(async move {
                                    log_info!(logger_clone, "Received connection. connection={}", socket.to_string());
                                    setup_inbound(peer_mgr, tcp_stream.into_std().unwrap()).await;
                                });
                            }
                            Err(e) => {
                                log_error!(logger_clone, "Error accepting connection. error={}", e);
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
        let logger = Arc::clone(&self.logger);
        tokio::spawn(async move {
            let mut message_interval = interval(Duration::from_secs(20));
            let logger_clone = logger.clone();
            loop {
                tokio::select! {
                    _ = message_stop.changed() => {
                        if *message_stop.borrow() {
                            log_warn!(logger_clone, "Stop signal for lightning message processor.");
                            break;
                        }
                    },
                    _ = message_interval.tick() => {
                        if message_handler.has_pending_messages() {
                            log_info!(logger_clone, "There are pending messages to be sent.");
                            peer_manager.process_events();
                        }
                        let messages = message_handler.get_and_clear_received_messages();
                        for (counter_party, message) in messages {
                            log_info!(logger_clone, "Processing DLC message. counter_party={}", counter_party.to_string());
                            match message_manager.on_dlc_message(&message, counter_party).await {
                                Ok(Some(message)) => {
                                    if peer_manager.peer_by_node_id(&counter_party).is_some() {
                                        log_info!(logger_clone, "Sending message to counter_party={}", counter_party.to_string());
                                        message_handler.send_message(counter_party, message);
                                        peer_manager.process_events();
                                    } else {
                                        log_warn!(logger_clone,
                                            "Not connected to counterparty. Message not sent. counter_party={}", counter_party.to_string()
                                        )
                                    }
                                }
                                Ok(None) => (),
                                Err(e) => {
                                    log_error!(logger_clone,
                                        "Could not process dlc message. message={:?} counterparty={} error={}", message, counter_party.to_string(), e.to_string()
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
    use ddk_messages::{Message, OfferDlc};

    use super::*;

    fn get_offer() -> OfferDlc {
        let offer_string = include_str!("../../../../ddk-manager/test_inputs/offer_contract.json");
        let offer: OfferDlc =
            serde_json::from_str(&offer_string).expect("to be able to parse offer");
        offer
    }

    #[tokio::test]
    async fn send_offer_test() {
        let logger = Arc::new(Logger::disabled("test_lightning_transport".to_string()));
        let mut seed_bytes = [0u8; 32];
        seed_bytes
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let mut bob_seed_bytes = [0u8; 32];
        bob_seed_bytes
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let alice = LightningTransport::new(&seed_bytes, 1776, logger.clone()).unwrap();
        let bob = LightningTransport::new(&bob_seed_bytes, 1777, logger.clone()).unwrap();

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
