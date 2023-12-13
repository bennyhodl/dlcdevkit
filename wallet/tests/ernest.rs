include!("./util.rs");
use bdk::bitcoin::Amount;
use dlc::EnumerationPayout;
use dlc_manager::contract::enum_descriptor::EnumDescriptor;

#[test]
#[ignore = "not finished yet"]
fn send_dlc_offer_over_nostr() {
    let ernest = TwoWalletTest::setup_bitcoind_and_electrsd_and_ernest("send_dlc", "send_dlc_two");

    generate_blocks_and_wait(&ernest.bitcoind, &ernest.electrsd, 150);

    let receive_wallet_one = ernest.ernest_one.wallet.new_external_address().unwrap();
    let receive_wallet_two = ernest.ernest_two.wallet.new_external_address().unwrap();

    ernest.bitcoind
        .client
        .send_to_address(
            &receive_wallet_one.address,
            Amount::from_sat(100_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    ernest.bitcoind
        .client
        .send_to_address(
            &receive_wallet_two.address,
            Amount::from_sat(100_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    generate_blocks_and_wait(&ernest.bitcoind, &ernest.electrsd, 6);

    let blockspaces_raises = EnumerationPayout {
        payout: dlc::Payout { offer: 100_000_000, accept: 0 },
        outcome: "BlockSpaces Raises".to_string()
    };

    let blockspaces_doesnt = EnumerationPayout {
        payout: dlc::Payout { offer: 0, accept: 100_000_000 },
        outcome: "BlockSpaces Doesn't".to_string()
    };

    let _descriptor = EnumDescriptor {
        outcome_payouts: vec![blockspaces_raises, blockspaces_doesnt],
    };

    let _manager_one = ernest.ernest_one.manager.lock().unwrap();

}
