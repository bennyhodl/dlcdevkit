use bdk::bitcoin::Network;
use ernest_wallet::ErnestWallet;

#[tokio::main]
async fn main() {
    let ernest = ErnestWallet::new(
        "wallet".to_string(),
        "http://localhost:30000".to_string(),
        Network::Regtest,
    )
    .unwrap();

    let address = ernest.new_external_address().unwrap();
    println!("Address: {:?}", address);
}
