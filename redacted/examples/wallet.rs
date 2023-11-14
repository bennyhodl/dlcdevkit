use redacted::ErnestWallet;
use bdk::bitcoin::Network;

fn main() {
    println!("heyhowareya");
    let wallet = ErnestWallet::new(Network::Regtest).unwrap();
    
    // let address = wallet.wallet.try_write().unwrap().get_address(AddressIndex::New);
    let address = wallet.new_address().unwrap();

    println!("Address: {:?}", address)
}
