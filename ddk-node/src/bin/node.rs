use clap::Parser;
use ddk_node::opts::NodeOpts;
use ddk_node::DdkNode;
use std::str::FromStr;
use tracing::level_filters::LevelFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = NodeOpts::parse();

    let level = LevelFilter::from_str(&opts.log).unwrap_or(LevelFilter::INFO);
    let subscriber = tracing_subscriber::fmt()
        .with_line_number(true)
        .with_max_level(level)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    DdkNode::serve(opts).await?;

    Ok(())
}
