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
use lightning_net_tokio::SocketDescriptor;
use std::{sync::Arc, time::SystemTime};

pub struct DlcDevKitLogger;

impl Logger for DlcDevKitLogger {
    fn log(&self, record: Record) {
        tracing::info!("{}", record.args);
    }
}

pub type LnPeerManager = LdkPeerManager<
    SocketDescriptor,
    Arc<ErroringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<DlcDevKitLogger>,
    Arc<DlcMessageHandler>,
    Arc<KeysManager>,
>;

pub struct LightningTransport {
    pub peer_manager: Arc<LnPeerManager>,
    pub message_handler: Arc<DlcMessageHandler>,
    pub node_id: PublicKey,
    pub listening_port: u16,
}

impl LightningTransport {
    pub fn new(seed_bytes: &[u8; 32], listening_port: u16) -> anyhow::Result<LightningTransport> {
        let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
        let key_signer = KeysManager::new(&seed_bytes, time.as_secs(), time.as_nanos() as u32);
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
}
