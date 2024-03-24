// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod models;
use std::sync::Arc;

use ernest_wallet::{dlc_handler::DlcHandler, dlc_storage::SledStorageProvider, Ernest, Network};
use models::Pubkeys;
// use nostr_sdk::RelayPoolNotification;
use nostr_relay_pool::RelayPoolNotification;

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
fn get_pubkeys(ernest: tauri::State<Arc<Ernest>>) -> Pubkeys {
    let nostr = ernest.nostr.keys.public_key().to_string();
    let bitcoin = ernest.wallet.get_pubkey().unwrap().to_string();

    Pubkeys { nostr, bitcoin }
}

#[tokio::main]
async fn main() {
    let ernest = Arc::new(
        Ernest::new(
            "terminal",
            "http://localhost:30000",
            Network::Regtest,
        ).await
        .unwrap(),
    );

    let dlc_storage = SledStorageProvider::new("terminal").unwrap();

    // TODO: I think a receiver might be a better arch so it doesn't block incoming messages
    // let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let dlc_handler = Arc::new(DlcHandler::new(dlc_storage));

    let nostr_clone = ernest.nostr.clone();
    let handler_clone = dlc_handler.clone();

    tokio::spawn(async move {
        let client = nostr_clone.listen().await.unwrap();

        while let Ok(msg) = client.notifications().recv().await {
            match msg {
                RelayPoolNotification::Event { relay_url: _, event, subscription_id: _ } => {
                    handler_clone.receive_event(*event);
                }
                _ => println!("other msg.")
            }
        }
    });

    tauri::Builder::default()
        .manage(ernest.clone())
        .invoke_handler(tauri::generate_handler![new_address, get_pubkeys])
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}
