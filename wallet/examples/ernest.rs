use ernest_wallet::{Ernest, Network};

#[tokio::main]
async fn main() {
    let ernest = Ernest::new("example_ernest".to_string(), "http://localhost:30000".to_string(), Network::Regtest).unwrap();

    // Use the BDK wallet.
    ernest.wallet.new_external_address().unwrap().address;
    
    // Listen for DLC events.
    ernest.nostr.listen().await.unwrap();

    // Use the DLC manager.
    ernest.manager.lock().unwrap().periodic_check(false).unwrap();

    // Handle DLC messages.
    ernest.message_handler.has_pending_messages();

    // Ernest API for DLC functions
    // ernest.send_dlc_offer(ContractInput, OracleAnnouncement, XOnlyPublicKey);
}
