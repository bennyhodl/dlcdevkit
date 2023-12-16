// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod models;
use std::sync::Arc;

use ernest_wallet::{handle_relay_event, Ernest, Network};
use models::Pubkeys;

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

    let nostr_clone = ernest.nostr.clone();

    tokio::spawn(async move {
        let client = nostr_clone.listen().await.unwrap();

        let _handler = client
            .handle_notifications(|event| async move {
                handle_relay_event(event);
                Ok(false)
            })
            .await;
    });

    tauri::Builder::default()
        .manage(ernest.clone())
        .invoke_handler(tauri::generate_handler![new_address, get_pubkeys])
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}
