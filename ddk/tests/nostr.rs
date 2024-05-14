include!("./util.rs");
use dlc_messages::Message;
use dlc_messages::OfferDlc;
use nostr::{
    secp256k1::{Secp256k1, SecretKey},
    Keys,
};

fn get_nostr_keys(name: &str) -> Keys {
    let nostr_key = get_dlc_dev_kit_dir().join(name).join("nostr_keys");
    let secp = Secp256k1::new();
    let seed_bytes = std::fs::read(nostr_key).unwrap();
    let secret_key = SecretKey::from_slice(&seed_bytes).unwrap();
    Keys::new_with_ctx(&secp, secret_key.into())
}

#[tokio::test]
async fn create_and_parse_nostr_dlc_offfer() {
    let name = "create-nostr-offer";
    let test = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit(name).await;

    let offer_str = include_str!("./data/dlc/offer.json");

    let offer: OfferDlc = serde_json::from_str(offer_str).unwrap();

    let msg = Message::Offer(offer.clone());

    let recipient = get_nostr_keys(name).public_key();

    let event = test
        .dlc_dev_kit
        .relays
        .create_dlc_msg_event(recipient, None, msg)
        .unwrap();

    let parse = test.dlc_dev_kit.relays.parse_dlc_msg_event(&event).unwrap();

    match parse {
        Message::Offer(parse_offer) => assert_eq!(parse_offer, offer),
        _ => panic!("Wrong message type"),
    }
}

#[tokio::test]
async fn send_dlc_offer() {
    let name = "send-dlc-offer";
    let test = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit(name).await;

    let offer_str = include_str!("./data/dlc/offer.json");

    let offer: OfferDlc = serde_json::from_str(offer_str).unwrap();

    let msg = Message::Offer(offer.clone());

    let recipient = get_nostr_keys(name).public_key();

    let event = test
        .dlc_dev_kit
        .relays
        .create_dlc_msg_event(recipient, None, msg)
        .unwrap();

    println!("Created event with id: {}", event.id);

    let client = test.dlc_dev_kit.relays.listen().await.unwrap();

    client
        .send_event(event)
        .await
        .expect("Nostr event did not send.");
}

#[tokio::test]
async fn send_and_receive_dlc_offer() {
    let sender = "sender-dlc-offer";
    let receiver = "receiver-dlc-offer";
    let test = TwoWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit(sender, receiver).await;

    let offer_str = include_str!("./data/dlc/offer.json");

    let offer: OfferDlc = serde_json::from_str(offer_str).unwrap();

    let msg = Message::Offer(offer.clone());

    let recipient = get_nostr_keys(sender).public_key();

    let event = test
        .dlc_dev_kit_one
        .relays
        .create_dlc_msg_event(recipient, None, msg)
        .unwrap();

    println!("Created event with id: {}", event.id);

    let sender = test.dlc_dev_kit_one.relays.listen().await.unwrap();

    let receiver_nostr = test.dlc_dev_kit_two.relays.clone();

    let client = receiver_nostr.listen().await.unwrap();
    //
    // client
    //     .handle_notifications(|e| async move {
    //         match e {
    //             nostr_sdk::RelayPoolNotification::Event(_, e) => {
    //                 println!("THERE WAS AN EVENT: {}", e.id);
    //             }
    //             nostr_sdk::RelayPoolNotification::Message(_, e) => {
    //                 println!("MESSAGE?: {:?}", e);
    //             }
    //             _ => println!("Other event."),
    //         }
    //         Ok(false)
    //     })
    //     .await
    //     .unwrap();

    sender
        .send_event(event)
        .await
        .expect("Nostr event did not send.");
}
