use ernest_wallet::{
    dlc_manager::contract::offered_contract::OfferedContract,
    peer_manager::{Ernest, ErnestPeerManager, Storage},
};
use std::sync::Arc;
use tauri::State;

use crate::process_incoming_messages;

#[tauri::command]
pub fn list_contracts(ernest: State<Arc<Ernest>>, peer_manager: State<Arc<ErnestPeerManager>>) {
    process_incoming_messages(
        &peer_manager.peer_manager,
        &ernest.manager,
        &peer_manager.message_handler,
    );
    let ernest_clone = ernest.manager.clone();
    tokio::task::spawn_blocking(move || {
        ernest_clone.lock().unwrap().periodic_check(false).unwrap();
        let contracts = ernest_clone
            .lock()
            .unwrap()
            .get_store()
            .get_contracts()
            .unwrap();
        println!("CONTRACTS: {:?}", contracts);
    });
}

#[tauri::command]
pub async fn list_offers(
    ernest: State<'_, Arc<Ernest>>,
    peer_manager: State<'_, Arc<ErnestPeerManager>>,
) -> Result<Vec<OfferedContract>, tauri::Error> {
    process_incoming_messages(
        &peer_manager.peer_manager,
        &ernest.manager,
        &peer_manager.message_handler,
    );
    let ernest_clone = ernest.manager.clone();
    let offers = tokio::task::spawn_blocking(move || {
        ernest_clone.lock().unwrap().periodic_check(false).unwrap();
        let offers = ernest_clone
            .lock()
            .unwrap()
            .get_store()
            .get_contract_offers()
            .unwrap();
        offers
    })
    .await
    .unwrap();

    Ok(offers)
}
