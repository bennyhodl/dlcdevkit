use crate::DdkTransport;

pub(crate) mod peer_manager;
use peer_manager::DlcDevKitPeerManager;

impl DdkTransport for DlcDevKitPeerManager {}
