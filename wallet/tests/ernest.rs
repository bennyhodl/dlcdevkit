#![allow(dead_code)]
include!("./util.rs");
use bdk::bitcoin::XOnlyPublicKey;
use dlc::{
    secp256k1_zkp::{rand::thread_rng, KeyPair, Message, Secp256k1, ONE_KEY},
    EnumerationPayout, Payout,
};
use dlc_manager::contract::{
    contract_input::{ContractInput, ContractInputInfo, OracleInput},
    enum_descriptor::EnumDescriptor,
    ContractDescriptor,
};
use dlc_messages::oracle_msgs::{
    DigitDecompositionEventDescriptor, EnumEventDescriptor, EventDescriptor, OracleAnnouncement,
    OracleEvent,
};
use lightning::util::ser::Writeable;
use nostr::Keys;

fn get_base_input() -> ContractInput {
    // let secp = Secp256k1::new();
    let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();

    ContractInput {
        offer_collateral: 1000000,
        accept_collateral: 2000000,
        fee_rate: 1234,
        contract_infos: vec![ContractInputInfo {
            contract_descriptor: ContractDescriptor::Enum(EnumDescriptor {
                outcome_payouts: vec![
                    EnumerationPayout {
                        outcome: "A".to_string(),
                        payout: Payout {
                            offer: 3000000,
                            accept: 0,
                        },
                    },
                    EnumerationPayout {
                        outcome: "B".to_string(),
                        payout: Payout {
                            offer: 0,
                            accept: 3000000,
                        },
                    },
                ],
            }),
            oracles: OracleInput {
                public_keys: vec![
                    XOnlyPublicKey::from_keypair(&KeyPair::from_secret_key(&secp256k1, &ONE_KEY)).0,
                ],
                event_id: "1234".to_string(),
                threshold: 1,
            },
        }],
    }
}
fn enum_descriptor() -> EnumEventDescriptor {
    EnumEventDescriptor {
        outcomes: vec!["1".to_string(), "2".to_string(), "3".to_string()],
    }
}

fn digit_descriptor() -> DigitDecompositionEventDescriptor {
    DigitDecompositionEventDescriptor {
        base: 2,
        is_signed: false,
        unit: "kg/sats".to_string(),
        precision: 1,
        nb_digits: 10,
    }
}

fn some_schnorr_pubkey() -> XOnlyPublicKey {
    let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();
    let key_pair = KeyPair::new(&secp256k1, &mut thread_rng());
    XOnlyPublicKey::from_keypair(&key_pair).0
}

fn digit_event(nb_nonces: usize) -> OracleEvent {
    OracleEvent {
        oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
        event_maturity_epoch: 10,
        event_descriptor: EventDescriptor::DigitDecompositionEvent(digit_descriptor()),
        event_id: "test".to_string(),
    }
}

fn enum_event(nb_nonces: usize) -> OracleEvent {
    OracleEvent {
        oracle_nonces: (0..nb_nonces).map(|_| some_schnorr_pubkey()).collect(),
        event_maturity_epoch: 10,
        event_descriptor: EventDescriptor::EnumEvent(enum_descriptor()),
        event_id: "1234".to_string(),
    }
}

fn get_oracle_announcement() -> OracleAnnouncement {
    let secp256k1: Secp256k1<bdk::bitcoin::secp256k1::All> = Secp256k1::new();
    let key_pair = KeyPair::new(&secp256k1, &mut thread_rng());
    let oracle_pubkey = XOnlyPublicKey::from_keypair(&key_pair).0;
    let event = enum_event(1);
    let mut event_hex = Vec::new();
    event
        .write(&mut event_hex)
        .expect("Error writing oracle event");
    let msg = Message::from_hashed_data::<dlc::secp256k1_zkp::hashes::sha256::Hash>(&event_hex);
    let sig = secp256k1.sign_schnorr(&msg, &key_pair);
    let valid_announcement = OracleAnnouncement {
        announcement_signature: sig,
        oracle_public_key: oracle_pubkey,
        oracle_event: event,
    };

    valid_announcement
        .validate(&secp256k1)
        .expect("a valid announcement.");

    valid_announcement
}

#[tokio::test]
#[ignore = "dont have test data yet"]
async fn send_dlc_offer_over_nostr() {
    let test = OneWalletTest::setup_bitcoind_and_electrsd_and_ernest("send-dlc-offer");

    let contract_input = get_base_input();
    let oracle_announcement = get_oracle_announcement();

    let recipient = Keys::generate();

    let send_offer = test
        .ernest
        .send_dlc_offer(
            &contract_input,
            &oracle_announcement,
            recipient.public_key(),
        )
        .await;

    println!("SNED {:?}", send_offer);

    assert!(send_offer.is_ok())
}
