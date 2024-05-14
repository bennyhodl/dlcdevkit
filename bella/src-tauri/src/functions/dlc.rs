use dlc_dev_kit::{
    dlc_manager::{contract::offered_contract::OfferedContract, ContractId},
    p2p::{DlcDevKit, DlcDevKitPeerManager, Storage},
};
// use futures::TryFutureExt;
use crate::process_incoming_messages;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_contracts(bella: State<Arc<DlcDevKit>>, p2p: State<Arc<DlcDevKitPeerManager>>) {
    process_incoming_messages(&p2p, &bella.manager);
    let bella_clone = bella.manager.clone();
    bella_clone.lock().unwrap().periodic_check(false).unwrap();
    let contracts = bella_clone
        .lock()
        .unwrap()
        .get_store()
        .get_contracts()
        .unwrap();
    println!("{:?}", contracts);
    ()
}

#[tauri::command]
pub fn list_offers(
    bella: State<Arc<DlcDevKit>>,
    p2p: State<Arc<DlcDevKitPeerManager>>,
) -> Result<Vec<OfferedContract>, tauri::Error> {
    process_incoming_messages(&p2p, &bella.manager);
    let bella_clone = bella.manager.clone();
    bella_clone.lock().unwrap().periodic_check(false).unwrap();
    let offers = bella_clone
        .lock()
        .unwrap()
        .get_store()
        .get_contract_offers()
        .unwrap();

    Ok(offers)
}

#[tauri::command]
pub fn accept_dlc(contract_id: ContractId, bella: State<Arc<DlcDevKit>>) -> Result<(), String> {
    println!("USING CONTRACT ID {:?}", contract_id);
    Ok(bella.accept_dlc_offer(contract_id).map_err(|e| {
        println!("ERRE: {:?}", e);
        e.to_string()
    })?)
}
// ASYNC example
// #[tauri::command]
// pub async fn list_offers(
//     bella: State<'_, Arc<Bella>>,
//     peer_manager: State<'_, Arc<BellaPeerManager>>,
// ) -> Result<Vec<OfferedContract>, tauri::Error> {
//     process_incoming_messages(
//         &peer_manager.peer_manager,
//         &bella.manager,
//         &peer_manager.message_handler,
//     );
//     let bella_clone = bella.manager.clone();
//     let offers = tokio::task::spawn_blocking(move || {
//         bella_clone.lock().unwrap().periodic_check(false).unwrap();
//         let offers = bella_clone
//             .lock()
//             .unwrap()
//             .get_store()
//             .get_contract_offers()
//             .unwrap();
//         offers
//     })
//     .await
//     .unwrap();
//
//     Ok(offers)
// }
