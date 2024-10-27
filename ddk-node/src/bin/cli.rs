use clap::Parser;
use ddk_node::cli_opts::CliCommand;
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;

#[derive(Debug, Clone, Parser)]
#[clap(name = "ddk-cli")]
#[clap(
    about = "CLI for ddk-node",
    author = "benny b <ben@bitcoinbay.foundation>"
)]
#[clap(version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"))]
struct DdkCliArgs {
    #[arg(short, long)]
    #[arg(help = "ddk-node gRPC server to connect to.")]
    #[arg(default_value = "http://127.0.0.1:3030")]
    pub server: String,
    #[clap(subcommand)]
    pub command: CliCommand,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = DdkCliArgs::parse();

    let mut client = DdkRpcClient::connect(opts.server).await?;

    ddk_node::command::cli_command(opts.command, &mut client).await?;

    Ok(())
}
