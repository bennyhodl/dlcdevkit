use std::sync::Arc;

use tauri::State;

use crate::models::Pubkeys;

pub mod dlc;
pub mod wallet;

#[tauri::command]
pub fn get_pubkeys(bella: State<Arc<crate::BellaDdk>>) -> Pubkeys {
    let bitcoin = bella.wallet.get_pubkey().unwrap().to_string();
    // let node_id = p2p.node_id.to_string();

    Pubkeys { bitcoin }
}

// #[tauri::command]
// fn list_peers(p2p: State<Arc<DlcDevKitPeerManager>>) -> Vec<String> {
//     let mut node_ids = Vec::new();
//     for (node_id, _) in p2p.peer_manager().get_peer_node_ids() {
//         node_ids.push(node_id.to_string())
//     }
//     info!("{:?}", node_ids);
//     node_ids
// }
