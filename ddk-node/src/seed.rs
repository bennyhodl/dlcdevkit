use ddk::bitcoin::bip32::Xpriv;
use ddk::bitcoin::key::rand;
use ddk::bitcoin::Network;
use rand::Fill;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

/// Helper function that reads `[bitcoin::bip32::Xpriv]` bytes from a file.
/// If the file does not exist then it will create a file `seed.ddk` in the specified path.
pub fn xprv_from_path(path: PathBuf, network: Network) -> anyhow::Result<Xpriv> {
    let seed_path = path.join("seed.ddk");
    let seed = if Path::new(&seed_path).exists() {
        let seed = std::fs::read(&seed_path)?;
        let mut key = [0; 32];
        key.copy_from_slice(&seed);
        Xpriv::new_master(network, &seed)?
    } else {
        let mut file = File::create(&seed_path)?;
        let mut entropy = [0u8; 32];
        entropy.try_fill(&mut rand::thread_rng())?;
        // let _mnemonic = Mnemonic::from_entropy(&entropy)?;
        let xprv = Xpriv::new_master(network, &entropy)?;
        file.write_all(&entropy)?;
        xprv
    };

    Ok(seed)
}
