use bitcoin::key::Secp256k1;
use wasm_bindgen::prelude::*;
use serde::{Serialize, Deserialize};
use bitcoin::Network;
use std::sync::Arc;
use ddk::{
    builder::Builder, DlcDevKit,
};
use ddk::oracle::memory::MemoryOracle;
use ddk::transport::memory::MemoryTransport;
use ddk::storage::memory::MemoryStorage;

// Error type for Wasm operations
#[wasm_bindgen]
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    #[error("Failed to initialize Ddk")]
    InitError,
    #[error("Operation failed")]
    OperationError,
}

// Configuration for DdkWasm
#[derive(Serialize, Deserialize)]
pub struct DdkConfig {
    name: Option<String>,
    esplora_host: Option<String>,
    network: Option<String>,
    seed_bytes: Option<Vec<u8>>,
}

// Main Wasm wrapper for DdkDevKit
#[wasm_bindgen]
pub struct DdkWasm {
    inner: Arc<DlcDevKit<MemoryTransport, MemoryStorage, MemoryOracle>>,
}

#[wasm_bindgen]
impl DdkWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(name: String, esplora_host: String, network: String, seed: js_sys::Uint8Array) -> Result<DdkWasm, JsValue> {
        console_error_panic_hook::set_once(); 

        if seed.to_vec().len() != 32 {
            return Err(JsValue::from_str("Seed bytes are not exactly 32 bytes long."))
        }
        let mut seed_bytes = [0u8;32];
        seed_bytes.copy_from_slice(&seed.to_vec());

        let mut builder = Builder::new(); 
        builder.set_seed_bytes(seed_bytes);
        builder.set_name(&name);
        builder.set_esplora_host(esplora_host);
        let network = match network.as_str() {
            "mainnet" => Network::Bitcoin,
            "testnet" => Network::Testnet,
            "signet" => Network::Signet,
            _ => Network::Signet, // default to signet
        };
        builder.set_network(network);

        let secp = Secp256k1::new();
        
        // Set up web-specific implementations
        builder.set_transport(Arc::new(MemoryTransport::new(&secp)));
        builder.set_storage(Arc::new(MemoryStorage::new()));
        builder.set_oracle(Arc::new(MemoryOracle::new()));
        
        let ddk = builder.finish()
            .map_err(|e| JsValue::from_str(&format!("Failed to initialize Ddk: {}", e)))?;
            
        Ok(DdkWasm {
            inner: Arc::new(ddk),
        })
    }
    
    // Example method to start the Ddk process
    #[wasm_bindgen]
    pub async fn start(&self) -> Result<(), JsValue> {
        self.inner.start()
            .map_err(|e| JsValue::from_str(&format!("Failed to start Ddk: {}", e)))
    }    
}

// Initialize function called when the wasm module is loaded
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}
