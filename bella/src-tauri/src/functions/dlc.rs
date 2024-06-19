use ddk::dlc_manager::{contract::offered_contract::OfferedContract, ContractId};
use crate::BellaDdk;
use std::sync::Arc;
use tauri::State;
use ddk::dlc_manager::Storage;

#[tauri::command]
pub fn list_contracts(bella: State<Arc<BellaDdk>>) {
    let contracts = bella.storage.get_contracts().unwrap();
    println!("{:?}", contracts);
    ()
}

#[tauri::command]
pub fn list_offers(
    bella: State<Arc<BellaDdk>>,
) -> Result<Vec<OfferedContract>, tauri::Error> {
    let offers = bella.storage.get_contract_offers().unwrap();

    Ok(offers)
}

#[tauri::command]
pub fn accept_dlc(contract_id: ContractId, bella: State<Arc<BellaDdk>>) -> Result<(), String> {
    println!("USING CONTRACT ID {:?}", contract_id);
    Ok(bella.accept_dlc_offer(contract_id).map_err(|e| {
        println!("ERRE: {:?}", e);
        e.to_string()
    })?)
}

// ASYNC example
#[tauri::command]
pub async fn list_offers_async(
    bella: State<'_, Arc<BellaDdk>>,
) -> Result<Vec<OfferedContract>, tauri::Error> {
    let bella_clone = bella.manager.clone();
    let offers = tokio::task::spawn_blocking(move || {
        bella_clone.lock().unwrap().periodic_check(false).unwrap();
        let offers = bella_clone
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
