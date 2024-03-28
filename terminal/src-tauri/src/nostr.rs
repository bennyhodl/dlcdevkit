use ernest_wallet::{
    nostr_manager::{ErnestNostr, NostrDlcHandler, RelayPoolNotification},
    Network, SledStorageProvider,
};
use std::sync::Arc;

#[allow(dead_code)]
pub async fn run_ernest_nostr() {
    let ernest =
        Arc::new(ErnestNostr::new("terminal", "http://localhost:30000", Network::Regtest).unwrap());

    let dlc_storage = SledStorageProvider::new("terminal").unwrap();

    // TODO: I think a receiver might be a better arch so it doesn't block incoming messages
    // let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let dlc_handler = Arc::new(NostrDlcHandler::new(dlc_storage));

    let relays_clone = ernest.relays.clone();
    let handler_clone = dlc_handler.clone();

    tokio::spawn(async move {
        let client = relays_clone.listen().await.unwrap();

        while let Ok(msg) = client.notifications().recv().await {
            match msg {
                RelayPoolNotification::Event {
                    relay_url: _,
                    event,
                    subscription_id: _,
                } => {
                    handler_clone.receive_event(*event);
                }
                _ => println!("other msg."),
            }
        }
    });
}
