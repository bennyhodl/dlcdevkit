use anyhow::anyhow;
use bitcoin::{key::rand::Fill, secp256k1::PublicKey};
use dlc_messages::message_handler::MessageHandler as DlcMessageHandler;
use lightning::{
    ln::peer_handler::{
        ErroringMessageHandler, IgnoringMessageHandler, MessageHandler,
        PeerManager as LdkPeerManager,
    },
    sign::{KeysManager, NodeSigner},
    util::logger::{Logger, Record},
};
use lightning_net_tokio::{setup_inbound, SocketDescriptor};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{net::TcpListener, sync::watch, task::JoinHandle, time::interval};

use crate::{ddk::DlcDevKitDlcManager, Oracle, Storage};

pub struct DlcDevKitLogger;

/// TODO: make a logging struct for the crate.
impl Logger for DlcDevKitLogger {
    fn log(&self, record: Record) {
        tracing::info!("{}", record.args);
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
    pub fn new(seed_bytes: &[u8; 32], listening_port: u16) -> anyhow::Result<LightningTransport> {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        let key_signer = KeysManager::new(seed_bytes, time.as_secs(), time.as_nanos() as u32);
        let node_id = key_signer
            .get_node_id(lightning::sign::Recipient::Node)
            .map_err(|_| anyhow!("Could not get node id."))?;

        let dlc_message_handler = Arc::new(DlcMessageHandler::new());
        let message_handler = MessageHandler {
            chan_handler: Arc::new(ErroringMessageHandler::new()),
            route_handler: Arc::new(IgnoringMessageHandler {}),
            onion_message_handler: Arc::new(IgnoringMessageHandler {}),
            custom_message_handler: dlc_message_handler.clone(),
        };

        let mut ephmeral_data = [0u8; 32];
        ephmeral_data.try_fill(&mut bitcoin::key::rand::thread_rng())?;

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
    ) -> JoinHandle<Result<(), anyhow::Error>> {
        let listening_port = self.listening_port;
        let mut listen_stop = stop_signal.clone();
        let peer_manager = Arc::clone(&self.peer_manager);
        tokio::spawn(async move {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", listening_port))
                .await
                .expect("Coldn't get port.");

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
                                tracing::error!("Error accepting connection: {}", e);
                            }
                        }
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        })
    }

    pub fn process_messages<S: Storage, O: Oracle>(
        &self,
        stop_signal: watch::Receiver<bool>,
        manager: Arc<DlcDevKitDlcManager<S, O>>,
    ) -> JoinHandle<Result<(), anyhow::Error>> {
        let mut message_stop = stop_signal.clone();
        let message_manager = Arc::clone(&manager);
        let peer_manager = Arc::clone(&self.peer_manager);
        let message_handler = Arc::clone(&self.message_handler);
        tokio::spawn(async move {
            let mut message_interval = interval(Duration::from_secs(5));
            // let mut event_interval = interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = message_stop.changed() => {
                        if *message_stop.borrow() {
                            tracing::warn!("Stop signal for lightning message processor.");
                            break;
                        }
                    },
                    _ = message_interval.tick() => {
                        peer_manager.process_events();
                        let messages = message_handler.get_and_clear_received_messages();
                        for (counter_party, message) in messages {
                            tracing::info!(
                                counter_party = counter_party.to_string(),
                                "Processing DLC message"
                            );
                            match message_manager.on_dlc_message(&message, counter_party).await {
                                Ok(Some(response)) => {
                                    message_handler.send_message(counter_party, response);
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
            Ok::<_, anyhow::Error>(())
        })
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::Network;
    use ddk_manager::Storage;
    use dlc_messages::{Message, OfferDlc};

    use crate::{
        builder::Builder, oracle::memory::MemoryOracle, storage::memory::MemoryStorage, DlcDevKit,
        Transport,
    };

    use super::*;

    fn get_offer() -> OfferDlc {
        let offer_string = include_str!("../../../../ddk-manager/test_inputs/offer_contract.json");
        let offer: OfferDlc = serde_json::from_str(&offer_string).unwrap();
        offer
    }

    fn create_peer_manager(listening_port: u16) -> (LightningTransport, PublicKey) {
        let mut seed = [0u8; 32];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let peer_manager = LightningTransport::new(&seed, listening_port).unwrap();
        let pubkey = peer_manager.node_id.clone();
        (peer_manager, pubkey)
    }

    async fn manager(
        listening_port: u16,
    ) -> DlcDevKit<LightningTransport, MemoryStorage, MemoryOracle> {
        let mut seed_bytes = [0u8; 32];
        seed_bytes
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();

        let transport = Arc::new(LightningTransport::new(&seed_bytes, listening_port).unwrap());
        let storage = Arc::new(MemoryStorage::new());
        let oracle_client = Arc::new(MemoryOracle::default());

        let mut builder = Builder::new();
        builder.set_network(Network::Regtest);
        builder.set_esplora_host("http://127.0.0.1:30000".to_string());
        builder.set_seed_bytes(seed_bytes);
        builder.set_transport(transport.clone());
        builder.set_storage(storage.clone());
        builder.set_oracle(oracle_client.clone());
        builder.finish().await.unwrap()
    }

    #[test_log::test(tokio::test)]
    async fn send_offer() {
        let alice = manager(1776).await;
        let alice_pk = alice.transport.public_key();
        let bob = manager(1777).await;
        let _bob_pk = bob.transport.public_key();

        bob.start().unwrap();
        alice.start().unwrap();

        bob.transport
            .connect_outbound(alice_pk, "127.0.0.1:1776")
            .await;

        let mut connected = false;
        let mut retries = 0;

        while !connected {
            if retries > 10 {
                bob.stop().unwrap();
                alice.stop().unwrap();
                panic!("Bob could not connect to alice.")
            }
            if bob
                .transport
                .peer_manager
                .peer_by_node_id(&alice_pk)
                .is_some()
            {
                connected = true
            }
            retries += 1;
            tokio::time::sleep(Duration::from_millis(100)).await
        }

        let offer = get_offer();
        bob.transport
            .send_message(alice_pk, Message::Offer(offer.clone()));

        let mut offer_received = false;
        let mut retries = 0;

        while !offer_received {
            if retries > 15 {
                bob.stop().unwrap();
                alice.stop().unwrap();
                panic!("Contract was not offered to alice")
            }
            if alice
                .storage
                .get_contract_offers()
                .unwrap()
                .iter()
                .find(|o| o.id == offer.temporary_contract_id)
                .is_some()
            {
                offer_received = true
            }
            retries += 1;
            tokio::time::sleep(Duration::from_secs(1)).await
        }

        bob.stop().unwrap();
        alice.stop().unwrap();
        assert!(true)
    }
}
