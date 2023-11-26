use bip39::Mnemonic;
use getrandom::getrandom;
use std::path::{Path, PathBuf};

pub fn read_or_generate_seed(seed_path: PathBuf) -> anyhow::Result<[u8; 16]> {
    if Path::new(&seed_path).exists() {
        let seed = std::fs::read(seed_path)?;
        let mut key = [0; 16];
        key.copy_from_slice(&seed);

        Ok(key)
    } else {
        let mut entropy = [0u8; 16];

        getrandom(&mut entropy)?;

        let _mnemonic = Mnemonic::from_entropy(&entropy)?;

        std::fs::write(seed_path, entropy)?;

        Ok(entropy)
    }
}

pub fn create_ernest_dir_with_wallet(wallet_name: String) -> anyhow::Result<PathBuf> {
    let dir = homedir::get_my_home()?.unwrap().join(".ernest");
    std::fs::create_dir_all(dir.clone())?;

    let file = dir.join(wallet_name);
    Ok(file)
}

pub fn get_wallet_dir(name: String) -> PathBuf {
    homedir::get_my_home()
        .unwrap()
        .unwrap()
        .join(format!(".ernest/{}", name))
}
