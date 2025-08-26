use bitcoin::key::rand;
use rand::Fill;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

/// Helper function that reads `[bitcoin::bip32::Xpriv]` bytes from a file.
/// If the file does not exist then it will create a file `seed.ddk` in the specified path.
pub fn xprv_from_path(path: PathBuf) -> anyhow::Result<[u8; 64]> {
    let seed_path = path.join("seed.ddk");
    let seed = if Path::new(&seed_path).exists() {
        let seed = std::fs::read(&seed_path)?;
        let mut key = [0; 64];
        key.copy_from_slice(&seed);
        key
    } else {
        let mut file = File::create(&seed_path)?;
        let mut entropy = [0u8; 64];
        entropy.try_fill(&mut rand::thread_rng())?;
        file.write_all(&entropy)?;
        entropy
    };

    Ok(seed)
}
