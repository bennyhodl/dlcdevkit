// use dlc_dev_kit::RELAY_HOST;
// use nostr::{Event, EventId, Keys};
// use nostr_sdk::Client;
//
// pub async fn send_event(event: &Event) -> EventId {
//     let keys = Keys::generate();
//     let client = Client::new(&keys);
//
//     client.add_relay(RELAY_HOST).await.unwrap();
//
//     client.connect().await;
//
//     let event_id = client.send_event(event.clone()).await.unwrap();
//
//     client.disconnect().await.unwrap();
//
//     event_id
// }
