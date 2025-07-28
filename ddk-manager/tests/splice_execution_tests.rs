extern crate bitcoincore_rpc;
extern crate bitcoincore_rpc_json;
extern crate ddk_manager;

#[macro_use]
#[allow(dead_code)]
mod test_utils;

use bitcoin::Amount;
use bitcoincore_rpc::{Client, RpcApi};
use ddk::chain::EsploraClient;
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk_manager::contract::Contract;
use ddk_manager::manager::Manager;
use ddk_manager::{Blockchain, CachedContractSignerProvider, Oracle, SimpleSigner, Wallet};
use ddk_manager::{ContractId, Storage};
use dlc_messages::Message;
use lightning::util::ser::Writeable;
use secp256k1_zkp::rand::{thread_rng, Fill};
use secp256k1_zkp::PublicKey;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use test_utils::*;
use tokio::sync::{mpsc::channel, Mutex};

type TestManager = Arc<
    Mutex<
        Manager<
            Arc<DlcDevKitWallet>,
            Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
            Arc<EsploraClient>,
            Arc<MemoryStorage>,
            Arc<MemoryOracle>,
            Arc<MockTime>,
            Arc<EsploraClient>,
            SimpleSigner,
        >,
    >,
>;

#[derive(Eq, PartialEq, Clone)]
enum SpliceTestPath {
    SpliceIn,
    SpliceOut,
}

pub struct SplicedInput {
    pub offer_public_key: PublicKey,
    pub accept_public_key: PublicKey,
    pub alice_public_key: PublicKey,
    pub alice_store: Arc<MemoryStorage>,
    pub bob_public_key: PublicKey,
    pub bob_store: Arc<MemoryStorage>,
    pub contract_id: ContractId,
    pub alice_manager: TestManager,
    pub bob_manager: TestManager,
    pub oracles: MemoryOracle,
    pub electrs: Arc<EsploraClient>,
    pub sink_rpc: Client,
}

async fn generate_blocks(nb_blocks: u64, electrs: &EsploraClient, sink_rpc: &Client) {
    let prev_blockchain_height = electrs.get_blockchain_height().await.unwrap();

    let sink_address = sink_rpc
        .get_new_address(None, None)
        .expect("RPC Error")
        .assume_checked();
    sink_rpc
        .generate_to_address(nb_blocks, &sink_address)
        .expect("RPC Error");

    // Wait for electrs to have processed the new blocks
    let mut cur_blockchain_height = prev_blockchain_height;
    while cur_blockchain_height < prev_blockchain_height + nb_blocks {
        std::thread::sleep(std::time::Duration::from_millis(200));
        cur_blockchain_height = electrs.get_blockchain_height().await.unwrap();
    }
}

async fn create_spliced_input() -> SplicedInput {
    env_logger::try_init().ok();
    let clients = init_clients().await;

    let mut alice_oracles = HashMap::with_capacity(1);
    let mut bob_oracles = HashMap::with_capacity(1);
    let test_params = get_single_funded_test_params(1, 1).await;
    for oracle in test_params.oracles.clone() {
        let oracle = Arc::new(oracle);
        alice_oracles.insert(oracle.get_public_key(), Arc::clone(&oracle));
        bob_oracles.insert(oracle.get_public_key(), Arc::clone(&oracle));
    }

    let alice_store = Arc::new(MemoryStorage::new());
    let bob_store = Arc::new(MemoryStorage::new());
    let mock_time = Arc::new(MockTime {});
    set_time((EVENT_MATURITY as u64) - 1);

    let electrs =
        Arc::new(EsploraClient::new("http://localhost:30000", bitcoin::Network::Regtest).unwrap());

    let mut alice_bytes = [0u8; 32];
    alice_bytes.try_fill(&mut thread_rng()).unwrap();

    let mut bob_bytes = [0u8; 32];
    bob_bytes.try_fill(&mut thread_rng()).unwrap();

    let alice_wallet = Arc::new(
        DlcDevKitWallet::new(
            &alice_bytes,
            "http://localhost:30000",
            bitcoin::Network::Regtest,
            alice_store.clone(),
        )
        .await
        .unwrap(),
    );

    let bob_wallet = Arc::new(
        DlcDevKitWallet::new(
            &bob_bytes,
            "http://localhost:30000",
            bitcoin::Network::Regtest,
            bob_store.clone(),
        )
        .await
        .unwrap(),
    );

    let alice_fund_address = alice_wallet.get_new_address().await.unwrap();
    let bob_fund_address = bob_wallet.get_new_address().await.unwrap();

    clients
        .4
        .send_to_address(
            &alice_fund_address,
            Amount::from_btc(2.0).unwrap(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    clients
        .4
        .send_to_address(
            &bob_fund_address,
            Amount::from_btc(2.0).unwrap(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    generate_blocks(6, &electrs, &clients.4).await;

    refresh_wallet(&alice_wallet, Amount::from_btc(2.0).unwrap().to_sat()).await;
    refresh_wallet(&bob_wallet, Amount::from_btc(2.0).unwrap().to_sat()).await;

    let alice_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&alice_wallet),
            Arc::clone(&alice_wallet),
            Arc::clone(&electrs),
            alice_store.clone(),
            alice_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
        )
        .await
        .unwrap(),
    ));

    let bob_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&bob_wallet),
            Arc::clone(&bob_wallet),
            Arc::clone(&electrs),
            bob_store.clone(),
            bob_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
        )
        .await
        .unwrap(),
    ));
    // Use consistent public keys like in manager execution tests
    let alice_pubkey: PublicKey =
        "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
            .parse()
            .unwrap();
    let bob_pubkey: PublicKey =
        "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
            .parse()
            .unwrap();

    let alice_offer = alice_manager
        .lock()
        .await
        .send_offer(&test_params.contract_input, bob_pubkey)
        .await
        .expect("error sending offer");

    let alice_offer_msg = Message::Offer(alice_offer.clone());

    let _bob_recv_offer = bob_manager
        .lock()
        .await
        .on_dlc_message(&alice_offer_msg, alice_pubkey)
        .await
        .expect("error receiving offer");

    let (contract_id, alice_pubkey_from_accept, bob_accept_msg) = bob_manager
        .lock()
        .await
        .accept_contract_offer(&alice_offer.temporary_contract_id)
        .await
        .unwrap();

    let bob_accept_msg = Message::Accept(bob_accept_msg);

    let alice_recv_accept = alice_manager
        .lock()
        .await
        .on_dlc_message(&bob_accept_msg, bob_pubkey)
        .await
        .expect("error receiving accept")
        .expect("to create a sign");

    let _dlc_managerbob_recv_sign = bob_manager
        .lock()
        .await
        .on_dlc_message(&alice_recv_accept, alice_pubkey_from_accept)
        .await
        .expect("to receive sign message");

    generate_blocks(10, &electrs, &clients.4).await;

    alice_wallet.sync().await.unwrap();
    bob_wallet.sync().await.unwrap();

    alice_manager
        .lock()
        .await
        .periodic_check(false)
        .await
        .expect("alice to update the contract");
    bob_manager
        .lock()
        .await
        .periodic_check(false)
        .await
        .expect("bob to update the contract");

    match alice_store
        .get_contract(&contract_id.clone())
        .await
        .unwrap()
        .expect("contract to exist")
    {
        Contract::Confirmed(_) => (),
        _ => panic!("contract to be confirmed"),
    };

    alice_wallet.sync().await.unwrap();
    bob_wallet.sync().await.unwrap();

    SplicedInput {
        offer_public_key: alice_pubkey,
        accept_public_key: bob_pubkey,
        alice_public_key: alice_pubkey,
        bob_public_key: bob_pubkey,
        alice_store: Arc::clone(&alice_store),
        bob_store: Arc::clone(&bob_store),
        contract_id,
        alice_manager: alice_manager,
        bob_manager: bob_manager,
        oracles: test_params.oracles[0].clone(),
        electrs: Arc::clone(&electrs),
        sink_rpc: clients.4,
    }
}

async fn splice_execution_test(spliced_input: SplicedInput, test_path: SpliceTestPath) {
    let (alice_send, mut bob_receive) = channel::<Option<Message>>(100);
    let (bob_send, mut alice_receive) = channel::<Option<Message>>(100);
    let (sync_send, mut sync_receive) = channel::<()>(100);
    let alice_sync_send = sync_send.clone();
    let bob_sync_send = sync_send;

    let alice_expect_error = Arc::new(AtomicBool::new(false));
    let bob_expect_error = Arc::new(AtomicBool::new(false));

    let alice_expect_error_loop = alice_expect_error.clone();
    let bob_expect_error_loop = bob_expect_error.clone();

    let alice_manager_loop = Arc::clone(&spliced_input.alice_manager);
    let alice_manager_send = Arc::clone(&spliced_input.alice_manager);
    let bob_manager_loop = Arc::clone(&spliced_input.bob_manager);
    let bob_manager_send = Arc::clone(&spliced_input.bob_manager);

    let alice_send_loop = alice_send.clone();
    let bob_send_loop = bob_send.clone();

    let msg_callback = |_msg: &Message| {};

    let alice_handle = receive_loop!(
        alice_receive,
        alice_manager_loop,
        alice_send_loop,
        alice_expect_error_loop,
        alice_sync_send,
        Some,
        msg_callback
    );

    let bob_handle = receive_loop!(
        bob_receive,
        bob_manager_loop,
        bob_send_loop,
        bob_expect_error_loop,
        bob_sync_send,
        Some,
        msg_callback
    );

    let test_params = match test_path {
        SpliceTestPath::SpliceIn => {
            get_splice_in_test_params(vec![spliced_input.oracles.clone()]).await
        }
        SpliceTestPath::SpliceOut => {
            get_splice_out_test_params(vec![spliced_input.oracles.clone()]).await
        }
    };

    let alice_splice_offer = alice_manager_send
        .lock()
        .await
        .send_splice_offer(
            &test_params.contract_input,
            spliced_input.bob_public_key,
            &spliced_input.contract_id,
        )
        .await
        .expect("error sending splice offer");

    let temporary_contract_id = alice_splice_offer.temporary_contract_id;
    alice_send
        .send(Some(Message::Offer(alice_splice_offer.clone())))
        .await
        .unwrap();

    assert_contract_state!(alice_manager_send, temporary_contract_id, Offered);

    sync_receive.recv().await.expect("Error synchronizing");

    assert_contract_state!(bob_manager_send, temporary_contract_id, Offered);

    let (contract_id, _, accept_msg) = bob_manager_send
        .lock()
        .await
        .accept_contract_offer(&temporary_contract_id)
        .await
        .expect("error accepting splice offer");

    assert_contract_state!(bob_manager_send, contract_id, Accepted);

    bob_send
        .send(Some(Message::Accept(accept_msg)))
        .await
        .unwrap();

    sync_receive.recv().await.expect("Error synchronizing");

    assert_contract_state!(alice_manager_send, contract_id, Signed);

    sync_receive.recv().await.expect("Error synchronizing");

    assert_contract_state!(bob_manager_send, contract_id, Signed);

    generate_blocks(10, &spliced_input.electrs, &spliced_input.sink_rpc).await;

    // Sync wallets before periodic checks
    // alice_wallet.sync().await.unwrap();
    // bob_wallet.sync().await.unwrap();

    alice_manager_send
        .lock()
        .await
        .periodic_check(false)
        .await
        .expect("alice to update the contract");
    bob_manager_send
        .lock()
        .await
        .periodic_check(false)
        .await
        .expect("bob to update the contract");

    assert_contract_state!(alice_manager_send, contract_id, Confirmed);
    assert_contract_state!(bob_manager_send, contract_id, Confirmed);

    let contract = match spliced_input
        .alice_store
        .get_contract(&contract_id)
        .await
        .unwrap()
        .expect("contract to exist")
    {
        Contract::Confirmed(c) => c,
        _ => panic!("contract should be in a confirmed state"),
    };

    let fund_tx = contract.accepted_contract.dlc_transactions.fund.clone();

    let _tx = spliced_input
        .electrs
        .get_transaction(&fund_tx.compute_txid())
        .await
        .expect("to have fund tx");

    alice_send.send(None).await.unwrap();
    bob_send.send(None).await.unwrap();

    alice_handle.await.unwrap();
    bob_handle.await.unwrap();
}

#[tokio::test]
#[ignore]
async fn splice_in_test() {
    let spliced_input = create_spliced_input().await;
    splice_execution_test(spliced_input, SpliceTestPath::SpliceIn).await;
}

#[tokio::test]
#[ignore]
async fn splice_out_test() {
    let spliced_input = create_spliced_input().await;
    splice_execution_test(spliced_input, SpliceTestPath::SpliceOut).await;
}
