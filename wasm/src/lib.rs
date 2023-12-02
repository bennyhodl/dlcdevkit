// wasm is considered "extra_unused_type_parameters"
#![allow(
    incomplete_features,
    clippy::extra_unused_type_parameters,
    clippy::arc_with_non_send_sync
)]
use thiserror::Error;
use wasm_bindgen::prelude::*;
use ernest_wallet::Ernest;

// #[derive(Debug, Error)]
// pub enum ErnestJsError {
//     #[error("JS Error")]
//     Error
// }
//
// pub struct ErnestWallet {
//     inner: Ernest
// }
//
// #[wasm_bindgen]
// impl ErnestWallet {
//     #[wasm_bindgen(constructor)]
//     pub fn new() -> Result<ErnestWallet, ErnestJsError> {
//
//     }
// }


#[wasm_bindgen]
pub fn heyhowareya() {
    let _ = 1 + 1;
}
