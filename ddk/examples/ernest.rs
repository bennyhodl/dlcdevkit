// #[tokio::main]
// async fn main() {
//     DlcDevKitNostr::new(
//         "example_dlc_dev_kit",
//         "http://localhost:30000",
//         Network::Regtest,
//     )
//     .unwrap();

//     Use the BDK wallet.
//     dlc_dev_kit.wallet.new_external_address().unwrap().address;

//     Listen for DLC events.
//     dlc_dev_kit.relays.listen().await.unwrap();

//     Use the DLC manager.
//     dlc_dev_kit
//         .manager
//         .lock()
//         .unwrap()
//         .periodic_check(false)
//         .unwrap();

//     Handle DLC messages.
//     dlc_dev_kit.message_handler.has_pending_messages();

//     dlc_dev_kit API for DLC functions
//     dlc_dev_kit.send_dlc_offer(ContractInput, OracleAnnouncement, XOnlyPublicKey);
// }
fn main() {}
