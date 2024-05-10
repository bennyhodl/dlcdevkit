use ernest_wallet::{bdk::wallet::Balance, p2p::Ernest};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn new_address(ernest: State<Arc<Ernest>>) -> String {
    ernest
        .wallet
        .new_external_address()
        .unwrap()
        .address
        .to_string()
}

#[tauri::command]
pub fn get_balance(ernest: State<Arc<Ernest>>) -> Balance {
    let balance = ernest.wallet.get_balance().unwrap();
    log::info!("Balance: {:?}", balance);
    balance
}

// #[tauri::command]
// pub fn send(ernest: State<Arc<Ernest>>, address: String) {
//     let addr = Address::from_str(&address).unwrap().assume_checked();
//     ernest.wallet.send_to_address(addr, 50_000, 1.0).unwrap();
// }
