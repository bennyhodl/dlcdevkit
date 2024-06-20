mod cli;

use std::sync::{Arc, RwLock};
use ddk::builder::DdkBuilder;
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::DdkConfig;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use clap::{CommandFactory, Parser};

use cli::Ddk;
use tokio::runtime::Runtime;

use crate::cli::match_ddk_command;

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;
type DdkRuntime = Arc<RwLock<Option<Runtime>>>;

fn main() -> anyhow::Result<()> {
    let mut config = DdkConfig::default();
    config.storage_path = std::env::current_dir()?.join("ddk-sample");

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

    let mut rl = DefaultEditor::new()?;
    if rl.load_history("history.txt").is_err() {
        println!("No previous history.");
    }

    println!("Network: {}", config.network.clone());
    println!("Pubkey: {}", ddk.transport.node_id);
    let rt = ddk.runtime.clone();
    ddk.start().unwrap();
    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str())?;
                let args = line.split_whitespace().collect::<Vec<_>>();
                if args.is_empty() {
                    continue;
                }

                let command = Ddk::try_parse_from(args);
                match command {
                    Ok(cli) => {
                        if let Some(command) = cli.command {
                            match_ddk_command(command, &ddk, rt.clone())?;
                        } else {
                            Ddk::command().print_help()?
                        }
                    }
                    Err(_) => {
                        println!("Command not found");
                        Ddk::command().print_help()?
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
