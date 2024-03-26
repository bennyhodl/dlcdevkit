// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod models;
mod nostr;
use std::sync::Arc;

use tauri::State;
use ernest_wallet::{Ernest, ErnestPeerManager, Network};
use models::Pubkeys;
// use nostr::run_ernest_nostr;
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
fn get_pubkeys(ernest: State<Arc<Ernest>>, peer_manager: State<Arc<ErnestPeerManager>>) -> Pubkeys {
    // let nostr = ernest.relays.keys.public_key().to_string();
    let bitcoin = ernest.wallet.get_pubkey().unwrap().to_string();
    let node_id = peer_manager.node_id.to_string();

    Pubkeys { bitcoin, node_id }
}

#[tokio::main]
async fn main() {
    let ernest =
        Arc::new(Ernest::new("terminal", "http://localhost:30000", Network::Regtest).await.unwrap());

    let peer_manager = Arc::new(ErnestPeerManager::new(&[0u8; 32]));

    let peer_manager_connection_handler = peer_manager.peer_manager.clone();
    tokio::spawn(async move {
        let listener = TcpListener::bind("0.0.0.0:9000").await.expect("Coldn't get port.");
        loop {
            let peer_mgr = peer_manager_connection_handler.clone();
            let (tcp_stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                lightning_net_tokio::setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
                
            });
        }
    });

    tauri::Builder::default()
        .manage(ernest.clone()) 
        .manage(peer_manager.clone())
        .invoke_handler(tauri::generate_handler![new_address, get_pubkeys])
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}
