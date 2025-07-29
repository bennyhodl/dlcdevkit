mod test_util;
use bitcoin::Amount;
use chrono::{Local, TimeDelta};
use ddk::util::ser::serialize_contract;
use ddk::Transport;
use ddk_manager::contract::Contract;
use ddk_manager::Storage;
use dlc::EnumerationPayout;
use dlc_messages::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_util::{generate_blocks, test_ddk};
use tokio::time::sleep;

#[macro_export]
macro_rules! write_contract {
    ($contract: ident, $state: ident) => {
        match &$contract {
            Contract::$state(_) => {
                let serialized =
                    serialize_contract(&$contract).expect("to be able to serialize the contract.");
                let dest_path = format!("{}", stringify!($state));
                std::fs::write(dest_path, serialized)
                    .expect("to be able to save the contract to file.");
            }
            _ => {}
        }
    };
}

#[macro_export]
macro_rules! assert_contract_state_and_serialize {
    ($storage:expr, $id:expr, $state:ident) => {
        let contract = $storage.get_contract(&$id).await.unwrap().unwrap();
        assert!(matches!(contract, Contract::$state(_)));
        if std::env::var("GENERATE_SERIALIZED_CONTRACT").is_ok() {
            write_contract!(contract, $state);
        }
    };
}

#[test_log::test(tokio::test)]
async fn enumeration_contract() {
    let (alice, bob, oracle) = test_ddk().await;
    let expiry = TimeDelta::seconds(15);
    let timestamp: u32 = Local::now()
        .checked_add_signed(expiry)
        .unwrap()
        .timestamp()
        .try_into()
        .unwrap();

    let announcement = oracle
        .oracle
        .create_enum_event(
            "test".into(),
            vec!["rust".to_string(), "go".to_string()],
            timestamp,
        )
        .await
        .unwrap();
    let contract_input = ddk_payouts::enumeration::create_contract_input(
        vec![
            EnumerationPayout {
                outcome: "rust".to_string(),
                payout: dlc::Payout {
                    offer: Amount::from_sat(100_000),
                    accept: Amount::ZERO,
                },
            },
            EnumerationPayout {
                outcome: "go".to_string(),
                payout: dlc::Payout {
                    offer: Amount::ZERO,
                    accept: Amount::from_sat(100_000),
                },
            },
        ],
        Amount::from_sat(50_000),
        Amount::from_sat(50_000),
        1,
        announcement.oracle_public_key.clone().to_string(),
        announcement.oracle_event.event_id.clone(),
    );

    let alice_makes_offer = alice.ddk.manager.send_offer_with_announcements(
        &contract_input,
        bob.ddk.transport.keypair.public_key(),
        vec![vec![announcement.clone()]],
    );

    let alice_makes_offer = alice_makes_offer
        .await
        .expect("alice did not create an offer");

    let contract_id = alice_makes_offer.temporary_contract_id.clone();
    let alice_pubkey = alice.ddk.transport.public_key();
    let bob_pubkey = bob.ddk.transport.public_key();

    // Serialize Offered state
    assert_contract_state_and_serialize!(alice.ddk.storage, contract_id, Offered);

    let bob_receives_offer = bob
        .ddk
        .manager
        .on_dlc_message(&Message::Offer(alice_makes_offer), alice_pubkey)
        .await;

    let bob_receive_offer = bob_receives_offer.expect("bob did not receive the offer");
    assert!(bob_receive_offer.is_none());

    // Serialize Offered state from Bob's perspective
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, Offered);

    let bob_accept_offer = bob
        .ddk
        .manager
        .accept_contract_offer(&contract_id)
        .await
        .expect("bob could not accept offer");

    let (contract_id, _counter_party, bob_accept_dlc) = bob_accept_offer;

    let alice_receive_accept = alice
        .ddk
        .manager
        .on_dlc_message(&Message::Accept(bob_accept_dlc), bob_pubkey)
        .await
        .expect("alice did not receive accept");

    assert!(alice_receive_accept.is_some());

    // Serialize Accepted state
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, Accepted);

    let alice_sign_message = alice_receive_accept.unwrap();
    bob.ddk
        .manager
        .on_dlc_message(&alice_sign_message, alice_pubkey)
        .await
        .expect("bob did not receive sign message");

    // Serialize Signed state
    assert_contract_state_and_serialize!(alice.ddk.storage, contract_id, Signed);
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, Signed);

    generate_blocks(10);

    alice
        .ddk
        .manager
        .periodic_check(false)
        .await
        .expect("alice check failed");

    bob.ddk
        .manager
        .periodic_check(false)
        .await
        .expect("bob check failed");

    let contract = alice.ddk.storage.get_contract(&contract_id).await.unwrap();
    assert!(matches!(contract.unwrap(), Contract::Confirmed(_)));

    // Serialize Confirmed state
    assert_contract_state_and_serialize!(alice.ddk.storage, contract_id, Confirmed);
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, Confirmed);

    bob.ddk.wallet.sync().await.unwrap();
    alice.ddk.wallet.sync().await.unwrap();

    // Used to check that timelock is reached.
    let locktime = match alice.ddk.storage.get_contract(&contract_id).await.unwrap() {
        Some(contract) => match contract {
            Contract::Confirmed(signed_contract) => {
                signed_contract.accepted_contract.dlc_transactions.cets[0]
                    .lock_time
                    .to_consensus_u32()
            }
            _ => unreachable!("No locktime."),
        },
        None => unreachable!("No locktime"),
    };

    let mut time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32;

    let attestation = alice
        .ddk
        .oracle
        .oracle
        .sign_enum_event(
            announcement.oracle_event.event_id.clone(),
            "rust".to_string(),
        )
        .await;

    while time < announcement.oracle_event.event_maturity_epoch || time < locktime {
        tracing::warn!("Waiting for time to expire for oracle event and locktime.");
        let checked_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        time = checked_time;
        generate_blocks(5);
    }

    assert!(attestation.is_ok());

    bob.ddk.wallet.sync().await.unwrap();
    alice.ddk.wallet.sync().await.unwrap();

    bob.ddk
        .manager
        .close_confirmed_contract(&contract_id, vec![(0, attestation.unwrap())])
        .await
        .unwrap();

    sleep(Duration::from_secs(10)).await;

    bob.ddk.manager.periodic_check(false).await.unwrap();

    let contract = bob
        .ddk
        .storage
        .get_contract(&contract_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(contract, Contract::PreClosed(_)));

    // Serialize PreClosed state
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, PreClosed);

    generate_blocks(10);

    bob.ddk.manager.periodic_check(false).await.unwrap();

    let contract = bob
        .ddk
        .storage
        .get_contract(&contract_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(contract, Contract::Closed(_)));

    // Serialize Closed state
    assert_contract_state_and_serialize!(bob.ddk.storage, contract_id, Closed);
}
