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
use std::{sync::Arc, time::SystemTime};

pub struct ErnestLogger;

impl Logger for ErnestLogger {
    fn log(&self, record: Record) {
        println!("LOG: {:?}", record);
    }
}

pub type PeerManager = LdkPeerManager<
    SocketDescriptor,
    Arc<ErroringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<IgnoringMessageHandler>,
    Arc<ErnestLogger>,
    Arc<DlcMessageHandler>,
    Arc<KeysManager>,
>;

pub struct ErnestPeerManager {
    pub peer_manager: Arc<PeerManager>,
    pub node_id: PublicKey,
}

impl ErnestPeerManager {
    pub fn new(name: &str, network: Network) -> ErnestPeerManager {
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
        let message_handler = MessageHandler {
            chan_handler: Arc::new(ErroringMessageHandler::new()),
            route_handler: Arc::new(IgnoringMessageHandler {}),
            onion_message_handler: Arc::new(IgnoringMessageHandler {}),
            custom_message_handler: Arc::new(DlcMessageHandler::new()),
        };

        ErnestPeerManager {
            peer_manager: Arc::new(PeerManager::new(
                message_handler,
                time.as_secs() as u32,
                &seed,
                Arc::new(ErnestLogger {}),
                Arc::new(key_signer),
            )),
            node_id,
        }
    }
}
