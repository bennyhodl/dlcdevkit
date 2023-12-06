// wasm is considered "extra_unused_type_parameters"
#![allow(
    incomplete_features,
    clippy::extra_unused_type_parameters,
    clippy::arc_with_non_send_sync
)]

extern crate ernest_wallet;

use thiserror::Error;
use wasm_bindgen::prelude::*;
use ernest_wallet::{Ernest, Network};

#[derive(Debug, Error)]
pub enum ErnestJsError {
    #[error("JS Error")]
    Error
}


#[wasm_bindgen]
pub struct ErnestWallet {
    inner: ernest_wallet::Ernest
}

#[wasm_bindgen]
impl ErnestWallet {
    #[wasm_bindgen]
    pub fn new(name: String, esplora_url: String) -> ErnestWallet {
        let wallet = Ernest::new(name, esplora_url, Network::Regtest).expect("Cant");

        ErnestWallet { inner: wallet }
    }

    #[wasm_bindgen]
    pub fn new_wallet_address(&self) -> String {
        self.inner.wallet.new_external_address().unwrap().address.to_string()
    }
}


#[wasm_bindgen]
pub fn heyhowareya() {
    let _ = 1 + 1;
}
