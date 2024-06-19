use std::sync::Arc;

use log::info;
use tauri::State;

use crate::{models::Pubkeys, BellaDdk};

pub mod dlc;
pub mod wallet;

#[tauri::command]
pub fn get_pubkeys(bella: State<Arc<crate::BellaDdk>>) -> Pubkeys {
    let bitcoin = bella.wallet.get_pubkey().unwrap().to_string();
    let node_id = bella.transport.node_id.to_string();

    Pubkeys { bitcoin, node_id }
}

#[tauri::command]
pub fn list_peers(_bella: State<Arc<BellaDdk>>) -> Vec<String> {
    let node_ids = Vec::new();
    // for (node_id, _) in bella.transport.peer_manager().get_peer_node_ids() {
    //     node_ids.push(node_id.to_string())
    // }
    info!("Peers: {:?}", node_ids);
    node_ids
}
