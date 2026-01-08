use clap::Parser;
use ddk_node::cli_opts::CliCommand;
use ddk_node::ddkrpc::ddk_rpc_client::DdkRpcClient;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

type HmacSha256 = Hmac<Sha256>;

fn compute_signature(timestamp: &str, secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(timestamp.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

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
    #[arg(long)]
    #[arg(help = "HMAC secret for authentication")]
    pub api_secret: Option<String>,
    #[clap(subcommand)]
    pub command: CliCommand,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = DdkCliArgs::parse();

    if let Some(secret) = opts.api_secret {
        let channel = Channel::from_shared(opts.server)?.connect().await?;
        let secret_bytes = secret.into_bytes();

        let mut client =
            DdkRpcClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_secs()
                    .to_string();

                let signature = compute_signature(&timestamp, &secret_bytes);

                let ts_value: MetadataValue<_> = timestamp.parse().unwrap();
                let sig_value: MetadataValue<_> = signature.parse().unwrap();

                req.metadata_mut().insert("x-timestamp", ts_value);
                req.metadata_mut().insert("x-signature", sig_value);
                Ok(req)
            });

        ddk_node::command::cli_command(opts.command, &mut client).await?;
    } else {
        let mut client = DdkRpcClient::connect(opts.server).await?;
        ddk_node::command::cli_command(opts.command, &mut client).await?;
    }

    Ok(())
}
