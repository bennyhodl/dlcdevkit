#![allow(unused_imports)]
mod cli;
mod logging;

use std::io;
use std::sync::Arc;
use std::time::Duration;
use ddk::{builder::DdkBuilder, DlcDevKit};
use ddk::oracle::P2PDOracleClient;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::config::{DdkConfig, SeedConfig};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use clap::{CommandFactory, Parser};
use colorful::{Colorful, Color};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

use crate::cli::{DdkCli, match_ddk_command};
use logging::SampleLogger;
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

type ApplicationDdk = ddk::DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn main() -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let rx = Arc::new(rx);

    let env_filter = EnvFilter::new("ddk=info,ddk_sample=info");
    let sub = tracing_subscriber::registry().with(SampleLogger { tx }).with(env_filter);
    let _ = tracing::subscriber::set_global_default(sub)?;

    // NOTE: use vecdeque for rolling logs
    let mut ddk_logs: Vec<String> = Vec::new();

    let ddk = ddk_instance()?;
    ddk.start().unwrap();


    let mut ddk_cli_input = String::new(); 
    let mut rl = DefaultEditor::new()?;
    if rl.load_history("history.txt").is_err() {
        log::warn!("No previous history.");
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let rx_clone = rx.clone();
    std::thread::spawn(|| {
    });

    loop {
        terminal.draw(|frame| {
            let frame_size = frame.size();
            let panes = Layout::default()
                .direction(tui::layout::Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(frame_size);

            let logs: Vec<ListItem> = ddk_logs.iter().map(|s| ListItem::new(s.as_ref())).collect();
            let logs_block = List::new(logs);
            frame.render_widget(logs_block.clone(), panes[0]);

            let ddk_input = Paragraph::new(ddk_cli_input.as_ref())
                .block(Block::default().title("DDK CLI").borders(Borders::ALL));
            frame.render_widget(ddk_input, panes[1]);
        })?;

        while let Ok(log) = rx_clone.try_recv() {
            ddk_logs.push(log); 
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => {
                        break
                        // return Ok(())
                    },
                    KeyCode::Enter => {
                        // let result = handle_repl_input(&current_input);
                        // repl_output.push(format!("> {}", current_input));
                        // repl_output.push(result);
                        // current_input.clear();
                    },
                    // KeyCode::Char(c) => {
                    //     current_input.push(c);
                    // },
                    // KeyCode::Backspace => {
                    //     current_input.pop();
                    // },
                    _ => {}
                }
            }
        }
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;


    // Handle the repl input
    // loop {
    //     let readline = rl.readline(">> ");
    //     match readline {
    //         Ok(line) => {
    //             let _ = rl.add_history_entry(line.as_str())?;
    //             let args = line.split_whitespace().collect::<Vec<_>>();
    //             if args.is_empty() {
    //                 continue;
    //             }
    //
    //             let command = DdkCli::try_parse_from(args);
    //             match command {
    //                 Ok(cli) => {
    //                     if let Some(command) = cli.command {
    //                         match_ddk_command(command, &ddk)?;
    //                     } else {
    //                         DdkCli::command().print_help()?
    //                     }
    //                 }
    //                 Err(e) => {
    //                     println!("{}", e.to_string().color(Color::Red3b));
    //                     DdkCli::command().print_help()?
    //                 }
    //             }
    //         }
    //         Err(ReadlineError::Interrupted) => {
    //             println!("{}","Stopping ddk.".color(Color::Red));
    //             break;
    //         }
    //         Err(err) => {
    //             println!("Error: {}", err);
    //             break;
    //         }
    //     }
    // }

    // Ok(rl.save_history("history.txt")?)
    Ok(())
}

fn ddk_instance() -> anyhow::Result<DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>> {
    let mut config = DdkConfig::default();
    config.storage_path = std::env::current_dir()?.join("ddk-sample");
    config.network = ddk::Network::Regtest;
    config.esplora_host = ddk::ESPLORA_HOST.to_string();
    config.seed_config = SeedConfig::File(config.storage_path.to_str().unwrap().to_string());

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

    // ddk.storage.peers()?.iter().for_each(|peer| {
    //     println!("stored peer {:?}", peer);
    // });
    // println!("Peers: {:?}", ddk.storage.list_peers());
    
    Ok(ddk)
}
