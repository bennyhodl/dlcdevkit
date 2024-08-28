use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use clap::Parser;
use ddk::config::DdkConfig;
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::oracle::P2PDOracleClient;
use ddk::transport::lightning::LightningTransport;
use ddk::Network;
use ddk_node::ddkrpc::ddk_rpc_server::DdkRpcServer;
use ddk_node::DdkNode;
use tonic::transport::Server;

type DdkServer = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

#[derive(Parser, Clone, Debug)]
struct NodeArgs {
    #[arg(short, long)]
    #[arg(help = "Set the Bitcoin network for DDK")]
    #[arg(default_value = "regtest")]
    network: String,
    #[arg(short, long)]
    #[arg(help = "The path where DlcDevKit will store data.")]
    storage_dir: Option<PathBuf>,
    #[arg(short = 'p')]
    #[arg(long = "port")]
    #[arg(default_value = "1776")]
    #[arg(help = "Listening port for network transport.")]
    listening_port: u16,
    #[arg(long = "grpc")]
    #[arg(default_value = "0.0.0.0:3030")]
    #[arg(help = "Host and port the gRPC server will run on.")]
    grpc_host: String,
    #[arg(long = "esplora")]
    #[arg(default_value = "http://127.0.0.1:30000")]
    #[arg(help = "Host to connect to an esplora server.")]
    esplora_host: String,
    #[arg(long = "oracle")]
    #[arg(default_value = "http://127.0.0.1:8080")]
    #[arg(help = "Host to connect to an oracle server.")]
    oracle_host: String
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = tracing_subscriber::fmt().finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let args = NodeArgs::parse();
    let mut config = DdkConfig::default();
    let storage_path = match args.storage_dir {
        Some(storage) => storage,
        None => homedir::my_home().expect("Provide a directory for ddk.").unwrap().join("defualt-ddk")
    };
    config.storage_path = storage_path;
    config.esplora_host = args.esplora_host;
    config.network = Network::from_str(&args.network)?;
    tracing::info!("Starting DDK server");

    let transport = Arc::new(LightningTransport::new(&config.seed_config, args.listening_port, config.network)?);
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().unwrap(),
    )?);

    let oracle_host = args.oracle_host.clone();
    let oracle_client = tokio::task::spawn_blocking(move || {
        Arc::new(P2PDOracleClient::new(&oracle_host).expect("Could not connect to oracle."))
    }).await.unwrap();

    let mut builder = DdkBuilder::new();
    builder.set_config(config);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: DdkServer = builder.finish()?;

    ddk.start()?;

    let node = DdkNode::new(ddk);

    Server::builder()
        .add_service(DdkRpcServer::new(node))
        .serve(args.grpc_host.parse()?)
        .await?;

    Ok(())
}
