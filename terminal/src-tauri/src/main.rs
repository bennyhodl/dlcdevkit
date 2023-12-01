// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::sync::Arc;

use ernest_wallet::{ErnestWallet, Network};

// Learn more about Tauri commands at https://tauri.app/v1/guides/features/command
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn new_address(wallet: Arc<ErnestWallet>) -> String {
    wallet.new_external_address().unwrap().address.to_string()
}


fn main() {
    let wallet = ErnestWallet::new("ernest".to_string(), "http://localhost:30000".to_string(), Network::Regtest).unwrap();

    tauri::Builder::default()
        .manage(Arc::new(wallet))
        .invoke_handler(tauri::generate_handler![greet, new_address])
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}
