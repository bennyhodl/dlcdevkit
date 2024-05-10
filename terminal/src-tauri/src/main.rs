// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod functions;
mod models;
mod nostr;

use crate::functions::{
    dlc::{accept_dlc, list_contracts, list_offers},
    wallet::{get_balance, new_address},
};
use ernest_wallet::{
    p2p::{lightning_net_tokio::setup_inbound, Ernest, ErnestDlcManager, ErnestPeerManager},
    Network,
};
use log::{info, LevelFilter};
use models::Pubkeys;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Manager, State};
use tauri_plugin_log::LogTarget;
use tokio::net::TcpListener;
use tracing_subscriber::FmtSubscriber;
use tracing::Level;

#[tauri::command]
fn get_pubkeys(ernest: State<Arc<Ernest>>, p2p: State<Arc<ErnestPeerManager>>) -> Pubkeys {
    let bitcoin = ernest.wallet.get_pubkey().unwrap().to_string();
    let node_id = p2p.node_id.to_string();

    Pubkeys { bitcoin, node_id }
}

#[tauri::command]
fn list_peers(p2p: State<Arc<ErnestPeerManager>>) -> Vec<String> {
    let mut node_ids = Vec::new();
    for (node_id, _) in p2p.peer_manager().get_peer_node_ids() {
        node_ids.push(node_id.to_string())
    }
    info!("{:?}", node_ids);
    node_ids
}

#[tokio::main]
async fn main() {
    let _ = fix_path_env::fix();
    // let _ = env_logger::init();
    env_logger::builder().filter_level(LevelFilter::Info).build();
    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::INFO)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    log::info!("heyhowareya");
    let name = "terminal".to_string();
    let ernest = Arc::new(
        Ernest::new(&name, "http://localhost:30000", Network::Regtest)
            .await
            .unwrap(),
    );

    let p2p = Arc::new(ErnestPeerManager::new(&name, Network::Regtest));

    let peer_manager_connection_handler = p2p.peer_manager();
    tokio::spawn(async move {
        let listener = TcpListener::bind("0.0.0.0:9002")
            .await
            .expect("Coldn't get port.");
        loop {
            let peer_mgr = peer_manager_connection_handler.clone();
            let (tcp_stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
            });
        }
    });

    let wallet_clone = ernest.wallet.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(10));
        loop {
            timer.tick().await;
            log::info!("Syncing wallet...");
            wallet_clone.sync().unwrap();
        }
    });

    // let dlc_manager_clone = ernest.manager.clone();
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

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(LevelFilter::Info)
                .targets([LogTarget::Stdout, LogTarget::Webview, LogTarget::LogDir])
                .build(),
        )
        .manage(ernest.clone())
        .manage(p2p.clone())
        .invoke_handler(tauri::generate_handler![
            // wallet
            new_address,
            get_pubkeys,
            get_balance,
            // dlc
            list_peers,
            list_contracts,
            list_offers,
            accept_dlc,
        ])
        .setup(move |_app| {
            // let p2p = Arc::new(ErnestPeerManager::new("terminal", Network::Regtest));
            // let app_handle = app.app_handle();
            //
            // let p2p_clone = p2p.clone();
            // let p2p_state = app_handle.state::<Arc<ErnestPeerManager>>().clone();
            // tauri::async_runtime::spawn(async move {
            //     peer_manager_server(p2p_state).await; 
            // });
            //
            // let wallet_state = app_handle.state::<Arc<Ernest>>().clone();
            // let wallet_clone = ernest.wallet.clone();
            // tauri::async_runtime::spawn(async move {
            //     wallet_watcher(wallet_state).await;
            // });

            Ok(())

        })
        .run(tauri::generate_context!())
        .expect("error while running ernest");
}

pub fn process_incoming_messages(
    p2p: &Arc<ErnestPeerManager>,
    dlc_manager: &Arc<Mutex<ErnestDlcManager>>,
) {
    let message_handler = p2p.message_handler();
    let peer_manager = p2p.peer_manager();
    let messages = message_handler.get_and_clear_received_messages();

    for (node_id, message) in messages {
        let resp = dlc_manager
            .lock()
            .unwrap()
            .on_dlc_message(&message, node_id)
            .expect("Error processing message");
        if let Some(msg) = resp {
            message_handler.send_message(node_id, msg);
        }
    }

    if message_handler.has_pending_messages() {
        peer_manager.process_events();
    }
}

async fn peer_manager_server(state: State<'_, Arc<ErnestPeerManager>>) {

    let peer_manager_connection_handler = state.peer_manager();

    let listener = TcpListener::bind("0.0.0.0:9002")
        .await
        .expect("Coldn't get port.");

    loop {
        let peer_mgr = peer_manager_connection_handler.clone();
        let (tcp_stream, _) = listener.accept().await.unwrap();
        tauri::async_runtime::spawn(async move {
            setup_inbound(peer_mgr.clone(), tcp_stream.into_std().unwrap()).await;
        });
    }
}

async fn wallet_watcher(state: State<'_, Arc<Ernest>>) {

    let mut timer = tokio::time::interval(Duration::from_secs(10));
    loop {
        timer.tick().await;
        log::info!("Syncing wallet...");
        state.wallet.sync().unwrap();
    }
}
