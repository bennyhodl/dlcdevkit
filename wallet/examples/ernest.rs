use ernest_wallet::{ErnestNostr, Network};

#[tokio::main]
async fn main() {
    ErnestNostr::new("example_ernest", "http://localhost:30000", Network::Regtest).unwrap();

    // Use the BDK wallet.
    // ernest.wallet.new_external_address().unwrap().address;

    // Listen for DLC events.
    // ernest.relays.listen().await.unwrap();

    // Use the DLC manager.
    // ernest
    //     .manager
    //     .lock()
    //     .unwrap()
    //     .periodic_check(false)
    //     .unwrap();

    // Handle DLC messages.
    // ernest.message_handler.has_pending_messages();

    // Ernest API for DLC functions
    // ernest.send_dlc_offer(ContractInput, OracleAnnouncement, XOnlyPublicKey);
}
