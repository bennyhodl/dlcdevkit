use bdk::bitcoin::Network;
use ernest_wallet::build;

#[tokio::main]
async fn main() {
    let ernest = build(
        "wallet".to_string(),
        "http://localhost:30000".to_string(),
        Network::Regtest,
    )
    .unwrap();

    ernest.start();

    let address = ernest.wallet.new_external_address().unwrap();
    println!("Address: {:?}", address);

    loop {}
}
