use clap::Parser;
use ddk_node::opts::NodeOpts;
use ddk_node::DdkNode;
use std::str::FromStr;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = NodeOpts::parse();
    let base_level = LevelFilter::from_str(&opts.log).unwrap_or(LevelFilter::INFO);

    // Build the filter string based on the base level
    let filter_string = match base_level {
        LevelFilter::TRACE => "trace",
        LevelFilter::DEBUG => {
            "debug,hyper_util=info,sqlx=info,nostr_relay_pool=info,hyper=info,h2=info"
        }
        LevelFilter::INFO => "info",
        LevelFilter::WARN => "warn",
        LevelFilter::ERROR => "error",
        LevelFilter::OFF => "off",
    };

    // Parse the filter string, with fallback to the base level
    let filter = EnvFilter::from_str(filter_string).unwrap_or(EnvFilter::from_default_env());

    let subscriber = tracing_subscriber::fmt()
        .with_line_number(true)
        .with_file(false)
        // .with_target(false)
        .with_env_filter(filter)
        .finish();

    tracing::subscriber::set_global_default(subscriber).unwrap();

    DdkNode::serve(opts).await?;

    Ok(())
}
