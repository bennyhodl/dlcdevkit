mod cli;
mod logging;

use std::sync::Arc;
use ddk::{builder::DdkBuilder, DlcDevKit};
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::config::DdkConfig;
use logging::DdkSubscriber;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use clap::{CommandFactory, Parser};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

use crate::cli::{DdkCli, match_ddk_command};

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn main() -> anyhow::Result<()> {
    // let ddk_subscriber = DdkSubscriber;
    // let env_filter = EnvFilter::new("ddk=info,ddk_sample=info");
    // let subscriber = tracing_subscriber::registry().with(env_filter).with(ddk_subscriber);
    // let _ = tracing::subscriber::set_global_default(subscriber);
    // env_logger::init();

    let ddk = ddk_instance()?;
    ddk.start().unwrap();

    let mut rl = DefaultEditor::new()?;
    if rl.load_history("history.txt").is_err() {
        log::warn!("No previous history.");
    }


    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str())?;
                let args = line.split_whitespace().collect::<Vec<_>>();
                if args.is_empty() {
                    continue;
                }

                let command = DdkCli::try_parse_from(args);
                match command {
                    Ok(cli) => {
                        if let Some(command) = cli.command {
                            match_ddk_command(command, &ddk)?;
                        } else {
                            DdkCli::command().print_help()?
                        }
                    }
                    Err(_) => {
                        println!("Command not found");
                        DdkCli::command().print_help()?
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("Stopping ddk.");
                break;
            }
            Err(err) => {
                println!("Error: {}", err);
                break;
            }
        }
    }

    Ok(rl.save_history("history.txt")?)
}

fn ddk_instance() -> anyhow::Result<DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>> {
    let mut config = DdkConfig::default();
    config.storage_path = std::env::current_dir()?.join("ddk-sample");

    log::info!("Launching DDK instance.");
    log::info!("Network: {}", config.network);
    log::info!("Path: {:?}", config.storage_path);
    let transport = Arc::new(LightningTransport::new(&config.seed_config, config.network)?);
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().expect("No storage."),
    )?);
    let oracle_client = Arc::new(P2PDOracleClient::new(ddk::ORACLE_HOST).expect("no oracle"));

    let mut builder = DdkBuilder::new();
    builder.set_config(config.clone());
    builder.set_transport(transport.clone());
    builder.set_storage(storage.clone());
    builder.set_oracle(oracle_client.clone());

    let ddk: ApplicationDdk = builder.finish()?;
    log::info!("Transport pubkey: {}", ddk.transport.node_id.to_string());

    Ok(ddk)
}
