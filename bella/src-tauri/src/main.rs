// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod functions;
mod models;
mod nostr;

use std::sync::Arc;
use crate::functions::*;
use log::LevelFilter;
use tauri_plugin_log::LogTarget;

use ddk::DlcDevKit;
use ddk::config::{DdkConfig, SeedConfig};
use ddk::builder::DdkBuilder;
use ddk::storage::SledStorageProvider;
use ddk::transport::lightning::LightningTransport;
use ddk::oracle::P2PDOracleClient;

pub type BellaDdk = DlcDevKit<LightningTransport, SledStorageProvider, P2PDOracleClient>;

fn bella_config() -> DdkConfig {
    let mut config = DdkConfig::default();
    let home_dir = "/Users/ben/.ddk/terminal";
    config.seed_config = SeedConfig::File(home_dir.to_string());
    config.storage_path = home_dir.into();
    config.network = ddk::Network::Regtest;
    config.esplora_host = ddk::ESPLORA_HOST.to_string();
    config
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = bella_config();

    let transport = Arc::new(LightningTransport::new(&config.seed_config, 1776, config.network)?);
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
            list_peers,
            dlc::list_contracts,
            dlc::list_offers,
            dlc::list_offers_async,
            dlc::accept_dlc,
        ])
        .setup(move |_app| {
            let _ = ddk_runtime.start();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running bella");
    Ok(())
}
