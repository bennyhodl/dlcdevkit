mod test_util;

use chrono::{Local, TimeDelta};
use ddk::Transport;
use ddk_manager::{contract::Contract, Storage};
use ddk_messages::Message;
use ddk_payouts::options::{Direction, OptionType};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use test_util::{generate_blocks, test_ddk};
use tokio::time::sleep;

#[tokio::test]
async fn short_call() {
    let (alice, bob, oracle) = test_ddk().await;
    let expiry = TimeDelta::seconds(15);
    let timestamp: u32 = Local::now()
        .checked_add_signed(expiry)
        .unwrap()
        .timestamp()
        .try_into()
        .unwrap();
    let event_id = uuid::Uuid::new_v4().to_string();

    let announcement = oracle
        .oracle
        .create_numeric_event(event_id, 20, false, 2, "BTC/USD".to_string(), timestamp)
        .await
        .unwrap();

    let contract_input = ddk_payouts::options::build_option_order_offer(
        &announcement,
        100_000_000,
        50_000,
        500_000,
        1,
        1_000,
        OptionType::Call,
        Direction::Short,
        100_500_000,
        20,
    )
    .unwrap();

    println!("{:?}", contract_input.accept_collateral);
    println!("{:?}", contract_input.offer_collateral);
    println!("{:?}", contract_input.contract_infos);

    let offer = alice
        .ddk
        .manager
        .send_offer_with_announcements(
            &contract_input,
            bob.ddk.transport.public_key(),
            vec![vec![announcement.clone()]],
        )
        .await
        .unwrap();

    let contract_id = offer.temporary_contract_id.clone();

    bob.ddk
        .manager
        .on_dlc_message(
            &Message::Offer(offer),
            alice.ddk.transport.public_key().clone(),
        )
        .await
        .unwrap();

    let accept = bob.ddk.manager.accept_contract_offer(&contract_id).await;

    let (contract_id, _counterparty, accept_dlc) = accept.unwrap();

    let alice_sign = alice
        .ddk
        .manager
        .on_dlc_message(
            &Message::Accept(accept_dlc),
            bob.ddk.transport.public_key().clone(),
        )
        .await
        .unwrap();

    bob.ddk
        .manager
        .on_dlc_message(
            &alice_sign.unwrap(),
            alice.ddk.transport.public_key().clone(),
        )
        .await
        .unwrap();

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

    let contract = bob.ddk.storage.get_contract(&contract_id);
    println!("{:?}", contract);
    assert!(matches!(contract.unwrap().unwrap(), Contract::Confirmed(_)));

    bob.ddk.wallet.sync().unwrap();
    alice.ddk.wallet.sync().unwrap();

    // Used to check that timelock is reached.
    let locktime = match alice.ddk.storage.get_contract(&contract_id).unwrap() {
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
        .sign_numeric_event(announcement.oracle_event.event_id.clone(), 53_000)
        .await;

    assert!(attestation.is_ok());

    while time < announcement.clone().oracle_event.event_maturity_epoch || time < locktime {
        tracing::warn!("Waiting for time to expire for oracle event and locktime.");
        let checked_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        time = checked_time;
        generate_blocks(5);
    }

    bob.ddk.wallet.sync().unwrap();
    alice.ddk.wallet.sync().unwrap();

    bob.ddk
        .manager
        .close_confirmed_contract(&contract_id, vec![(0, attestation.unwrap())])
        .await
        .unwrap();

    sleep(Duration::from_secs(10)).await;

    let contract = bob.ddk.storage.get_contract(&contract_id).unwrap().unwrap();
    assert!(matches!(contract, Contract::PreClosed(_)));

    generate_blocks(10);

    bob.ddk.manager.periodic_check(false).await.unwrap();
    alice.ddk.manager.periodic_check(false).await.unwrap();

    let contract = bob.ddk.storage.get_contract(&contract_id);
    assert!(matches!(contract.unwrap().unwrap(), Contract::Closed(_)));

    let bob_contract = bob.ddk.storage.get_contract(&contract_id).unwrap().unwrap();
    let alice_contract = alice
        .ddk
        .storage
        .get_contract(&contract_id)
        .unwrap()
        .unwrap();

    match bob_contract {
        Contract::Closed(c) => println!("Bob: {} sats", c.pnl),
        _ => assert!(false),
    }

    match alice_contract {
        Contract::Closed(c) => println!("Alice: {} sats", c.pnl),
        _ => assert!(false),
    }
}
