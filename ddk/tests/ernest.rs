// #![allow(dead_code)]
// include!("./util.rs");
// use bdk::bitcoin::XOnlyPublicKey;
// use dlc::{
//     secp256k1_zkp::{rand::thread_rng, KeyPair, Message, Secp256k1, ONE_KEY},
//     EnumerationPayout, Payout,
// };
// use dlc_manager::contract::{
//     contract_input::{ContractInput, ContractInputInfo, OracleInput},
//     enum_descriptor::EnumDescriptor,
//     ContractDescriptor,
// };
// use dlc_messages::oracle_msgs::{
//     DigitDecompositionEventDescriptor, EnumEventDescriptor, EventDescriptor, OracleAnnouncement,
//     OracleEvent,
// };
// use lightning::util::ser::Writeable;
// use nostr::Keys;
//
// fn get_base_input(diff: &str) -> ContractInput {
//     // let secp = Secp256k1::new();
//     let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();
//
//     ContractInput {
//         offer_collateral: 1000000,
//         accept_collateral: 2000000,
//         fee_rate: 1234,
//         contract_infos: vec![ContractInputInfo {
//             contract_descriptor: ContractDescriptor::Enum(EnumDescriptor {
//                 outcome_payouts: vec![
//                     EnumerationPayout {
//                         outcome: diff.to_string(),
//                         payout: Payout {
//                             offer: 3000000,
//                             accept: 0,
//                         },
//                     },
//                     EnumerationPayout {
//                         outcome: "B".to_string(),
//                         payout: Payout {
//                             offer: 0,
//                             accept: 3000000,
//                         },
//                     },
//                 ],
//             }),
//             oracles: OracleInput {
//                 public_keys: vec![
//                     XOnlyPublicKey::from_keypair(&KeyPair::from_secret_key(&secp256k1, &ONE_KEY)).0,
//                 ],
//                 event_id: "1234".to_string(),
//                 threshold: 1,
//             },
//         }],
//     }
// }
//
// fn enum_descriptor(diff: &str) -> EnumEventDescriptor {
//     EnumEventDescriptor {
//         outcomes: vec![diff.to_string(), "B".to_string()],
//     }
// }
//
// fn digit_descriptor() -> DigitDecompositionEventDescriptor {
//     DigitDecompositionEventDescriptor {
//         base: 2,
//         is_signed: false,
//         unit: "kg/sats".to_string(),
//         precision: 1,
//         nb_digits: 10,
//     }
// }
//
// fn some_schnorr_pubkey() -> XOnlyPublicKey {
//     let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();
//     let key_pair = KeyPair::new(&secp256k1, &mut thread_rng());
//     XOnlyPublicKey::from_keypair(&key_pair).0
// }
//
// fn digit_event(nb_nonces: usize) -> OracleEvent {
//     OracleEvent {
//         oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
//         event_maturity_epoch: 10,
//         event_descriptor: EventDescriptor::DigitDecompositionEvent(digit_descriptor()),
//         event_id: "test".to_string(),
//     }
// }
//
// fn enum_event(nb_nonces: usize, diff: &str) -> OracleEvent {
//     OracleEvent {
//         oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
//         event_maturity_epoch: 10,
//         event_descriptor: EventDescriptor::EnumEvent(enum_descriptor(diff)),
//         event_id: "1234".to_string(),
//     }
// }
//
// fn get_oracle_announcement(diff: &str) -> OracleAnnouncement {
//     let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();
//     let key_pair = KeyPair::new(&secp256k1, &mut thread_rng());
//     let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
//     let event = enum_event(1, diff);
//     let mut event_hex = Vec::new();
//     event
//         .write(&mut event_hex)
//         .expect("Error writing oracle event");
//     let msg = Message::from_hashed_data::<dlc::secp256k1_zkp::hashes::sha256::Hash>(&event_hex);
//     let sig = secp256k1.sign_schnorr(&msg, &key_pair);
//     let valid_announcement = OracleAnnouncement {
//         announcement_signature: sig,
//         oracle_public_key: oracle_pubkey,
//         oracle_event: event,
//     };
//
//     valid_announcement
//         .validate(&secp256k1)
//         .expect("a valid announcement.");
//
//     valid_announcement
// }
//
// #[tokio::test]
// async fn send_dlc_offer_over_nostr() {
//     let test = OneWalletTest::setup_bitcoind_and_electrsd_and_dlc_dev_kit("send-dlc-offer").await;
//
//     let contract_input = get_base_input("A");
//     let oracle_announcement = get_oracle_announcement("A");
//
//     let recipient = Keys::generate();
//
//     // let send_offer = test
//     //     .dlc_dev_kit
//     //     .send_dlc_offer(
//     //         &contract_input,
//     //         &oracle_announcement,
//     //         recipient.public_key(),
//     //     )
//     //     .await;
//
//     // let dlc = test.dlc_dev_kit.manager.lock().unwrap();
//
//     // let store = dlc.get_store();
//
//     // let contract = store.contract_tree().unwrap().iter().count();
//
//     // drop(dlc);
//
//     // assert!(send_offer.is_ok());
//     // assert_eq!(contract, 1);
//
//     let contract_input_two = get_base_input("C");
//     let oracle_announcement_two = get_oracle_announcement("C");
//
//     // let send_offer_two = test
//     //     .dlc_dev_kit
//     //     .send_dlc_offer(
//     //         &contract_input_two,
//     //         &oracle_announcement_two,
//     //         recipient.public_key(),
//     //     )
//     //     .await;
//
//     // let dlc_two = test.dlc_dev_kit.manager.lock().unwrap();
//
//     // let store_two = dlc_two.get_store();
//
//     // let contract = store_two.contract_tree().unwrap().iter().count();
//
//     // assert!(send_offer_two.is_ok());
//     // assert_eq!(contract, 2)
// }
