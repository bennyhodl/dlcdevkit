use bdk::bitcoin::{bip32::ExtendedPrivKey, Network};
use getrandom::getrandom;
use std::{fs::File, io::Write, path::Path};

use crate::config::SeedConfig;

pub fn xprv_from_config(
    seed_config: &SeedConfig,
    network: Network,
) -> anyhow::Result<ExtendedPrivKey> {
    let seed = match seed_config {
        SeedConfig::Bytes(bytes) => ExtendedPrivKey::new_master(network, bytes)?,
        SeedConfig::File(file) => {
            if Path::new(&format!("{file}/seed.ddk")).exists() {
                tracing::info!("seed exists {}", network);
                let seed = std::fs::read(format!("{file}/seed.ddk"))?;
                let mut key = [0; 64];
                key.copy_from_slice(&seed);
                let xprv = ExtendedPrivKey::new_master(network, &seed)?;
                xprv
            } else {
                tracing::info!("seed doesnt exist");
                let mut file = File::create(format!("{file}/seed.ddk"))?;
                let mut entropy = [0u8; 64];
                getrandom(&mut entropy)?;
                // let _mnemonic = Mnemonic::from_entropy(&entropy)?;
                let xprv = ExtendedPrivKey::new_master(network, &entropy)?;
                file.write_all(&entropy)?;
                xprv
            }
        }
    };

    Ok(seed)
}
