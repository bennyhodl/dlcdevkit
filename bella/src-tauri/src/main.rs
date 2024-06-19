// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod functions;
mod models;
mod nostr;

use std::sync::Arc;
use crate::functions::*;

// use crate::functions::{
//     dlc::{accept_dlc, list_contracts, list_offers},
//     wallet::{get_balance, new_address},
// };
use log::LevelFilter;
// use models::Pubkeys;
// use std::{
//     sync::{Arc, Mutex},
//     time::Duration,
// };
// use tauri::{Manager, State};
use tauri_plugin_log::LogTarget;
// use tokio::net::TcpListener;
// use tracing::Level;
// use tracing_subscriber::FmtSubscriber;

use ddk::{DdkConfig, DlcDevKit, SeedConfig};
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::oracle::P2PDOracleClient;

pub type BellaDdk = DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // let _ = fix_path_env::fix();
    // // let _ = env_logger::init();
    // env_logger::builder()
    //     .filter_level(LevelFilter::Info)
    //     .build();
    // // a builder for `FmtSubscriber`.
    // let subscriber = FmtSubscriber::builder()
    //     // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
    //     // will be written to stdout.
    //     .with_max_level(Level::INFO)
    //     // completes the builder.
    //     .finish();
    //
    // tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    // let dlc_manager_clone = bella.manager.clone();
    // let p2p_clone = p2p.clone();
    // tokio::spawn(async move {
    //     let mut ticker = tokio::time::interval(Duration::from_secs(5));
    //     loop {
    //         ticker.tick().await;
    //         println!("timer tick");
    //         let message_handler = p2p_clone.message_handler();
    //         let peer_manager = p2p_clone.peer_manager();
    //         let messages = message_handler.get_and_clear_received_messages();
    //         for (node_id, message) in messages {
    //             if let Ok(mut man) = dlc_manager_clone.lock() {
    //                 println!("Checking msg lock");
    //                 let resp = man.on_dlc_message(&message, node_id)
    //                     .expect("Error processing message");
    //
    //                 if let Some(msg) = resp {
    //                     message_handler.send_message(node_id, msg);
    //                 }
    //
    //                 if message_handler.has_pending_messages() {
    //                     peer_manager.process_events();
    //                 }
    //             } else {
    //                 println!("Could acquire lock");
    //                 continue;
    //             }
    //         }
    //     }
    // });

    let mut config = DdkConfig::default();
    let home_dir = "/Users/ben/.ddk/terminal";
    config.seed_config = SeedConfig::File(home_dir.to_string());
    config.storage_path = home_dir.into();
    let transport = Arc::new(LightningTransport::new(&config.seed_config, config.network)?);
    let storage = Arc::new(SledStorageProvider::new(
        config.storage_path.join("sled_db").to_str().expect("No storage."),
    )?);
    let oracle = tokio::task::spawn_blocking(|| {
        Arc::new(P2PDOracleClient::new(ddk::ORACLE_HOST).expect("no oracle"))
    }).await?;

    let mut builder = DdkBuilder::new();
    builder.set_name("bella");
    builder.set_config(config);
    builder.set_storage(storage);
    builder.set_transport(transport);
    builder.set_oracle(oracle);

    let ddk = Arc::new(builder.finish()?);
    let ddk_runtime = ddk.clone();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(LevelFilter::Info)
                .targets([LogTarget::Stdout, LogTarget::Webview, LogTarget::LogDir])
                .build(),
        )
        .manage(ddk.clone())
        .invoke_handler(tauri::generate_handler![
            // wallet
            wallet::new_address,
            get_pubkeys,
            wallet::get_balance,
            wallet::send,
            // // dlc
            // list_peers,
            // list_contracts,
            // list_offers,
            // accept_dlc,
        ])
        .setup(move |_app| {
            let _ = ddk_runtime.start();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running bella");
    Ok(())
}

// pub fn process_incoming_messages(
//     p2p: &Arc<DlcDevKitPeerManager>,
//     dlc_manager: &Arc<Mutex<DlcDevKitDlcManager>>,
// ) {
//     let message_handler = p2p.message_handler();
//     let peer_manager = p2p.peer_manager();
//     let messages = message_handler.get_and_clear_received_messages();
//
//     for (node_id, message) in messages {
//         let resp = dlc_manager
//             .lock()
//             .unwrap()
//             .on_dlc_message(&message, node_id)
//             .expect("Error processing message");
//         if let Some(msg) = resp {
//             message_handler.send_message(node_id, msg);
//         }
//     }
//
//     if message_handler.has_pending_messages() {
//         peer_manager.process_events();
//     }
// }
//
// async fn peer_manager_server(state: State<'_, Arc<DlcDevKitPeerManager>>) {
//     let peer_manager_connection_handler = state.peer_manager();
//
//     let listener = TcpListener::bind("0.0.0.0:9002")
//         .await
//         .expect("Coldn't get port.");
//
//     loop {
//         let peer_mgr = peer_manager_connection_handler.clone();
//         let (tcp_stream, _) = listener.accept().await.unwrap();
//         tauri::async_runtime::spawn(async move {
//             setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
//         });
//     }
// }
//
// async fn wallet_watcher(state: State<'_, Arc<DlcDevKit>>) {
//     let mut timer = tokio::time::interval(Duration::from_secs(10));
//     loop {
//         timer.tick().await;
//         log::info!("Syncing wallet...");
//         state.wallet.sync().unwrap();
//     }
// }
