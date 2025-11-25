use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Clone, Debug)]
#[clap(name = "ddk-node")]
#[clap(
    about = "DDK Node for DLC Contracts",
    author = "benny b <ben@bitcoinbay.foundation>"
)]
#[clap(version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"))]
pub struct NodeOpts {
    #[arg(long)]
    #[arg(help = "Set the log level.")]
    #[arg(default_value = "info")]
    #[arg(value_parser = ["info", "debug"])]
    pub log: String,
    #[arg(short, long)]
    #[arg(help = "Set the Bitcoin network for DDK")]
    #[arg(default_value = "signet")]
    #[arg(value_parser = ["regtest", "mainnet", "signet"])]
    pub network: String,
    #[arg(short, long)]
    #[arg(
        help = "The path where ddk-node stores data. ddk-node will try to store in the $HOME directory by default."
    )]
    pub storage_dir: Option<PathBuf>,
    #[arg(short = 'p')]
    #[arg(long = "port")]
    #[arg(default_value = "1776")]
    #[arg(help = "Listening port for the lightning network transport.")]
    pub listening_port: u16,
    #[arg(long = "grpc")]
    #[arg(default_value = "0.0.0.0:3030")]
    #[arg(help = "Host and port the gRPC server will run on.")]
    pub grpc_host: String,
    #[arg(long = "esplora")]
    #[arg(default_value = "https://mutinynet.com/api")]
    #[arg(help = "Esplora server to connect to.")]
    pub esplora_host: String,
    #[arg(long = "oracle")]
    #[arg(default_value = "https://kormir.dlcdevkit.com")]
    #[arg(help = "Kormir oracle to connect to.")]
    pub oracle_host: String,
    #[arg(long)]
    #[arg(help = "Seed config strategy.")]
    #[arg(default_value = "file")]
    #[arg(value_parser = ["file", "bytes"])]
    pub seed: String,
    #[arg(long)]
    #[arg(help = "Name for the wallet.")]
    #[arg(default_value = "ddk-node")]
    pub name: String,
    #[arg(long)]
    #[arg(help = "Url for the postgres database connection.")]
    pub postgres_url: String,
}
