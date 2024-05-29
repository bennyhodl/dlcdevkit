use ddk::builder::DdkBuilder;
use ddk::{DdkOracle, DdkStorage, DdkTransport};
use std::sync::Arc;

#[derive(Clone)]
struct MockTransport;
impl DdkTransport for MockTransport {}

#[derive(Clone)]
struct MockStorage;
impl DdkStorage for MockStorage {}

#[derive(Clone)]
struct MockOracle;
impl DdkOracle for MockOracle {}

type ApplicationDdk = ddk::DlcDevKit<MockTransport, MockStorage, MockOracle>;

#[tokio::main]
async fn main() {
    let transport = Arc::new(MockTransport {});
    let storage = Arc::new(MockStorage {});
    let oracle_client = Arc::new(MockOracle {});
    let builder: ApplicationDdk = DdkBuilder::new()
        .set_name("ddk")
        .set_esplora_url("https://mempool.space/api")
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .await
        .unwrap();

    let wallet = builder.wallet.new_external_address();

    assert!(wallet.is_ok());
}
