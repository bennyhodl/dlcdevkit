use dlc_messages::OfferDlc;
use super::util::TestSuite;
use crate::{Ernest, Network, nostr::NostrDlcHandler, io::get_ernest_dir};
use dlc_messages::Message;
use nostr::{Keys, secp256k1::{PublicKey, XOnlyPublicKey, SecretKey, Secp256k1}};

fn get_nostr_keys() -> Keys {
    let nostr_key = get_ernest_dir().join("nostr").join("nostr_keys");
    let secp = Secp256k1::new();
    let seed_bytes = std::fs::read(nostr_key).unwrap();
    let secret_key = SecretKey::from_slice(&seed_bytes).unwrap();
    Keys::new_with_ctx(&secp, secret_key)

    // XOnlyPublicKey::from_slice(seed_bytes.as).unwrap()
}

#[test]
fn nostr_manager() {
    let test = TestSuite::setup_bitcoind_and_electrsd_and_ernest("nostr");

    let offer_str = include_str!("../../test_files/dlc/offer.json");

    let offer: OfferDlc = serde_json::from_str(offer_str).unwrap();

    let msg = Message::Offer(offer.clone());

    let recipient = get_nostr_keys().public_key();

    let event = test.ernest.nostr.create_dlc_msg_event(recipient, None, msg).unwrap();

    let parse = test.ernest.nostr.parse_dlc_msg_event(&event).unwrap();

    match parse {
        Message::Offer(parse_offer) => assert_eq!(parse_offer, offer),
        _ => panic!("Wrong message type")
    }

}
