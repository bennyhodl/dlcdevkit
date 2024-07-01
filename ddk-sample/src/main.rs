#![allow(unused_imports)]
// 038f7597e3ba9b9fa66e13e71d9586f0c6fc2cef6be2086b1164c504bb822d50ab
mod cli;
mod logging;

use std::io;
use std::sync::Arc;
use std::time::Duration;
use crossterm::event::{KeyModifiers, MouseEventKind};
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
use tui::style::{Style, Color as TuiColor};

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

#[derive(Debug, clap::Parser)]
pub struct DdkSampleArgs {
    pub port: Option<u16>
}

fn main() -> anyhow::Result<()> {
    let start_args = DdkSampleArgs::parse();
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    let env_filter = EnvFilter::new("ddk=info,ddk_sample=info,lightning=info");
    let sub = tracing_subscriber::registry().with(SampleLogger { tx }).with(env_filter);
    let _ = tracing::subscriber::set_global_default(sub)?;

    // NOTE: use vecdeque for rolling logs
    let mut ddk_logs: Vec<String> = Vec::new();

    let ddk = ddk_instance(start_args.port.unwrap_or(1776))?;
    ddk.start().unwrap();


    let mut ddk_cli_input = String::new(); 
    let mut rl = DefaultEditor::new()?;
    if rl.load_history("history.txt").is_err() {
        log::warn!("No previous history.");
    }

    let mut ddk_cli_output: Vec<String> = Vec::new();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|frame| {
            let frame_size = frame.size();
            let panes = Layout::default()
                .direction(tui::layout::Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                .split(frame_size);

            let logs: Vec<ListItem> = ddk_logs.iter().map(|s| ListItem::new(s.as_ref()).style(Style { fg: Some(TuiColor::LightBlue), ..Default::default() })).collect();
            let logs_block = List::new(logs).block(Block::default().title("ddk logs").borders(Borders::ALL));
            frame.render_widget(logs_block.clone(), panes[1]);

            let previous_output = ddk_cli_output.join("\n");
            let repl_logs = format!("{}\n>> {}", previous_output, ddk_cli_input);
            let ddk_input = Paragraph::new(repl_logs.as_ref())
                .block(Block::default().title("ddk cli").borders(Borders::ALL));
            frame.render_widget(ddk_input, panes[0]);
        })?;

        while let Ok(log) = rx.try_recv() {
            ddk_logs.push(log); 
        }

        // Copy to clipboard. Need a crate or std lib functions to work with clipboard.
        // if event::poll(Duration::from_millis(100))? {
        //     if let Event::Key(key) = event::read()? {
        //         match (key.modifiers, key.code) {
        //             (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
        //                 let selected_text = logs.get_selected_text();
        //                 if !selected_text.is_empty() {
        //                     clipboard.set_contents(selected_text).unwrap();
        //                 }
        //             },
        //             // ... (keep other key handling code)
        //             _ => {}
        //         }
        //     } else if let Event::Mouse(mouse) = event::read()? {
        //         match mouse.kind {
        //             MouseEventKind::Down(event::MouseButton::Left) => {
        //                 logs.set_selection_start(mouse.row as usize, mouse.column as usize);
        //             },
        //             MouseEventKind::Drag(event::MouseButton::Left) => {
        //                 logs.set_selection_end(mouse.row as usize, mouse.column as usize);
        //             },
        //             _ => {}
        //         }
        //     }
        // }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Esc => {
                        break
                    },
                    KeyCode::Enter => {
                        let _result = handle_repl_input(&ddk_cli_input, &ddk, &mut ddk_cli_output)?;
                        // repl_output.push(format!("> {}", current_input));
                        // repl_output.push(result);
                        ddk_cli_input.clear();
                    },
                    KeyCode::Char(c) => {
                        ddk_cli_input.push(c);
                    },
                    KeyCode::Backspace => {
                        ddk_cli_input.pop();
                    },
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
        // let readline = rl.readline(">> ");
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

fn handle_repl_input(input: &str, ddk: &DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>, output: &mut Vec<String>) -> anyhow::Result<()> {
    if input == "clear" {
        return Ok(output.clear())
    }
    output.push(format!(">> {}", input));
    let input_vec: Vec<&str> = input.split(" ").collect();
    let command = DdkCli::try_parse_from(input_vec);
    match command {
        Ok(cli) => {
            if let Some(command) = cli.command {
                match_ddk_command(command, &ddk, output)?;
            } else {
                // DdkCli::command().print_help()?;
            }
        }
        Err(e) => {
            println!("{}", e.to_string().color(Color::Red3b));
            // DdkCli::command().print_help()?;
        }
    }
    Ok(())
}

fn ddk_instance(listening_port: u16) -> anyhow::Result<DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>> {
    let mut config = DdkConfig::default();
    config.storage_path = std::env::current_dir()?.join("ddk-sample");
    config.network = ddk::Network::Regtest;
    config.esplora_host = ddk::ESPLORA_HOST.to_string();
    config.seed_config = SeedConfig::File(config.storage_path.to_str().unwrap().to_string());

    log::info!("Launching DDK instance.");
    log::info!("Network: {}", config.network);
    log::info!("Path: {:?}", config.storage_path);
    let transport = Arc::new(LightningTransport::new(&config.seed_config, listening_port, config.network)?);
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
