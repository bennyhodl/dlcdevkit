use dlc_dev_kit::{bdk::wallet::Balance, p2p::DlcDevKit};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn new_address(bella: State<Arc<DlcDevKit>>) -> String {
    bella
        .wallet
        .new_external_address()
        .unwrap()
        .address
        .to_string()
}

#[tauri::command]
pub fn get_balance(bella: State<Arc<DlcDevKit>>) -> Balance {
    let balance = bella.wallet.get_balance().unwrap();
    log::info!("Balance: {:?}", balance);
    balance
}

// #[tauri::command]
// pub fn send(bella: State<Arc<Bella>>, address: String) {
//     let addr = Address::from_str(&address).unwrap().assume_checked();
//     bella.wallet.send_to_address(addr, 50_000, 1.0).unwrap();
// }
