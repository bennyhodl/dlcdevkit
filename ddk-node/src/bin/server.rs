use std::env::current_dir;
use std::sync::Arc;
use ddk::config::DdkConfig;
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::oracle::KormirOracleClient;
use ddk::transport::lightning::LightningTransport;
use ddk_node::ddkrpc::ddk_rpc_server::DdkRpcServer;
use ddk_node::DdkNode;
use tonic::transport::Server;

type DdkServer = ddk::DlcDevKit<LightningTransport, SledStorageProvider, KormirOracleClient>;

// toml options w/ clap
//  - storage dir
//  - seed file
//  - listener port
//  - name

#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::fmt().finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    tracing::info!("Starting DDK server");
    let mut config = DdkConfig::default();
    config.storage_path = current_dir().unwrap();

    let transport = Arc::new(LightningTransport::new(&config.seed_config, 1776, config.network).expect("transport fail"));
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().expect("No storage."),
    ).expect("sled failed"));

    let oracle_client = tokio::task::spawn_blocking(|| {
        Arc::new(KormirOracleClient::new().expect("no oracle"))
    }).await.unwrap();

    let mut builder = DdkBuilder::new();
    builder.set_config(config);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: DdkServer = builder.finish().expect("finish build");

    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());
    tracing::info!("Wallet is good");

    ddk.start().expect("couldn't start ddk");

    let guy = DdkNode::new(ddk);

    tracing::info!("Done with server.");

    Server::builder()
        .add_service(DdkRpcServer::new(guy))
        .serve("0.0.0.0:3030".parse().unwrap())
        .await.expect("Didn't start grpc");
}
