use bdk::bitcoin::Network;
use ernest_wallet::ErnestWallet;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let wallet = ErnestWallet::new(
        "wallet".to_string(),
        "http://localhost:30000".to_string(),
        Network::Regtest,
    )
    .unwrap();

    let address = wallet.new_external_address().unwrap();
    println!("Address: {:?}", address);

    loop {
        let balance = wallet.get_balance().await.unwrap();
        println!("Balance {}", balance);
        std::thread::sleep(Duration::from_secs(3));
    }
}
