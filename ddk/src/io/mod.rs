use bdk::bitcoin::{bip32::ExtendedPrivKey, Network};
use getrandom::getrandom;
use std::path::Path;

use crate::config::SeedConfig;

pub fn xprv_from_config(
    seed_config: &SeedConfig,
    network: Network,
) -> anyhow::Result<ExtendedPrivKey> {
    let seed = match seed_config {
        SeedConfig::Bytes(bytes) => ExtendedPrivKey::new_master(network, bytes)?,
        SeedConfig::File(file) => {
            if Path::new(&format!("{file}/seed")).exists() {
                let seed = std::fs::read(format!("{file}/seed"))?;
                let mut key = [0; 64];
                key.copy_from_slice(&seed);
                let xprv = ExtendedPrivKey::new_master(network, &seed)?;
                xprv
            } else {
                std::fs::File::create(format!("{file}/seed"))?;
                let mut entropy = [0u8; 64];
                getrandom(&mut entropy)?;
                // let _mnemonic = Mnemonic::from_entropy(&entropy)?;
                let xprv = ExtendedPrivKey::new_master(network, &entropy)?;
                std::fs::write(format!("{file}/seed"), &entropy)?;
                xprv
            }
        }
    };

    Ok(seed)
}
