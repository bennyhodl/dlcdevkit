use clap::{Parser, Subcommand};
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;
use ddk_node::ddkrpc::InfoRequest;

#[derive(Debug, Clone, Parser)]
#[clap(author, version, about)]
struct  DdkCliArgs {
    #[clap(subcommand)]
    pub command: CliCommand
}

#[derive(Debug, Clone, Subcommand)]
enum CliCommand {
    // Gets information about the DDK instance
    Info
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = DdkCliArgs::parse();

    let mut client = DdkRpcClient::connect("http://127.0.0.1:3030").await?;

    match args.command {
        CliCommand::Info => {
            let info = client.info(InfoRequest::default()).await?.into_inner();
            println!("{:?}", info);
        }
    }

    println!("heyhowareya cli");

    Ok(())
}
