use bitcoin::{secp256k1::PublicKey, Network};
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
use log::info;
use std::{sync::Arc, time::SystemTime};

pub struct DlcDevKitLogger;

impl Logger for DlcDevKitLogger {
    fn log(&self, record: Record) {
        info!("LOG: {:?}", record);
    }
}

pub type PeerManager = LdkPeerManager<
    SocketDescriptor,
    Arc<ErroringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<DlcDevKitLogger>,
    Arc<DlcMessageHandler>,
    Arc<KeysManager>,
>;

pub struct LightningTransport {
    peer_manager: Arc<PeerManager>,
    message_handler: Arc<DlcMessageHandler>,
    pub node_id: PublicKey,
}

impl LightningTransport {
    pub fn new(name: &str, network: Network) -> LightningTransport {
        let seed = crate::io::read_or_generate_xprv(&name, network)
            .unwrap()
            .private_key
            .secret_bytes();
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let key_signer = KeysManager::new(&seed, time.as_secs(), time.as_nanos() as u32);
        let node_id = key_signer
            .get_node_id(lightning::sign::Recipient::Node)
            .unwrap();
        let dlc_message_handler = Arc::new(DlcMessageHandler::new());

        let message_handler = MessageHandler {
            chan_handler: Arc::new(ErroringMessageHandler::new()),
            route_handler: Arc::new(IgnoringMessageHandler {}),
            onion_message_handler: Arc::new(IgnoringMessageHandler {}),
            custom_message_handler: dlc_message_handler.clone(),
        };

        LightningTransport {
            peer_manager: Arc::new(PeerManager::new(
                message_handler,
                time.as_secs() as u32,
                &seed,
                Arc::new(DlcDevKitLogger {}),
                Arc::new(key_signer),
            )),
            message_handler: dlc_message_handler,
            node_id,
        }
    }

    pub fn peer_manager(&self) -> Arc<PeerManager> {
        self.peer_manager.clone()
    }

    pub fn message_handler(&self) -> Arc<DlcMessageHandler> {
        self.message_handler.clone()
    }
}
