mod test_util;
use bitcoin::Amount;
use chrono::{Local, TimeDelta};
use ddk::Transport;
use ddk_manager::contract::Contract;
use ddk_manager::Storage;
use dlc::EnumerationPayout;
use dlc_messages::Message;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_util::{generate_blocks, test_ddk};
use tokio::time::sleep;

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

    let bob_receives_offer = bob
        .ddk
        .manager
        .on_dlc_message(&Message::Offer(alice_makes_offer), alice_pubkey)
        .await;

    let bob_receive_offer = bob_receives_offer.expect("bob did not receive the offer");
    assert!(bob_receive_offer.is_none());

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

    let alice_sign_message = alice_receive_accept.unwrap();
    bob.ddk
        .manager
        .on_dlc_message(&alice_sign_message, alice_pubkey)
        .await
        .expect("bob did not receive sign message");

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
}
