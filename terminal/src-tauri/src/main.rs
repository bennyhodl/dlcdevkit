// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod dlc;
mod models;
mod nostr;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use ernest_wallet::{
    p2p::{
        lightning_net_tokio::setup_inbound, Ernest, ErnestPeerManager, ErnestDlcManager
    },
    Network,
};
use models::Pubkeys;
use tauri::State;
use tokio::net::TcpListener;

#[tauri::command]
fn new_address(ernest: tauri::State<Arc<Ernest>>) -> String {
    ernest
        .wallet
        .new_external_address()
        .unwrap()
        .address
        .to_string()
}

#[tauri::command]
fn get_pubkeys(ernest: State<Arc<Ernest>>, p2p: State<Arc<ErnestPeerManager>>) -> Pubkeys {
    let bitcoin = ernest.wallet.get_pubkey().unwrap().to_string();
    let node_id = p2p.node_id.to_string();

    Pubkeys { bitcoin, node_id }
}

#[tauri::command]
fn list_peers(p2p: State<Arc<ErnestPeerManager>>) -> Vec<String> {
    let mut node_ids = Vec::new();
    for (node_id, _) in p2p.peer_manager().get_peer_node_ids() {
        node_ids.push(node_id.to_string())
    }
    node_ids
}

#[tokio::main]
async fn main() {
    let name = "terminal".to_string();
    let ernest = Arc::new(
        Ernest::new(&name, "http://localhost:30000", Network::Regtest)
            .await
            .unwrap(),
    );

    let p2p = Arc::new(ErnestPeerManager::new(&name, Network::Regtest));

    let peer_manager_connection_handler = p2p.peer_manager();
    tokio::spawn(async move {
        let listener = TcpListener::bind("0.0.0.0:9000")
            .await
            .expect("Coldn't get port.");
        loop {
            let peer_mgr = peer_manager_connection_handler.clone();
            let (tcp_stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
            });
        }
    });

    let dlc_manager_clone = ernest.manager.clone();
    let p2p_clone = p2p.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        loop {
            ticker.tick().await;
            process_incoming_messages(
                &p2p_clone,
                &dlc_manager_clone,
            );
        }
    });

    tauri::Builder::default()
        .manage(ernest.clone())
        .manage(p2p.clone())
        .invoke_handler(tauri::generate_handler![
            new_address,
            get_pubkeys,
            list_peers,
            crate::dlc::list_contracts,
            crate::dlc::list_offers
        ])
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}

pub fn process_incoming_messages(
    p2p: &Arc<ErnestPeerManager>,
    dlc_manager: &Arc<Mutex<ErnestDlcManager>>,
) {
    let message_handler = p2p.message_handler();
    let peer_manager = p2p.peer_manager();
    let messages = message_handler.get_and_clear_received_messages();

    for (node_id, message) in messages {
        let resp = dlc_manager
            .lock()
            .unwrap()
            .on_dlc_message(&message, node_id)
            .expect("Error processing message");
        if let Some(msg) = resp {
            message_handler.send_message(node_id, msg);
        }
    }

    if message_handler.has_pending_messages() {
        peer_manager.process_events();
    }
}
