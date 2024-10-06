use clap::Parser;
use ddk::bitcoin::Network;
use ddk::builder::DdkBuilder;
use ddk::config::SeedConfig;
use ddk::oracle::KormirOracleClient;
use ddk::storage::SledStorage;
use ddk::transport::lightning::LightningTransport;
use ddk_node::ddkrpc::ddk_rpc_server::DdkRpcServer;
use ddk_node::DdkNode;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tonic::transport::Server;
use tracing::level_filters::LevelFilter;

type DdkServer = ddk::DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[derive(Parser, Clone, Debug)]
#[clap(name = "ddk-node")]
#[clap(
    about = "DDK Node for DLC Contracts",
    author = "benny b <ben@bitcoinbay.foundation>"
)]
#[clap(version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"))]
struct NodeArgs {
    #[arg(long)]
    #[arg(help = "Set the log level.")]
    #[arg(default_value = "info")]
    #[arg(value_parser = ["info", "debug"])]
    log: String,
    #[arg(short, long)]
    #[arg(help = "Set the Bitcoin network for DDK")]
    #[arg(default_value = "regtest")]
    #[arg(value_parser = ["regtest", "mainnet", "signet"])]
    network: String,
    #[arg(short, long)]
    #[arg(
        help = "The path where ddk-node stores data. ddk-node will try to store in the $HOME directory by default."
    )]
    storage_dir: Option<PathBuf>,
    #[arg(short = 'p')]
    #[arg(long = "port")]
    #[arg(default_value = "1776")]
    #[arg(help = "Listening port for the lightning network transport.")]
    listening_port: u16,
    #[arg(long = "grpc")]
    #[arg(default_value = "0.0.0.0:3030")]
    #[arg(help = "Host and port the gRPC server will run on.")]
    grpc_host: String,
    #[arg(long = "esplora")]
    #[arg(default_value = "http://127.0.0.1:30000")]
    #[arg(help = "Esplora server to connect to.")]
    esplora_host: String,
    #[arg(long = "oracle")]
    #[arg(default_value = "http://127.0.0.1:8082")]
    #[arg(help = "Kormir oracle to connect to.")]
    oracle_host: String,
    #[arg(long)]
    #[arg(help = "Seed config strategy.")]
    #[arg(default_value = "file")]
    #[arg(value_parser = ["file", "bytes"])]
    seed: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = NodeArgs::parse();

    let level = LevelFilter::from_str(&args.log).unwrap_or(LevelFilter::INFO);
    let subscriber = tracing_subscriber::fmt()
        .with_line_number(true)
        .with_max_level(level)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let storage_path = match args.storage_dir {
        Some(storage) => storage,
        None => homedir::my_home()
            .expect("Provide a directory for ddk.")
            .unwrap()
            .join(".ddk")
            .join("default-ddk"),
    };
    let network = Network::from_str(&args.network)?;
    let seed_config = match args.seed.as_str() {
        "bytes" => SeedConfig::Bytes([0u8; 64]),
        _ => SeedConfig::File(storage_path.to_str().unwrap().to_string()),
    };

    std::fs::create_dir_all(storage_path.clone())?;

    tracing::info!("Starting DDK node.");

    let transport = Arc::new(LightningTransport::new(
        &seed_config,
        args.listening_port,
        network,
    )?);

    let storage = Arc::new(SledStorage::new(
        storage_path.join("sled_db").to_str().unwrap(),
    )?);

    // let oracle = Arc::new(P2PDOracleClient::new(&oracle_host).await?);
    let oracle = Arc::new(KormirOracleClient::new(&args.oracle_host).await?);

    let mut builder = DdkBuilder::new();
    builder.set_storage_path(storage_path.to_str().unwrap().to_string());
    builder.set_esplora_host(args.esplora_host);
    builder.set_network(network);
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle.clone());

    let ddk: DdkServer = builder.finish()?;

    ddk.start()?;

    let node = DdkNode::new(ddk);

    Server::builder()
        .add_service(DdkRpcServer::new(node))
        .serve(args.grpc_host.parse()?)
        .await?;

    Ok(())
}
