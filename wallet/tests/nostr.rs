include!("./util.rs");
use dlc_messages::Message;
use dlc_messages::OfferDlc;
use nostr::{
    secp256k1::{Secp256k1, SecretKey},
    Keys,
};

fn get_nostr_keys(name: &str) -> Keys {
    let nostr_key = io::get_ernest_dir().join(name).join("nostr_keys");
    let secp = Secp256k1::new();
    let seed_bytes = std::fs::read(nostr_key).unwrap();
    let secret_key = SecretKey::from_slice(&seed_bytes).unwrap();
    Keys::new_with_ctx(&secp, secret_key)
}

#[test]
fn create_and_parse_nostr_dlc_offfer() {
    let name = "create-nostr-offer";
    let test = OneWalletTest::setup_bitcoind_and_electrsd_and_ernest(name);

    let offer_str = include_str!("./data/dlc/offer.json");

    let offer: OfferDlc = serde_json::from_str(offer_str).unwrap();

    let msg = Message::Offer(offer.clone());

    let recipient = get_nostr_keys(name).public_key();

    let event = test
        .ernest
        .nostr
        .create_dlc_msg_event(recipient, None, msg)
        .unwrap();

    let parse = test.ernest.nostr.parse_dlc_msg_event(&event).unwrap();

    match parse {
        Message::Offer(parse_offer) => assert_eq!(parse_offer, offer),
        _ => panic!("Wrong message type"),
    }
}
